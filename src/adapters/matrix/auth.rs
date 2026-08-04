use std::net::{Ipv4Addr, Ipv6Addr};

use matrix_sdk::authentication::SessionTokens;
use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::authentication::oauth::registration::{
    ApplicationType, ClientMetadata, Localized, OAuthGrantType,
};
use matrix_sdk::authentication::oauth::{ClientId, OAuthSession, UserSession};
use matrix_sdk::media::MediaRetentionPolicy;
use matrix_sdk::ruma::api::client::session::get_login_types::v3::LoginType;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{IdParseError, OwnedDeviceId, OwnedUserId};
use matrix_sdk::utils::UrlOrQuery;
use matrix_sdk::utils::local_server::{LocalServerBuilder, LocalServerRedirectHandle};
use matrix_sdk::{Client, ClientBuilder, HttpError, SessionChange, SessionMeta};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, mpsc};
use url::Url;

use super::store::StorePaths;
use crate::adapters::private_fs;
use crate::domain::auth::{AuthMethod, LoginCredentials, OAuthLoginData, ServerInfo, Session};
use crate::error::{AppError, AuthFailure, Result};
use crate::ports::matrix::RestoreStep;

async fn open_store(
    builder: ClientBuilder,
    paths: &StorePaths,
    passphrase: &str,
) -> Result<Client> {
    private_fs::create_dir(&paths.data).await?;
    private_fs::create_dir(&paths.cache).await?;

    let client = builder
        .handle_refresh_tokens()
        .respect_login_well_known(true)
        .sqlite_store_with_cache_path(&paths.data, &paths.cache, Some(passphrase))
        .build()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;

    client
        .media()
        .set_media_retention_policy(MediaRetentionPolicy::new())
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;

    Ok(client)
}

pub(super) async fn discover_auth(
    paths: &StorePaths,
    homeserver: &str,
    passphrase: &str,
) -> Result<(Client, ServerInfo)> {
    let client = open_store(
        Client::builder().server_name_or_homeserver_url(homeserver),
        paths,
        passphrase,
    )
    .await?;

    let (auth_methods, unsupported_flows) = probe_auth_methods(&client).await?;

    let homeserver_url = client.homeserver().to_string();
    tracing::info!(
        homeserver = %homeserver_url,
        methods = ?auth_methods,
        unsupported = ?unsupported_flows,
        "server discovery complete"
    );

    Ok((
        client,
        ServerInfo {
            auth_methods,
            unsupported_flows,
            homeserver_url,
        },
    ))
}

async fn probe_auth_methods(client: &Client) -> Result<(Vec<AuthMethod>, Vec<String>)> {
    let oauth = client.oauth().server_metadata().await;
    let login_types = client.matrix_auth().get_login_types().await;

    let (mut methods, unsupported_flows) = match &login_types {
        Ok(types) => partition_login_flows(types.flows.iter().map(LoginType::login_type)),
        Err(e) => {
            tracing::debug!("homeserver did not answer with its login types: {e}");
            (Vec::new(), Vec::new())
        }
    };

    match &oauth {
        Ok(_) => methods.push(AuthMethod::OAuth),
        Err(e) => tracing::debug!("homeserver does not offer OAuth: {e}"),
    }

    if let (Err(oauth_error), Err(login_types_error)) = (oauth, login_types) {
        return Err(AppError::Other(format!(
            "neither authentication API answered ({oauth_error}; {login_types_error})"
        )));
    }

    Ok((methods, unsupported_flows))
}

fn partition_login_flows<'a>(
    flows: impl Iterator<Item = &'a str>,
) -> (Vec<AuthMethod>, Vec<String>) {
    let mut methods = Vec::new();
    let mut unsupported = Vec::new();

    for flow in flows {
        match AuthMethod::from_login_type(flow) {
            Some(method) => methods.push(method),
            None => unsupported.push(flow.to_owned()),
        }
    }

    (methods, unsupported)
}

