use std::collections::HashMap;
use std::fs as fs_std;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use matrix_sdk::authentication::SessionTokens;
use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::authentication::oauth::registration::{
    ApplicationType, ClientMetadata, Localized, OAuthGrantType,
};
use matrix_sdk::authentication::oauth::{ClientId, OAuthSession, UrlOrQuery, UserSession};
use matrix_sdk::config::SyncSettings;
use matrix_sdk::encryption::verification::{
    SasState, SasVerification, Verification, VerificationRequest, VerificationRequestState,
};
use matrix_sdk::media::{MediaFormat, MediaRequestParameters, MediaThumbnailSettings};
use matrix_sdk::ruma::api::client::error::ErrorKind as RumaErrorKind;
use matrix_sdk::ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEvent;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::{
    MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent,
};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{IdParseError, OwnedDeviceId, OwnedMxcUri, OwnedRoomId, OwnedUserId};
use matrix_sdk::utils::local_server::{LocalServerBuilder, LocalServerRedirectHandle};
use matrix_sdk::{Client, SessionChange, SessionMeta};
use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::{EventTimelineItem, RoomExt, TimelineDetails, TimelineItem};
use tokio::fs;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::time::timeout;
use url::Url;

use crate::domain::models::{
    AuthMethod, ConnectionStatus, EventId, FileMeta, ImageMeta, LoginCredentials, MessageBody,
    OAuthLoginData, Room as DomainRoom, RoomId, ServerInfo, Session, SyncSnapshot, TimelineMessage,
    VerificationEmoji, VerificationEvent,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::MatrixPort;

pub struct MatrixAdapter {
    data_dir: PathBuf,
    client: RwLock<Option<Client>>,
    redirect_handle: Mutex<Option<LocalServerRedirectHandle>>,
    verification_request: Mutex<Option<VerificationRequest>>,
    sas_verification: Mutex<Option<SasVerification>>,
    media_sources: Arc<StdMutex<HashMap<String, MediaSource>>>,
}

impl MatrixAdapter {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            client: RwLock::new(None),
            redirect_handle: Mutex::new(None),
            verification_request: Mutex::new(None),
            sas_verification: Mutex::new(None),
            media_sources: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    async fn get_client(&self) -> Result<Client> {
        self.client
            .read()
            .await
            .clone()
            .ok_or_else(|| AppError::Other("No client, run server discovery first".into()))
    }
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
        let last_activity_ts: u64 = room
            .new_latest_event_timestamp()
            .map_or(0, |ts| ts.0.into());
        rooms.push(DomainRoom {
            id: RoomId(room.room_id().to_string()),
            display_name,
            is_direct,
            unread_count: unread,
            last_activity_ts,
        });
    }
    rooms.sort_by(|a, b| {
        b.unread_count
            .min(1)
            .cmp(&a.unread_count.min(1))
            .then(b.last_activity_ts.cmp(&a.last_activity_ts))
    });
    rooms
}

