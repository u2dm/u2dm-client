use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use matrix_sdk::deserialized_responses::SyncOrStrippedState;
use matrix_sdk::latest_events::LatestEventValue;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk::ruma::events::space::child::SpaceChildEventContent;
use matrix_sdk::ruma::events::{
    AnyMessageLikeEventContent, AnySyncMessageLikeEvent, AnySyncTimelineEvent,
    SyncMessageLikeEvent, SyncStateEvent,
};
use matrix_sdk::ruma::{OwnedMxcUri, OwnedUserId, UserId};
use matrix_sdk::sync::RoomUpdates;
use matrix_sdk::{Client, HttpError, Room};
use matrix_sdk_ui::encryption_sync_service::Error as EncryptionSyncError;
use matrix_sdk_ui::room_list_service::Error as RoomListError;
use matrix_sdk_ui::sync_service::{Error as SyncServiceError, State as SyncState, SyncService};
use tokio::fs;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinSet;

use super::media::{fetch_and_materialize, lookup_materialized};
use crate::domain::models::{Room as DomainRoom, RoomId, Space as DomainSpace, SyncEvent};
use crate::error::{AppError, Result as AppResult};
use crate::util::hex_encode_id;

async fn build_single_room(room: &Room) -> DomainRoom {
    let display_name = room
        .cached_display_name()
        .map(|dn| dn.to_string())
        .unwrap_or_default();
    let unread = room.num_unread_notifications();
    let mentions = room.num_unread_mentions();
    let is_direct = room.is_direct().await.unwrap_or_default();
    let last_activity_ts: u64 = room.latest_event_timestamp().map_or(0, |ts| ts.0.into());
    let last_message = build_last_message(room, is_direct).await;
    DomainRoom {
        id: RoomId::new(room.room_id().to_string()),
        display_name,
        is_direct,
        unread_count: unread,
        mention_count: mentions,
        last_activity_ts,
        last_message_sender: last_message.sender,
        last_message_kind: last_message.kind,
        last_message_body: last_message.body,
        last_message_is_own: last_message.is_own,
    }
}

#[derive(Default)]
struct LastMessage {
    sender: Option<String>,
    kind: String,
    body: String,
    is_own: bool,
}

async fn build_last_message(room: &Room, is_direct: bool) -> LastMessage {
    let Some((preview, sender_id)) = latest_message_preview(&room.latest_event()) else {
        return LastMessage::default();
    };

    let is_own = sender_id
        .as_ref()
        .is_none_or(|sender| sender == room.own_user_id());

    let sender = if is_own || is_direct {
        None
    } else {
        match &sender_id {
            Some(sender) => Some(resolve_sender_name(room, sender).await),
            None => None,
        }
    };

    LastMessage {
        sender,
        kind: preview.kind.to_owned(),
        body: preview.body,
        is_own,
    }
}

async fn resolve_sender_name(room: &Room, user_id: &UserId) -> String {
    if let Ok(Some(member)) = room.get_member_no_sync(user_id).await
        && let Some(name) = member.display_name()
    {
        return name.to_owned();
    }
    user_id.localpart().to_owned()
}

struct MessagePreview {
    kind: &'static str,
    body: String,
}

impl MessagePreview {
    fn labelled(kind: &'static str) -> Self {
        Self {
            kind,
            body: String::new(),
        }
    }
}

fn latest_message_preview(
    value: &LatestEventValue,
) -> Option<(MessagePreview, Option<OwnedUserId>)> {
    match value {
        LatestEventValue::Remote(event) => {
            let preview = preview_from_event(&event.raw().deserialize().ok()?)?;
            Some((preview, event.sender()))
        }
        LatestEventValue::LocalIsSending(local)
        | LatestEventValue::LocalHasBeenSent { value: local, .. }
        | LatestEventValue::LocalCannotBeSent(local) => match local.content.deserialize().ok()? {
            AnyMessageLikeEventContent::RoomMessage(message) => {
                Some((message_preview(&message.msgtype), None))
            }
            _ => None,
        },
        LatestEventValue::None | LatestEventValue::RemoteInvite { .. } => None,
    }
}

