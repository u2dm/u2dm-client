use std::io::ErrorKind;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use matrix_sdk::authentication::SessionTokens;
use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::authentication::oauth::registration::{
    ApplicationType, ClientMetadata, Localized, OAuthGrantType,
};
use matrix_sdk::authentication::oauth::{ClientId, OAuthSession, UrlOrQuery, UserSession};
use matrix_sdk::config::SyncSettings;
use matrix_sdk::ruma::events::room::message::{MessageType, RoomMessageEventContent};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{IdParseError, OwnedDeviceId, OwnedRoomId, OwnedUserId};
use matrix_sdk::utils::local_server::{LocalServerBuilder, LocalServerRedirectHandle};
use matrix_sdk::{Client, SessionMeta};
use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::{EventTimelineItem, RoomExt, TimelineDetails, TimelineItem};
use rand::RngExt;
use rand::distr::Alphanumeric;
use tokio::fs;
use tokio::sync::{Mutex, mpsc};
use url::Url;

use crate::domain::models::{
    AuthMethod, EventId, LoginCredentials, MessageBody, OAuthLoginData, Room as DomainRoom, RoomId,
    ServerInfo, Session, SyncSnapshot, TimelineMessage,
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

    async fn get_or_create_passphrase(&self) -> Result<String> {
        let path = self.data_dir.join("db_passphrase");
        match fs::read_to_string(&path).await {
            Ok(passphrase) => Ok(passphrase),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let passphrase = generate_passphrase();
                fs::create_dir_all(&self.data_dir)
                    .await
                    .map_err(|e| AppError::Storage(e.to_string()))?;
                fs::write(&path, &passphrase)
                    .await
                    .map_err(|e| AppError::Storage(e.to_string()))?;
                Ok(passphrase)
            }
            Err(e) => Err(AppError::Storage(e.to_string())),
        }
    }
}

