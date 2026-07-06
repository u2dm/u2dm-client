use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;

use matrix_sdk::authentication::SessionTokens;
use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::authentication::oauth::registration::{
    ApplicationType, ClientMetadata, Localized, OAuthGrantType,
};
use matrix_sdk::authentication::oauth::{ClientId, OAuthSession, UserSession};
use matrix_sdk::encryption::verification::VerificationRequest;
use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::media::MediaRetentionPolicy;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{IdParseError, OwnedDeviceId, OwnedUserId};
use matrix_sdk::utils::UrlOrQuery;
use matrix_sdk::utils::local_server::{LocalServerBuilder, LocalServerRedirectHandle};
use matrix_sdk::{Client, SessionChange, SessionMeta};
use tokio::fs;
use tokio::sync::{Mutex, RwLock, mpsc};
use url::Url;

use crate::domain::models::{AuthMethod, LoginCredentials, OAuthLoginData, ServerInfo, Session};
use crate::error::{AppError, Result};

pub(super) async fn discover_auth(
    client_lock: &RwLock<Option<Client>>,
    data_dir: &Path,
    cache_dir: &Path,
    homeserver: &str,
    passphrase: &str,
) -> Result<ServerInfo> {
    let client = Client::builder()
        .server_name_or_homeserver_url(homeserver)
        .handle_refresh_tokens()
        .respect_login_well_known(true)
        .sqlite_store_with_cache_path(
            data_dir.join("matrix-store"),
            cache_dir.join("matrix-store"),
            Some(passphrase),
        )
        .build()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;

    client
        .media()
        .set_media_retention_policy(MediaRetentionPolicy::new())
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;

    let mut methods = Vec::new();

    if client.oauth().server_metadata().await.is_ok() {
        methods.push(AuthMethod::OAuth);
    }

    if let Ok(login_types) = client.matrix_auth().get_login_types().await {
        methods.extend(
            login_types
                .flows
                .iter()
                .filter_map(|f| AuthMethod::from_login_type(f.login_type())),
        );
    }

    let homeserver_url = client.homeserver().to_string();
    tracing::info!(homeserver = %homeserver_url, methods = ?methods, "server discovery complete");
    *client_lock.write().await = Some(client);

    Ok(ServerInfo {
        auth_methods: methods,
        homeserver_url,
    })
}

pub(super) async fn login_password(client: &Client, creds: LoginCredentials) -> Result<Session> {
    tracing::info!(user = %creds.username, "logging in with password");
    client
        .matrix_auth()
        .login_username(&creds.username, &creds.password)
        .initial_device_display_name("U2DM")
        .await?;

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
        .await?;

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

pub(super) async fn restore_session(
    client_lock: &RwLock<Option<Client>>,
    data_dir: &Path,
    cache_dir: &Path,
    session: &Session,
    passphrase: &str,
    on_progress: Box<dyn Fn(String) + Send + Sync>,
) -> Result<()> {
    on_progress("connecting".into());

    let client = Client::builder()
        .homeserver_url(&session.homeserver)
        .handle_refresh_tokens()
        .respect_login_well_known(true)
        .sqlite_store_with_cache_path(
            data_dir.join("matrix-store"),
            cache_dir.join("matrix-store"),
            Some(passphrase),
        )
        .build()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;

    client
        .media()
        .set_media_retention_policy(MediaRetentionPolicy::new())
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;

    on_progress("restoring-auth".into());

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

    tracing::info!("session restored successfully");
    *client_lock.write().await = Some(client);
    Ok(())
}

pub(super) async fn logout(
    client_lock: &RwLock<Option<Client>>,
    verification_req_rx: &Mutex<Option<mpsc::UnboundedReceiver<VerificationRequest>>>,
    verification_handler_guards: &Mutex<Vec<EventHandlerDropGuard>>,
) -> Result<()> {
    tracing::info!("logging out");
    verification_handler_guards.lock().await.clear();
    let mut guard = client_lock.write().await;
    if let Some(client) = guard.as_ref()
        && let Err(e) = client.logout().await
    {
        tracing::warn!("failed to logout from server: {e}");
    }
    *guard = None;
    drop(guard);
    *verification_req_rx.lock().await = None;
    Ok(())
}

pub(super) async fn clear_store(
    client_lock: &RwLock<Option<Client>>,
    data_dir: &Path,
    cache_dir: &Path,
    verification_req_rx: &Mutex<Option<mpsc::UnboundedReceiver<VerificationRequest>>>,
    verification_handler_guards: &Mutex<Vec<EventHandlerDropGuard>>,
) -> Result<()> {
    tracing::info!("clearing matrix store");
    verification_handler_guards.lock().await.clear();
    *client_lock.write().await = None;
    let store_path = data_dir.join("matrix-store");
    if store_path.exists() {
        fs::remove_dir_all(&store_path).await?;
    }
    let cache_path = cache_dir.join("matrix-store");
    if cache_path.exists() {
        fs::remove_dir_all(&cache_path).await?;
    }
    *verification_req_rx.lock().await = None;
    tracing::debug!("matrix store cleared");
    Ok(())
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

pub(super) async fn subscribe_session_changes(
    client: &Client,
    session_tx: mpsc::UnboundedSender<Session>,
) -> Result<()> {
    let mut rx = client.subscribe_to_session_changes();

    while let Ok(change) = rx.recv().await {
        if change == SessionChange::TokensRefreshed
            && let Some(session) = extract_current_session(client)
            && session_tx.send(session).is_err()
        {
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