#[allow(clippy::too_many_lines)]
fn convert_event_item(
    event: &EventTimelineItem,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    own_user_id: Option<&str>,
) -> Option<TimelineMessage> {
    let event_id_str = event
        .event_id()
        .map(ToString::to_string)
        .unwrap_or_default();

    let content = event.content();

    let Some(message) = content.as_message() else {
        if content.as_unable_to_decrypt().is_some() {
            let (sender_display_name, sender_avatar_url) = match event.sender_profile() {
                TimelineDetails::Ready(profile) => (
                    profile.display_name.clone(),
                    profile.avatar_url.as_ref().map(ToString::to_string),
                ),
                _ => (None, None),
            };
            let ts: u64 = event.timestamp().0.into();
            let sender_str = event.sender().to_string();
            let is_own = own_user_id.is_some_and(|uid| uid == sender_str);
            return Some(TimelineMessage {
                event_id: EventId(event_id_str),
                sender: sender_str,
                sender_display_name,
                sender_avatar_url,
                sender_avatar_path: None,
                body: MessageBody::Unknown("Unable to decrypt message.".into()),
                timestamp: ts,
                is_own,
            });
        }
        tracing::debug!(
            event_id = event_id_str,
            sender = %event.sender(),
            "skipping non-message event"
        );
        return None;
    };

    let body = match message.msgtype() {
        MessageType::Text(t) => MessageBody::Text(t.body.clone()),
        MessageType::Notice(n) => MessageBody::Notice(n.body.clone()),
        MessageType::Emote(e) => MessageBody::Emote(e.body.clone()),
        MessageType::Image(i) => {
            if let Ok(mut sources) = media_sources.lock() {
                sources.insert(event_id_str.clone(), i.source.clone());
                if let Some(info) = &i.info
                    && let Some(ref thumb_source) = info.thumbnail_source
                {
                    sources.insert(format!("{event_id_str}:thumb"), thumb_source.clone());
                }
            }
            #[allow(clippy::cast_possible_truncation)]
            let (width, height, mimetype) = i.info.as_ref().map_or((None, None, None), |info| {
                let w = info.width.map(|v| {
                    let n: u64 = v.into();
                    n as u32
                });
                let h = info.height.map(|v| {
                    let n: u64 = v.into();
                    n as u32
                });
                (w, h, info.mimetype.clone())
            });
            MessageBody::Image {
                alt_text: i.body.clone(),
                meta: ImageMeta {
                    width,
                    height,
                    mimetype,
                    thumbnail_path: None,
                },
            }
        }
        MessageType::File(f) => {
            if let Ok(mut sources) = media_sources.lock() {
                sources.insert(event_id_str.clone(), f.source.clone());
            }
            let (mimetype, size) = f.info.as_ref().map_or((None, None), |info| {
                (info.mimetype.clone(), info.size.map(Into::into))
            });
            MessageBody::File {
                meta: FileMeta {
                    filename: f.filename.clone().unwrap_or_else(|| f.body.clone()),
                    mimetype,
                    size,
                },
            }
        }
        other => MessageBody::Unknown(other.body().to_string()),
    };

    let (sender_display_name, sender_avatar_url) = match event.sender_profile() {
        TimelineDetails::Ready(profile) => (
            profile.display_name.clone(),
            profile.avatar_url.as_ref().map(ToString::to_string),
        ),
        _ => (None, None),
    };

    let ts: u64 = event.timestamp().0.into();

    let sender_str = event.sender().to_string();
    let is_own = own_user_id.is_some_and(|uid| uid == sender_str);

    Some(TimelineMessage {
        event_id: EventId(event_id_str),
        sender: sender_str,
        sender_display_name,
        sender_avatar_url,
        sender_avatar_path: None,
        body,
        timestamp: ts,
        is_own,
    })
}

fn convert_timeline_items(
    items: &[Arc<TimelineItem>],
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    own_user_id: Option<&str>,
) -> Vec<TimelineMessage> {
    items
        .iter()
        .filter_map(|item| convert_event_item(item.as_event()?, media_sources, own_user_id))
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

fn lookup_media_source(
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    event_id: &str,
) -> Option<MediaSource> {
    let thumb_key = format!("{event_id}:thumb");
    media_sources.lock().ok().and_then(|sources| {
        sources
            .get(&thumb_key)
            .or_else(|| sources.get(event_id))
            .cloned()
    })
}

fn ext_from_magic(data: &[u8]) -> &'static str {
    infer::get(data).map_or("png", |t| t.extension())
}

fn find_cached(stem: &Path) -> Option<PathBuf> {
    let parent = stem.parent()?;
    let file_stem = stem.file_name()?;
    fs_std::read_dir(parent).ok()?.find_map(|entry| {
        let path = entry.ok()?.path();
        (path.file_stem() == Some(file_stem)).then_some(path)
    })
}

