use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;

use async_trait::async_trait;
use matrix_sdk::Client;
use matrix_sdk::authentication::oauth::UrlOrQuery;
use matrix_sdk::authentication::oauth::registration::{
    ApplicationType, ClientMetadata, Localized, OAuthGrantType,
};
use matrix_sdk::config::SyncSettings;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::utils::local_server::{LocalServerBuilder, LocalServerRedirectHandle};
use tokio::sync::Mutex;
use url::Url;

use crate::domain::models::{
    AuthMethod, LoginCredentials, OAuthLoginData, Room as DomainRoom, RoomId, ServerInfo, Session,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::MatrixPort;

pub struct MatrixAdapter {
    data_dir: PathBuf,
    client: Mutex<Option<Client>>,
    redirect_handle: Mutex<Option<LocalServerRedirectHandle>>,
}

impl MatrixAdapter {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            client: Mutex::new(None),
            redirect_handle: Mutex::new(None),
        }
    }
}

fn client_metadata() -> Result<Raw<ClientMetadata>> {
    let ipv4_uri: Url = format!("http://{}/", Ipv4Addr::LOCALHOST)
        .parse()
        .map_err(|e: url::ParseError| AppError::Matrix(e.to_string()))?;
    let ipv6_uri: Url = format!("http://[{}]/", Ipv6Addr::LOCALHOST)
        .parse()
        .map_err(|e: url::ParseError| AppError::Matrix(e.to_string()))?;
    let client_uri: Url = "https://github.com/drendog/U2DM"
        .parse()
        .map_err(|e: url::ParseError| AppError::Matrix(e.to_string()))?;

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

    Raw::new(&metadata).map_err(|e| AppError::Matrix(e.to_string()))
}

#[async_trait]
impl MatrixPort for MatrixAdapter {
    async fn discover_auth(&self, homeserver: &str) -> Result<ServerInfo> {
        let client = Client::builder()
            .server_name_or_homeserver_url(homeserver)
            .handle_refresh_tokens()
            .sqlite_store(self.data_dir.join("matrix-store"), None)
            .build()
            .await
            .map_err(|e| AppError::Matrix(e.to_string()))?;

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
        *self.client.lock().await = Some(client);

        Ok(ServerInfo {
            auth_methods: methods,
            homeserver_url,
        })
    }

    async fn login_password(&self, creds: LoginCredentials) -> Result<Session> {
        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| AppError::Matrix("No client, run server discovery first".into()))?;

        client
            .matrix_auth()
            .login_username(&creds.username, &creds.password)
            .initial_device_display_name("U2DM")
            .await
            .map_err(|e| AppError::Matrix(e.to_string()))?;

        let user_id = client
            .user_id()
            .ok_or_else(|| AppError::Matrix("No user ID after login".into()))?
            .to_string();
        let device_id = client
            .device_id()
            .ok_or_else(|| AppError::Matrix("No device ID after login".into()))?
            .to_string();
        let homeserver = client.homeserver().to_string();

        drop(guard);

        Ok(Session {
            user_id,
            device_id,
            homeserver,
        })
    }

    async fn login_oauth_start(&self) -> Result<OAuthLoginData> {
        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| AppError::Matrix("No client, run server discovery first".into()))?;

        let (redirect_uri, server_handle) = LocalServerBuilder::new()
            .spawn()
            .await
            .map_err(|e| AppError::Matrix(format!("Failed to start local callback server: {e}")))?;

        let metadata = client_metadata()?;
        let auth_data = client
            .oauth()
            .login(redirect_uri, None, Some(metadata.into()), None)
            .build()
            .await
            .map_err(|e| AppError::Matrix(e.to_string()))?;

        drop(guard);
        *self.redirect_handle.lock().await = Some(server_handle);

        Ok(OAuthLoginData {
            auth_url: auth_data.url.to_string(),
        })
    }

    async fn login_oauth_finish(&self) -> Result<Session> {
        let handle = self
            .redirect_handle
            .lock()
            .await
            .take()
            .ok_or_else(|| AppError::Matrix("No pending OAuth login".into()))?;

        let query_string = handle
            .await
            .ok_or_else(|| AppError::Matrix("No callback received from browser".into()))?;

        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| AppError::Matrix("No client, run server discovery first".into()))?;

        client
            .oauth()
            .finish_login(UrlOrQuery::Query(query_string.0))
            .await
            .map_err(|e| AppError::Matrix(e.to_string()))?;

        let user_id = client
            .user_id()
            .ok_or_else(|| AppError::Matrix("No user ID after OAuth login".into()))?
            .to_string();
        let device_id = client
            .device_id()
            .ok_or_else(|| AppError::Matrix("No device ID after OAuth login".into()))?
            .to_string();
        let homeserver = client.homeserver().to_string();

        drop(guard);

        Ok(Session {
            user_id,
            device_id,
            homeserver,
        })
    }

    async fn rooms(&self) -> Result<Vec<DomainRoom>> {
        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| AppError::Matrix("No client".into()))?;

        client
            .sync_once(SyncSettings::default())
            .await
            .map_err(|e| AppError::Matrix(e.to_string()))?;

        let joined_rooms = client.joined_rooms();
        drop(guard);

        let mut rooms = Vec::new();
        for room in joined_rooms {
            let display_name = room
                .display_name()
                .await
                .map(|dn| dn.to_string())
                .unwrap_or_default();
            let unread = room.unread_notification_counts().notification_count;
            let is_direct = room.is_direct().await.unwrap_or_default();
            rooms.push(DomainRoom {
                id: RoomId(room.room_id().to_string()),
                display_name,
                is_direct,
                unread_count: unread,
            });
        }

        Ok(rooms)
    }
}