fn preview_from_event(event: &AnySyncTimelineEvent) -> Option<MessagePreview> {
    match event {
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
            SyncMessageLikeEvent::Original(message),
        )) => Some(message_preview(&message.content.msgtype)),
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomEncrypted(_)) => {
            Some(MessagePreview::labelled("encrypted"))
        }
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::Sticker(_)) => {
            Some(MessagePreview::labelled("sticker"))
        }
        _ => None,
    }
}

fn message_preview(msgtype: &MessageType) -> MessagePreview {
    let (kind, body) = match msgtype {
        MessageType::Text(content) => ("text", content.body.as_str()),
        MessageType::Notice(content) => ("text", content.body.as_str()),
        MessageType::Emote(content) => ("text", content.body.as_str()),
        MessageType::Image(_) => return MessagePreview::labelled("image"),
        MessageType::Video(_) => return MessagePreview::labelled("video"),
        MessageType::Audio(_) => return MessagePreview::labelled("audio"),
        MessageType::File(content) => {
            ("file", content.filename.as_deref().unwrap_or(&content.body))
        }
        MessageType::Location(_) => return MessagePreview::labelled("location"),
        other => ("text", other.body()),
    };
    MessagePreview {
        kind,
        body: body.split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

fn sort_rooms(rooms: &mut [DomainRoom]) {
    rooms.sort_by(|a, b| {
        b.unread_count
            .min(1)
            .cmp(&a.unread_count.min(1))
            .then(b.last_activity_ts.cmp(&a.last_activity_ts))
    });
}

fn emit_room_update(
    room_cache: &HashMap<String, DomainRoom>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
) {
    let mut rooms: Vec<DomainRoom> = room_cache.values().cloned().collect();
    sort_rooms(&mut rooms);
    on_sync(SyncEvent::Rooms(rooms));
}

fn space_avatar_key(mxc: &OwnedMxcUri) -> String {
    format!("space-avatar:{mxc}")
}

async fn space_child_ids(space: &Room) -> Vec<String> {
    let events = match space
        .get_state_events_static::<SpaceChildEventContent>()
        .await
    {
        Ok(events) => events,
        Err(e) => {
            tracing::debug!(space = %space.room_id(), "failed to read space children: {e}");
            return Vec::new();
        }
    };
    events
        .into_iter()
        .filter_map(|raw| match raw.deserialize() {
            Ok(SyncOrStrippedState::Sync(SyncStateEvent::Original(event))) => {
                (!event.content.via.is_empty()).then(|| event.state_key.to_string())
            }
            _ => None,
        })
        .collect()
}

async fn build_spaces_meta(
    client: &Client,
    materialized: &StdMutex<HashMap<String, PathBuf>>,
) -> Vec<DomainSpace> {
    let mut spaces = Vec::new();
    for space in client.joined_space_rooms() {
        let name = space
            .cached_display_name()
            .map(|dn| dn.to_string())
            .unwrap_or_default();
        let child_room_ids = space_child_ids(&space).await;
        let avatar_path = space
            .avatar_url()
            .and_then(|mxc| lookup_materialized(materialized, &space_avatar_key(&mxc)));
        spaces.push(DomainSpace {
            id: space.room_id().to_string(),
            name,
            avatar_path,
            child_room_ids,
            unread: 0,
            mentions: 0,
        });
    }
    spaces
}

struct SpaceBuilder {
    media_dir: PathBuf,
    materialized: Arc<StdMutex<HashMap<String, PathBuf>>>,
    avatar_tasks: JoinSet<()>,
    avatars_fetched: HashSet<String>,
}

impl SpaceBuilder {
    fn new(media_dir: PathBuf, materialized: Arc<StdMutex<HashMap<String, PathBuf>>>) -> Self {
        Self {
            media_dir,
            materialized,
            avatar_tasks: JoinSet::new(),
            avatars_fetched: HashSet::new(),
        }
    }

    async fn emit(&mut self, client: &Client, on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>) {
        let spaces = build_spaces_meta(client, &self.materialized).await;
        for space in client.joined_space_rooms() {
            if let Some(mxc) = space.avatar_url()
                && lookup_materialized(&self.materialized, &space_avatar_key(&mxc)).is_none()
            {
                self.spawn_avatar_fetch(client, mxc, on_sync);
            }
        }
        on_sync(SyncEvent::Spaces(spaces));
    }

    fn spawn_avatar_fetch(
        &mut self,
        client: &Client,
        mxc: OwnedMxcUri,
        on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
    ) {
        let key = space_avatar_key(&mxc);
        if !self.avatars_fetched.insert(key.clone()) {
            return;
        }
        let client = client.clone();
        let media_dir = self.media_dir.clone();
        let materialized = Arc::clone(&self.materialized);
        let on_sync = Arc::clone(on_sync);
        self.avatar_tasks.spawn(async move {
            let avatar_dir = media_dir.join("avatars");
            if let Err(e) = fs::create_dir_all(&avatar_dir).await {
                tracing::warn!("failed to create space avatar dir: {e}");
                return;
            }
            let cache_stem = avatar_dir.join(hex_encode_id(mxc.as_str()));
            let source = MediaSource::Plain(mxc);
            if fetch_and_materialize(&client, &materialized, &cache_stem, source, &key)
                .await
                .is_some()
            {
                on_sync(SyncEvent::Spaces(
                    build_spaces_meta(&client, &materialized).await,
                ));
            }
        });
    }
}

async fn build_sync_service(client: &Client) -> AppResult<SyncService> {
    client
        .event_cache()
        .subscribe()
        .map_err(|e| AppError::Other(e.to_string()))?;

    SyncService::builder(client.clone())
        .build()
        .await
        .map_err(|e| AppError::Other(e.to_string()))
}

async fn seed_room_cache(client: &Client, room_cache: &mut HashMap<String, DomainRoom>) {
    for room in client.joined_rooms() {
        if room.is_space() {
            continue;
        }
        let dr = build_single_room(&room).await;
        room_cache.insert(dr.id.to_string(), dr);
    }
}

async fn apply_room_updates(
    client: &Client,
    updates: &RoomUpdates,
    room_cache: &mut HashMap<String, DomainRoom>,
) {
    for room_id in updates.left.keys() {
        let key = room_id.to_string();
        room_cache.remove(&key);
    }
    for room_id in updates.joined.keys() {
        if let Some(room) = client.get_room(room_id)
            && !room.is_space()
        {
            let dr = build_single_room(&room).await;
            room_cache.insert(dr.id.to_string(), dr);
        }
    }
}

fn extract_sdk_error(err: &SyncServiceError) -> Option<&matrix_sdk::Error> {
    match err {
        SyncServiceError::RoomList(RoomListError::SlidingSync(e))
        | SyncServiceError::EncryptionSync(EncryptionSyncError::SlidingSync(e)) => Some(e),
        _ => None,
    }
}

fn is_refresh_token_error(err: &matrix_sdk::Error) -> bool {
    matches!(
        err,
        matrix_sdk::Error::Http(http) if matches!(http.as_ref(), HttpError::RefreshToken(_))
    )
}

fn is_auth_error(err: &SyncServiceError) -> bool {
    extract_sdk_error(err).is_some_and(|e| {
        if matches!(
            e.client_api_error_kind(),
            Some(ErrorKind::UnknownToken { .. } | ErrorKind::Unauthorized | ErrorKind::Forbidden)
        ) {
            return true;
        }
        is_refresh_token_error(e)
    })
}

enum LoopAction {
    Continue,
    Break,
}

async fn handle_room_update(
    client: &Client,
    update: Result<RoomUpdates, RecvError>,
    room_cache: &mut HashMap<String, DomainRoom>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
    space_builder: &mut SpaceBuilder,
) -> LoopAction {
    match update {
        Ok(updates) => {
            if updates.joined.is_empty() && updates.left.is_empty() {
                return LoopAction::Continue;
            }
            tracing::debug!(
                joined = updates.joined.len(),
                left = updates.left.len(),
                "processing room updates"
            );
            apply_room_updates(client, &updates, room_cache).await;
            emit_room_update(room_cache, on_sync);
            space_builder.emit(client, on_sync).await;
            LoopAction::Continue
        }
        Err(RecvError::Lagged(n)) => {
            tracing::warn!("room updates lagged by {n} messages, full rebuild");
            room_cache.clear();
            seed_room_cache(client, room_cache).await;
            emit_room_update(room_cache, on_sync);
            space_builder.emit(client, on_sync).await;
            LoopAction::Continue
        }
        Err(RecvError::Closed) => LoopAction::Break,
    }
}

#[allow(clippy::cognitive_complexity)]
async fn handle_sync_state(
    client: &Client,
    state: SyncState,
    sync_service: &SyncService,
    room_cache: &mut HashMap<String, DomainRoom>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
    space_builder: &mut SpaceBuilder,
) -> LoopAction {
    match state {
        SyncState::Running => {
            tracing::info!("sliding sync running");
            seed_room_cache(client, room_cache).await;
            if !room_cache.is_empty() {
                emit_room_update(room_cache, on_sync);
            }
            space_builder.emit(client, on_sync).await;
            on_sync(SyncEvent::Connected);
            LoopAction::Continue
        }
        SyncState::Error(err) => {
            let msg = err.to_string();
            tracing::warn!("sliding sync error: {msg}");
            if is_auth_error(&err) {
                on_sync(SyncEvent::SessionExpired);
                return LoopAction::Break;
            }
            on_sync(SyncEvent::ConnectionError(msg));
            sync_service.start().await;
            LoopAction::Continue
        }
        SyncState::Terminated => {
            tracing::info!("sliding sync terminated");
            LoopAction::Break
        }
        SyncState::Idle | SyncState::Offline => LoopAction::Continue,
    }
}

async fn run_sync_loop(
    client: &Client,
    sync_service: &SyncService,
    room_updates_rx: &mut Receiver<RoomUpdates>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
    space_builder: &mut SpaceBuilder,
) {
    let mut room_cache: HashMap<String, DomainRoom> = HashMap::new();
    let mut state_stream = sync_service.state();

    seed_room_cache(client, &mut room_cache).await;
    if !room_cache.is_empty() {
        emit_room_update(&room_cache, on_sync);
    }
    space_builder.emit(client, on_sync).await;
    on_sync(SyncEvent::Connected);

    loop {
        tokio::select! {
            biased;
            update = room_updates_rx.recv() => {
                if matches!(
                    handle_room_update(client, update, &mut room_cache, on_sync, space_builder).await,
                    LoopAction::Break
                ) {
                    break;
                }
            }
            Some(state) = state_stream.next() => {
                if matches!(
                    handle_sync_state(client, state, sync_service, &mut room_cache, on_sync, space_builder).await,
                    LoopAction::Break
                ) {
                    break;
                }
            }
        }
    }
}

pub(super) async fn start_sync(
    client: &Client,
    media_dir: PathBuf,
    materialized: Arc<StdMutex<HashMap<String, PathBuf>>>,
    on_sync: Arc<dyn Fn(SyncEvent) + Send + Sync>,
) -> AppResult<()> {
    let sync_service = build_sync_service(client).await?;
    let mut room_updates_rx = client.subscribe_to_all_room_updates();
    let mut space_builder = SpaceBuilder::new(media_dir, materialized);

    sync_service.start().await;
    tracing::info!("sliding sync service started");

    run_sync_loop(
        client,
        &sync_service,
        &mut room_updates_rx,
        &on_sync,
        &mut space_builder,
    )
    .await;

    sync_service.stop().await;
    Ok(())
}