async fn fetch_single_thumbnail(
    client: &Client,
    cache_stem: &Path,
    source: MediaSource,
    event_id: &str,
) -> Option<PathBuf> {
    let format = MediaFormat::Thumbnail(MediaThumbnailSettings::new(400u32.into(), 400u32.into()));
    let request = MediaRequestParameters { source, format };

    let media = client.media();
    let download = media.get_media_content(&request, true);
    let data = match timeout(Duration::from_secs(5), download).await {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => {
            tracing::debug!("thumbnail download failed for {event_id}: {e}");
            return None;
        }
        Err(_) => {
            tracing::debug!("thumbnail download timed out for {event_id}");
            return None;
        }
    };

    let cache_path = cache_stem.with_extension(ext_from_magic(&data));

    if let Err(e) = fs::write(&cache_path, &data).await {
        tracing::warn!("failed to cache thumbnail: {e}");
        return None;
    }
    Some(cache_path)
}

async fn download_thumbnails(
    client: &Client,
    cache_dir: &Path,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    messages: &mut [TimelineMessage],
) {
    if let Err(e) = fs::create_dir_all(cache_dir).await {
        tracing::warn!("failed to create media cache dir: {e}");
        return;
    }

    for msg in messages.iter_mut() {
        let MessageBody::Image { meta, .. } = &mut msg.body else {
            continue;
        };
        let event_id = &msg.event_id.0;
        let sanitized = event_id.replace(':', "_");
        let cache_stem = cache_dir.join(&sanitized);

        let cached = find_cached(&cache_stem);
        if let Some(path) = cached {
            meta.thumbnail_path = Some(path);
            continue;
        }

        let Some(source) = lookup_media_source(media_sources, event_id) else {
            continue;
        };

        if let Some(path) = fetch_single_thumbnail(client, &cache_stem, source, event_id).await {
            meta.thumbnail_path = Some(path);
        }
    }
}

async fn download_avatars(client: &Client, cache_dir: &Path, messages: &mut [TimelineMessage]) {
    let avatar_dir = cache_dir.join("avatars");
    if let Err(e) = fs::create_dir_all(&avatar_dir).await {
        tracing::warn!("failed to create avatar cache dir: {e}");
        return;
    }

    for msg in messages.iter_mut() {
        let Some(mxc_url) = &msg.sender_avatar_url else {
            continue;
        };

        let sanitized = msg.sender.replace(':', "_").replace('@', "");
        let cache_stem = avatar_dir.join(&sanitized);

        let cached = find_cached(&cache_stem);
        if let Some(path) = cached {
            msg.sender_avatar_path = Some(path);
            continue;
        }

        let avatar_mxc: OwnedMxcUri = mxc_url.as_str().into();
        let source = MediaSource::Plain(avatar_mxc);

        if let Some(path) = fetch_single_thumbnail(client, &cache_stem, source, &msg.sender).await {
            msg.sender_avatar_path = Some(path);
        }
    }
}

fn is_auth_error(err: &matrix_sdk::Error) -> bool {
    if matches!(
        err.client_api_error_kind(),
        Some(
            RumaErrorKind::Unauthorized
                | RumaErrorKind::Forbidden { .. }
                | RumaErrorKind::UnknownToken { .. }
        )
    ) {
        return true;
    }

    matches!(
        err,
        matrix_sdk::Error::Http(http_err) if matches!(http_err.as_ref(), matrix_sdk::HttpError::RefreshToken(_))
    )
}

async fn handle_verification_request(
    request: VerificationRequest,
    sas_mutex: &Mutex<Option<SasVerification>>,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) {
    let mut stream = request.changes();

    while let Some(state) = stream.next().await {
        match state {
            VerificationRequestState::Transitioned { verification } => {
                if let Verification::SasV1(sas) = verification {
                    *sas_mutex.lock().await = Some(sas.clone());
                    handle_sas_verification(sas, tx).await;
                }
                break;
            }
            VerificationRequestState::Done => {
                tx.send(VerificationEvent::Done).ok();
                break;
            }
            VerificationRequestState::Cancelled(info) => {
                tx.send(VerificationEvent::Cancelled(info.reason().to_string()))
                    .ok();
                break;
            }
            _ => {}
        }
    }
}