fn classify_auth_failure(error: &matrix_sdk::Error) -> AuthFailure {
    match error.client_api_error_kind() {
        Some(ErrorKind::Forbidden | ErrorKind::Unauthorized) => AuthFailure::InvalidCredentials,
        Some(ErrorKind::UserDeactivated) => AuthFailure::AccountDeactivated,
        Some(ErrorKind::InvalidUsername) => AuthFailure::InvalidUsername,
        Some(ErrorKind::LimitExceeded { .. }) => AuthFailure::RateLimited,
        Some(ErrorKind::Unrecognized) => AuthFailure::MethodUnsupported,
        Some(_) => AuthFailure::Unknown,
        None => match error {
            matrix_sdk::Error::Http(http) if is_transport_failure(http) => AuthFailure::Unreachable,
            _ => AuthFailure::Unknown,
        },
    }
}

fn is_transport_failure(error: &HttpError) -> bool {
    match error {
        HttpError::Reqwest(_) => true,
        HttpError::Cached(inner) => is_transport_failure(inner),
        _ => false,
    }
}

fn auth_error(error: &matrix_sdk::Error) -> AppError {
    AppError::Auth {
        kind: classify_auth_failure(error),
        detail: error.to_string(),
    }
}

pub(super) async fn login_password(client: &Client, creds: LoginCredentials) -> Result<Session> {
    tracing::info!(user = %creds.username, "logging in with password");
    client
        .matrix_auth()
        .login_username(&creds.username, &creds.password)
        .initial_device_display_name("U2DM")
        .await
        .map_err(|e| auth_error(&e))?;

    let sdk_session = client
        .matrix_auth()
        .session()
        .ok_or_else(|| AppError::Other("No session after login".into()))?;
    let homeserver = client.homeserver().to_string();
    tracing::info!(
        user_id = %sdk_session.meta.user_id,
        device_id = %sdk_session.meta.device_id,
        "password login successful"
    );

    Ok(Session {
        user_id: sdk_session.meta.user_id.to_string(),
        device_id: sdk_session.meta.device_id.to_string(),
        homeserver,
        access_token: sdk_session.tokens.access_token,
        refresh_token: sdk_session.tokens.refresh_token,
        client_id: None,
    })
}

pub(super) async fn login_oauth_start(
    client: &Client,
    redirect_handle: &Mutex<Option<LocalServerRedirectHandle>>,
) -> Result<OAuthLoginData> {
    tracing::info!("starting OAuth login flow");
    let (redirect_uri, server_handle) = LocalServerBuilder::new().spawn().await?;

    let metadata = client_metadata()?;
    let auth_data = client
        .oauth()
        .login(redirect_uri, None, Some(metadata.into()), None)
        .build()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;

    *redirect_handle.lock().await = Some(server_handle);

    Ok(OAuthLoginData {
        auth_url: auth_data.url.to_string(),
    })
}

pub(super) async fn login_oauth_finish(
    client: &Client,
    redirect_handle: &Mutex<Option<LocalServerRedirectHandle>>,
) -> Result<Session> {
    let handle = redirect_handle
        .lock()
        .await
        .take()
        .ok_or_else(|| AppError::Other("No pending OAuth login".into()))?;

    let query_string = handle
        .await
        .ok_or_else(|| AppError::Other("No callback received from browser".into()))?;

    client
        .oauth()
        .finish_login(UrlOrQuery::Query(query_string.0))
        .await
        .map_err(|e| auth_error(&e))?;

    let sdk_session = client
        .oauth()
        .full_session()
        .ok_or_else(|| AppError::Other("No session after OAuth login".into()))?;
    let homeserver = client.homeserver().to_string();
    tracing::info!(
        user_id = %sdk_session.user.meta.user_id,
        device_id = %sdk_session.user.meta.device_id,
        "OAuth login successful"
    );

    Ok(Session {
        user_id: sdk_session.user.meta.user_id.to_string(),
        device_id: sdk_session.user.meta.device_id.to_string(),
        homeserver,
        access_token: sdk_session.user.tokens.access_token,
        refresh_token: sdk_session.user.tokens.refresh_token,
        client_id: Some(sdk_session.client_id.to_string()),
    })
}