fn generate_passphrase() -> String {
    (&mut rand::rng())
        .sample_iter(Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

async fn build_room_list(client: &Client) -> Vec<DomainRoom> {
    let mut rooms = Vec::new();
    for room in client.joined_rooms() {
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
    rooms
}

fn convert_event_item(event: &EventTimelineItem) -> Option<TimelineMessage> {
    let message = event.content().as_message()?;

    let body = match message.msgtype() {
        MessageType::Text(t) => MessageBody::Text(t.body.clone()),
        MessageType::Notice(n) => MessageBody::Notice(n.body.clone()),
        MessageType::Emote(e) => MessageBody::Emote(e.body.clone()),
        MessageType::Image(i) => MessageBody::Image(i.body.clone()),
        MessageType::File(f) => MessageBody::File(f.body.clone()),
        other => MessageBody::Unknown(other.body().to_string()),
    };

    let sender_display_name = match event.sender_profile() {
        TimelineDetails::Ready(profile) => profile.display_name.clone(),
        _ => None,
    };

    let event_id = event
        .event_id()
        .map(ToString::to_string)
        .unwrap_or_default();
    let ts: u64 = event.timestamp().0.into();

    Some(TimelineMessage {
        event_id: EventId(event_id),
        sender: event.sender().to_string(),
        sender_display_name,
        body,
        timestamp: ts,
    })
}

fn convert_timeline_items(items: &[Arc<TimelineItem>]) -> Vec<TimelineMessage> {
    items
        .iter()
        .filter_map(|item| convert_event_item(item.as_event()?))
        .collect()
}

fn apply_timeline_diff(items: &mut Vec<Arc<TimelineItem>>, diff: VectorDiff<Arc<TimelineItem>>) {
    match diff {
        VectorDiff::Append { values } => items.extend(values),
        VectorDiff::Clear => items.clear(),
        VectorDiff::PushFront { value } => items.insert(0, value),
        VectorDiff::PushBack { value } => items.push(value),
        VectorDiff::PopFront => {
            if !items.is_empty() {
                items.remove(0);
            }
        }
        VectorDiff::PopBack => {
            items.pop();
        }
        VectorDiff::Insert { index, value } => {
            if index <= items.len() {
                items.insert(index, value);
            }
        }
        VectorDiff::Set { index, value } => {
            if let Some(slot) = items.get_mut(index) {
                *slot = value;
            }
        }
        VectorDiff::Remove { index } => {
            if index < items.len() {
                items.remove(index);
            }
        }
        VectorDiff::Truncate { length } => items.truncate(length),
        VectorDiff::Reset { values } => *items = values.into_iter().collect(),
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
        let passphrase = self.get_or_create_passphrase().await?;
        let client = Client::builder()
            .server_name_or_homeserver_url(homeserver)
            .handle_refresh_tokens()
            .sqlite_store(self.data_dir.join("matrix-store"), Some(&passphrase))
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

        let sdk_session = client
            .matrix_auth()
            .session()
            .ok_or_else(|| AppError::Matrix("No session after login".into()))?;
        let homeserver = client.homeserver().to_string();

        drop(guard);

        Ok(Session {
            user_id: sdk_session.meta.user_id.to_string(),
            device_id: sdk_session.meta.device_id.to_string(),
            homeserver,
            access_token: sdk_session.tokens.access_token,
            refresh_token: sdk_session.tokens.refresh_token,
            client_id: None,
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

        let sdk_session = client
            .oauth()
            .full_session()
            .ok_or_else(|| AppError::Matrix("No session after OAuth login".into()))?;
        let homeserver = client.homeserver().to_string();

        drop(guard);

        Ok(Session {
            user_id: sdk_session.user.meta.user_id.to_string(),
            device_id: sdk_session.user.meta.device_id.to_string(),
            homeserver,
            access_token: sdk_session.user.tokens.access_token,
            refresh_token: sdk_session.user.tokens.refresh_token,
            client_id: Some(sdk_session.client_id.to_string()),
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

        let rooms = build_room_list(client).await;
        drop(guard);

        Ok(rooms)
    }

    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        timeline_tx: mpsc::Sender<Vec<TimelineMessage>>,
    ) -> Result<()> {
        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| AppError::Matrix("No client".into()))?;

        let room_id_parsed: OwnedRoomId = room_id
            .0
            .as_str()
            .try_into()
            .map_err(|e: IdParseError| AppError::Matrix(e.to_string()))?;

        let room = client
            .get_room(&room_id_parsed)
            .ok_or_else(|| AppError::Matrix("Room not found".into()))?;

        drop(guard);

        let timeline = room
            .timeline()
            .await
            .map_err(|e| AppError::Matrix(e.to_string()))?;

        drop(timeline.paginate_backwards(50).await);

        let (initial_items, mut stream) = timeline.subscribe().await;

        let mut items: Vec<Arc<TimelineItem>> = initial_items.into_iter().collect();
        let messages = convert_timeline_items(&items);
        if timeline_tx.send(messages).await.is_err() {
            return Ok(());
        }

        while let Some(diffs) = stream.next().await {
            for diff in diffs {
                apply_timeline_diff(&mut items, diff);
            }
            let messages = convert_timeline_items(&items);
            if timeline_tx.send(messages).await.is_err() {
                break;
            }
        }

        Ok(())
    }

    async fn start_sync(&self, state_tx: mpsc::Sender<SyncSnapshot>) -> Result<()> {
        let client = {
            let guard = self.client.lock().await;
            guard
                .as_ref()
                .ok_or_else(|| AppError::Matrix("No client".into()))?
                .clone()
        };

        let stream = client.sync_stream(SyncSettings::default()).await;
        tokio::pin!(stream);

        while let Some(result) = stream.next().await {
            if result.is_ok() {
                let rooms = build_room_list(&client).await;
                if state_tx.send(SyncSnapshot { rooms }).await.is_err() {
                    break;
                }
            }
        }

        Ok(())
    }

    async fn restore_session(&self, session: &Session) -> Result<()> {
        let passphrase = self.get_or_create_passphrase().await?;
        let client = Client::builder()
            .homeserver_url(&session.homeserver)
            .handle_refresh_tokens()
            .sqlite_store(self.data_dir.join("matrix-store"), Some(&passphrase))
            .build()
            .await
            .map_err(|e| AppError::Matrix(e.to_string()))?;

        let user_id: OwnedUserId = session
            .user_id
            .as_str()
            .try_into()
            .map_err(|e: IdParseError| AppError::Matrix(e.to_string()))?;
        let device_id: OwnedDeviceId = session.device_id.as_str().into();
        let meta = SessionMeta { user_id, device_id };
        let tokens = SessionTokens {
            access_token: session.access_token.clone(),
            refresh_token: session.refresh_token.clone(),
        };

        if let Some(client_id) = &session.client_id {
            let oauth_session = OAuthSession {
                client_id: ClientId::new(client_id.clone()),
                user: UserSession { meta, tokens },
            };
            client
                .restore_session(oauth_session)
                .await
                .map_err(|e| AppError::Matrix(e.to_string()))?;
        } else {
            let matrix_session = MatrixSession { meta, tokens };
            client
                .restore_session(matrix_session)
                .await
                .map_err(|e| AppError::Matrix(e.to_string()))?;
        }

        *self.client.lock().await = Some(client);
        Ok(())
    }

    async fn logout(&self) -> Result<()> {
        let mut guard = self.client.lock().await;
        if let Some(client) = guard.as_ref() {
            drop(client.matrix_auth().logout().await);
        }
        *guard = None;
        Ok(())
    }

    async fn send_text(&self, room_id: &RoomId, body: &str) -> Result<()> {
        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| AppError::Matrix("No client".into()))?;

        let room_id_parsed: OwnedRoomId = room_id
            .0
            .as_str()
            .try_into()
            .map_err(|e: IdParseError| AppError::Matrix(e.to_string()))?;

        let room = client
            .get_room(&room_id_parsed)
            .ok_or_else(|| AppError::Matrix("Room not found".into()))?;

        drop(guard);

        room.send(RoomMessageEventContent::text_plain(body))
            .await
            .map_err(|e| AppError::Matrix(e.to_string()))?;

        Ok(())
    }
}