async fn handle_sas_verification(
    sas: SasVerification,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) {
    if let Err(e) = sas.accept().await {
        tx.send(VerificationEvent::Cancelled(format!(
            "Failed to accept SAS: {e}"
        )))
        .ok();
        return;
    }

    let mut stream = sas.changes();

    while let Some(state) = stream.next().await {
        match state {
            SasState::KeysExchanged { .. } => {
                if let Some(emojis) = sas.emoji() {
                    let domain_emojis: Vec<VerificationEmoji> = emojis
                        .iter()
                        .map(|e| VerificationEmoji {
                            symbol: e.symbol.to_string(),
                            description: e.description.to_string(),
                        })
                        .collect();
                    tx.send(VerificationEvent::Emojis(domain_emojis)).ok();
                }
            }
            SasState::Confirmed => {
                tx.send(VerificationEvent::Confirming).ok();
            }
            SasState::Done { .. } => {
                tx.send(VerificationEvent::Done).ok();
                break;
            }
            SasState::Cancelled(info) => {
                tx.send(VerificationEvent::Cancelled(info.reason().to_string()))
                    .ok();
                break;
            }
            _ => {}
        }
    }
}

fn extract_current_session(client: &Client) -> Option<Session> {
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

fn client_metadata() -> Result<Raw<ClientMetadata>> {
    let ipv4_uri: Url = format!("http://{}/", Ipv4Addr::LOCALHOST)
        .parse()
        .map_err(|e: url::ParseError| AppError::Other(e.to_string()))?;
    let ipv6_uri: Url = format!("http://[{}]/", Ipv6Addr::LOCALHOST)
        .parse()
        .map_err(|e: url::ParseError| AppError::Other(e.to_string()))?;
    let client_uri: Url = "https://github.com/drendog/U2DM"
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

#[async_trait]
impl MatrixPort for MatrixAdapter {
    async fn discover_auth(&self, homeserver: &str, passphrase: &str) -> Result<ServerInfo> {
        let client = Client::builder()
            .server_name_or_homeserver_url(homeserver)
            .handle_refresh_tokens()
            .sqlite_store(self.data_dir.join("matrix-store"), Some(passphrase))
            .build()
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
        *self.client.write().await = Some(client);

        Ok(ServerInfo {
            auth_methods: methods,
            homeserver_url,
        })
    }

    async fn login_password(&self, creds: LoginCredentials) -> Result<Session> {
        let client = self.get_client().await?;

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
        let client = self.get_client().await?;

        let (redirect_uri, server_handle) = LocalServerBuilder::new().spawn().await?;

        let metadata = client_metadata()?;
        let auth_data = client
            .oauth()
            .login(redirect_uri, None, Some(metadata.into()), None)
            .build()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;

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
            .ok_or_else(|| AppError::Other("No pending OAuth login".into()))?;

        let query_string = handle
            .await
            .ok_or_else(|| AppError::Other("No callback received from browser".into()))?;

        let client = self.get_client().await?;

        client
            .oauth()
            .finish_login(UrlOrQuery::Query(query_string.0))
            .await?;

        let sdk_session = client
            .oauth()
            .full_session()
            .ok_or_else(|| AppError::Other("No session after OAuth login".into()))?;
        let homeserver = client.homeserver().to_string();

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
        let client = self.get_client().await?;
        client
            .event_cache()
            .subscribe()
            .map_err(|e| AppError::Other(e.to_string()))?;
        if let Err(e) = client.sync_once(SyncSettings::default()).await {
            if is_auth_error(&e) {
                return Err(AppError::SessionExpired);
            }
            return Err(e.into());
        }
        Ok(build_room_list(&client).await)
    }

    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        timeline_tx: mpsc::UnboundedSender<Vec<TimelineMessage>>,
    ) -> Result<()> {
        let client = self.get_client().await?;

        let room_id_parsed: OwnedRoomId = room_id
            .0
            .as_str()
            .try_into()
            .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;

        let room = client
            .get_room(&room_id_parsed)
            .ok_or_else(|| AppError::Other("Room not found".into()))?;

        let timeline = room
            .timeline()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;

        if let Err(e) = timeline.paginate_backwards(50).await {
            tracing::warn!("failed to paginate timeline backwards: {e}");
        }

        let (initial_items, mut stream) = timeline.subscribe().await;

        let media_sources = Arc::clone(&self.media_sources);
        let cache_dir = self.data_dir.join("media-cache");
        let own_user_id = client.user_id().map(ToString::to_string);

        let mut items: Vec<Arc<TimelineItem>> = initial_items.into_iter().collect();

        let mut messages = convert_timeline_items(&items, &media_sources, own_user_id.as_deref());
        download_thumbnails(&client, &cache_dir, &media_sources, &mut messages).await;
        download_avatars(&client, &cache_dir, &mut messages).await;
        if timeline_tx.send(messages).is_err() {
            return Ok(());
        }

        let backup_client = client.clone();
        let backup_room_id = room_id_parsed.clone();
        tokio::spawn(async move {
            if let Err(e) = backup_client
                .encryption()
                .backups()
                .download_room_keys_for_room(&backup_room_id)
                .await
            {
                tracing::debug!("backup key download for {backup_room_id}: {e}");
            }
        });

        while let Some(diffs) = stream.next().await {
            for diff in diffs {
                apply_timeline_diff(&mut items, diff);
            }
            let mut messages =
                convert_timeline_items(&items, &media_sources, own_user_id.as_deref());
            download_thumbnails(&client, &cache_dir, &media_sources, &mut messages).await;
            download_avatars(&client, &cache_dir, &mut messages).await;
            if timeline_tx.send(messages).is_err() {
                break;
            }
        }

        Ok(())
    }

    async fn start_sync(&self, state_tx: mpsc::UnboundedSender<SyncSnapshot>) -> Result<()> {
        let client = self.get_client().await?;

        let stream = client.sync_stream(SyncSettings::default()).await;
        tokio::pin!(stream);

        while let Some(result) = stream.next().await {
            let snapshot = match result {
                Ok(_) => SyncSnapshot {
                    rooms: build_room_list(&client).await,
                    connection_status: ConnectionStatus::Connected,
                },
                Err(e) => {
                    if is_auth_error(&e) {
                        tracing::warn!("unrecoverable auth error in sync loop, stopping");
                        return Err(AppError::SessionExpired);
                    }
                    SyncSnapshot {
                        rooms: Vec::new(),
                        connection_status: ConnectionStatus::Error(e.to_string()),
                    }
                }
            };
            if state_tx.send(snapshot).is_err() {
                break;
            }
        }

        Ok(())
    }

    async fn restore_session(&self, session: &Session, passphrase: &str) -> Result<()> {
        let client = Client::builder()
            .homeserver_url(&session.homeserver)
            .handle_refresh_tokens()
            .sqlite_store(self.data_dir.join("matrix-store"), Some(passphrase))
            .build()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;

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

        *self.client.write().await = Some(client);
        Ok(())
    }

    async fn logout(&self) -> Result<()> {
        let mut guard = self.client.write().await;
        if let Some(client) = guard.as_ref()
            && let Err(e) = client.matrix_auth().logout().await
        {
            tracing::warn!("failed to logout from server: {e}");
        }
        *guard = None;
        Ok(())
    }

    async fn clear_store(&self) -> Result<()> {
        *self.client.write().await = None;
        let store_path = self.data_dir.join("matrix-store");
        if store_path.exists() {
            fs::remove_dir_all(&store_path).await?;
        }
        Ok(())
    }

    async fn listen_for_verification(
        &self,
        verification_tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()> {
        let client = self.get_client().await?;
        let (req_tx, mut req_rx) = mpsc::unbounded_channel::<VerificationRequest>();

        client.add_event_handler({
            let req_tx = req_tx.clone();
            move |ev: ToDeviceKeyVerificationRequestEvent, client: Client| {
                let req_tx = req_tx.clone();
                async move {
                    if let Some(request) = client
                        .encryption()
                        .get_verification_request(&ev.sender, &ev.content.transaction_id)
                        .await
                    {
                        req_tx.send(request).ok();
                    }
                }
            }
        });

        client.add_event_handler({
            let req_tx = req_tx.clone();
            move |ev: OriginalSyncRoomMessageEvent, client: Client| {
                let req_tx = req_tx.clone();
                async move {
                    if let MessageType::VerificationRequest(_) = &ev.content.msgtype
                        && let Some(request) = client
                            .encryption()
                            .get_verification_request(&ev.sender, &ev.event_id)
                            .await
                    {
                        req_tx.send(request).ok();
                    }
                }
            }
        });

        while let Some(request) = req_rx.recv().await {
            *self.verification_request.lock().await = Some(request.clone());

            verification_tx
                .send(VerificationEvent::Requested {
                    sender: request.other_user_id().to_string(),
                    is_self: request.is_self_verification(),
                })
                .ok();

            handle_verification_request(request, &self.sas_verification, &verification_tx).await;

            *self.verification_request.lock().await = None;
            *self.sas_verification.lock().await = None;
        }

        Ok(())
    }

    async fn accept_verification(&self) -> Result<()> {
        let guard = self.verification_request.lock().await;
        let request = guard
            .as_ref()
            .ok_or_else(|| AppError::Other("No pending verification request".into()))?;
        request.accept().await?;
        Ok(())
    }

    async fn confirm_verification(&self) -> Result<()> {
        let guard = self.sas_verification.lock().await;
        let sas = guard
            .as_ref()
            .ok_or_else(|| AppError::Other("No active SAS verification".into()))?;
        sas.confirm().await?;
        Ok(())
    }

    async fn reject_verification(&self) -> Result<()> {
        if let Some(sas) = self.sas_verification.lock().await.take() {
            sas.mismatch().await?;
        } else if let Some(request) = self.verification_request.lock().await.take() {
            request.cancel().await?;
        }
        Ok(())
    }

    async fn send_text(&self, room_id: &RoomId, body: &str) -> Result<()> {
        let client = self.get_client().await?;

        let room_id_parsed: OwnedRoomId = room_id
            .0
            .as_str()
            .try_into()
            .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;

        let room = client
            .get_room(&room_id_parsed)
            .ok_or_else(|| AppError::Other("Room not found".into()))?;

        room.send(RoomMessageEventContent::text_plain(body)).await?;

        Ok(())
    }

    async fn download_media(&self, event_id: &str, thumbnail: bool) -> Result<Vec<u8>> {
        let client = self.get_client().await?;

        let key = if thumbnail {
            format!("{event_id}:thumb")
        } else {
            event_id.to_string()
        };

        let source = self
            .media_sources
            .lock()
            .map_err(|e| AppError::Other(format!("media source lock poisoned: {e}")))?
            .get(&key)
            .cloned()
            .or_else(|| {
                if thumbnail {
                    self.media_sources.lock().ok()?.get(event_id).cloned()
                } else {
                    None
                }
            })
            .ok_or_else(|| AppError::Other(format!("no media source for event {event_id}")))?;

        let format = if thumbnail {
            MediaFormat::Thumbnail(MediaThumbnailSettings::new(400u32.into(), 400u32.into()))
        } else {
            MediaFormat::File
        };

        let request = MediaRequestParameters { source, format };
        let data = client
            .media()
            .get_media_content(&request, true)
            .await
            .map_err(|e| AppError::Other(format!("media download failed: {e}")))?;

        Ok(data)
    }

    async fn subscribe_session_changes(
        &self,
        session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()> {
        let client = self.get_client().await?;
        let mut rx = client.subscribe_to_session_changes();

        while let Ok(change) = rx.recv().await {
            if change == SessionChange::TokensRefreshed
                && let Some(session) = extract_current_session(&client)
                && session_tx.send(session).is_err()
            {
                break;
            }
        }

        Ok(())
    }
}