pub(super) async fn open_session(
    paths: &StorePaths,
    session: &Session,
    passphrase: &str,
    on_progress: &(dyn Fn(RestoreStep) + Send + Sync),
) -> Result<Client> {
    on_progress(RestoreStep::Connecting);

    let client = open_store(
        Client::builder().homeserver_url(&session.homeserver),
        paths,
        passphrase,
    )
    .await?;

    on_progress(RestoreStep::RestoringAuth);

    let user_id: OwnedUserId = session
        .user_id
        .as_str()
        .try_into()
        .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;
    let device_id: OwnedDeviceId = session.device_id.as_str().into();
    let meta = SessionMeta { user_id, device_id };
    let tokens = SessionTokens {
        access_token: session.access_token.clone(),
        refresh_token: session.refresh_token.clone(),
    };

    let auth_type = if session.client_id.is_some() {
        "OAuth"
    } else {
        "password"
    };
    tracing::info!(
        user_id = %session.user_id,
        device_id = %session.device_id,
        auth_type,
        "restoring session"
    );

    if let Some(client_id) = &session.client_id {
        let oauth_session = OAuthSession {
            client_id: ClientId::new(client_id.clone()),
            user: UserSession { meta, tokens },
        };
        client.restore_session(oauth_session).await?;
    } else {
        let matrix_session = MatrixSession { meta, tokens };
        client.restore_session(matrix_session).await?;
    }

    super::identity::ensure_identity_matches_server(&client).await?;

    tracing::info!("session restored successfully");
    Ok(client)
}

pub(super) fn extract_current_session(client: &Client) -> Option<Session> {
    let homeserver = client.homeserver().to_string();

    if let Some(oauth) = client.oauth().full_session() {
        return Some(Session {
            user_id: oauth.user.meta.user_id.to_string(),
            device_id: oauth.user.meta.device_id.to_string(),
            homeserver,
            access_token: oauth.user.tokens.access_token,
            refresh_token: oauth.user.tokens.refresh_token,
            client_id: Some(oauth.client_id.to_string()),
        });
    }

    if let Some(matrix) = client.matrix_auth().session() {
        return Some(Session {
            user_id: matrix.meta.user_id.to_string(),
            device_id: matrix.meta.device_id.to_string(),
            homeserver,
            access_token: matrix.tokens.access_token,
            refresh_token: matrix.tokens.refresh_token,
            client_id: None,
        });
    }

    None
}

fn send_current_session(client: &Client, session_tx: &mpsc::UnboundedSender<Session>) -> bool {
    extract_current_session(client).is_none_or(|session| session_tx.send(session).is_ok())
}

pub(super) async fn subscribe_session_changes(
    client: &Client,
    session_tx: mpsc::UnboundedSender<Session>,
) -> Result<()> {
    let mut changes = client.subscribe_to_session_changes();

    if !send_current_session(client, &session_tx) {
        return Ok(());
    }

    loop {
        match changes.recv().await {
            Ok(SessionChange::TokensRefreshed) => {}
            Ok(SessionChange::UnknownToken(_)) => continue,
            Err(RecvError::Lagged(missed)) => {
                tracing::debug!(missed, "missed session changes, saving the tokens in hand");
            }
            Err(RecvError::Closed) => break,
        }

        if !send_current_session(client, &session_tx) {
            break;
        }
    }

    Ok(())
}

fn client_metadata() -> Result<Raw<ClientMetadata>> {
    let ipv4_uri: Url = format!("http://{}/", Ipv4Addr::LOCALHOST)
        .parse()
        .map_err(|e: url::ParseError| AppError::Other(e.to_string()))?;
    let ipv6_uri: Url = format!("http://[{}]/", Ipv6Addr::LOCALHOST)
        .parse()
        .map_err(|e: url::ParseError| AppError::Other(e.to_string()))?;
    let client_uri: Url = "https://github.com/drendog/u2dm"
        .parse()
        .map_err(|e: url::ParseError| AppError::Other(e.to_string()))?;

    let client_uri = Localized::new(client_uri, []);
    let metadata = ClientMetadata {
        client_name: Some(Localized::new("U2DM".to_owned(), [])),
        ..ClientMetadata::new(
            ApplicationType::Native,
            vec![OAuthGrantType::AuthorizationCode {
                redirect_uris: vec![ipv4_uri, ipv6_uri],
            }],
            client_uri,
        )
    };

    Ok(Raw::new(&metadata)?)
}
