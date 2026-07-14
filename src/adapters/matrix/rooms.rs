use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use matrix_sdk::deserialized_responses::SyncOrStrippedState;
use matrix_sdk::latest_events::LatestEventValue;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::{Relation, RoomMessageEventContent};
use matrix_sdk::ruma::events::space::child::SpaceChildEventContent;
use matrix_sdk::ruma::events::{
    AnyMessageLikeEventContent, AnySyncMessageLikeEvent, AnySyncTimelineEvent,
    SyncMessageLikeEvent, SyncStateEvent,
};
use matrix_sdk::ruma::{OwnedMxcUri, OwnedUserId, RoomId as MatrixRoomId, UserId};
use matrix_sdk::sync::RoomUpdates;
use matrix_sdk::{Client, HttpError, Room};
use matrix_sdk_base::RoomInfoNotableUpdate;
use matrix_sdk_ui::encryption_sync_service::Error as EncryptionSyncError;
use matrix_sdk_ui::room_list_service::Error as RoomListError;
use matrix_sdk_ui::sync_service::{Error as SyncServiceError, State as SyncState, SyncService};
use tokio::fs;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinSet;

use super::media::{fetch_and_materialize, lookup_materialized, mxc_avatar_key};
use super::preview::{self, MessagePreview};
use crate::domain::models::{
    MessagePreviewKind, Room as DomainRoom, RoomId, Space as DomainSpace, SyncEvent,
};
use crate::error::{AppError, Result as AppResult};
use crate::util::hex_encode_id;

fn room_avatar_mxc(room: &Room, is_direct: bool) -> Option<String> {
    if let Some(mxc) = room.avatar_url() {
        return Some(mxc.to_string());
    }
    if !is_direct {
        return None;
    }
    room.heroes()
        .first()
        .and_then(|hero| hero.avatar_url.as_ref())
        .map(ToString::to_string)
}

async fn build_single_room(room: &Room) -> DomainRoom {
    let display_name = room
        .cached_display_name()
        .map(|dn| dn.to_string())
        .unwrap_or_default();
    let unread = room.num_unread_notifications();
    let mentions = room.num_unread_mentions();
    let is_direct = room.is_direct().await.unwrap_or_default();
    let member_count = room.joined_members_count();
    let last_activity_ts: u64 = room.latest_event_timestamp().map_or(0, |ts| ts.0.into());
    let last_message = build_last_message(room, is_direct).await;
    DomainRoom {
        id: RoomId::new(room.room_id().to_string()),
        display_name,
        avatar_mxc: room_avatar_mxc(room, is_direct),
        is_direct,
        member_count,
        unread_count: unread,
        mention_count: mentions,
        last_activity_ts,
        last_message_sender: last_message.sender,
        last_message_kind: last_message.kind,
        last_message_body: last_message.body,
        last_message_is_own: last_message.is_own,
        last_message_edited: last_message.edited,
    }
}

#[derive(Default)]
struct LastMessage {
    sender: Option<String>,
    kind: MessagePreviewKind,
    body: String,
    is_own: bool,
    edited: bool,
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
        kind: preview.kind,
        body: preview.body,
        is_own,
        edited: preview.edited,
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
                Some((preview_from_message_content(&message), None))
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
        )) => Some(preview_from_message_content(&message.content)),
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomEncrypted(_)) => {
            Some(MessagePreview::labelled(MessagePreviewKind::Encrypted))
        }
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::Sticker(_)) => {
            Some(MessagePreview::labelled(MessagePreviewKind::Sticker))
        }
        _ => None,
    }
}

fn preview_from_message_content(content: &RoomMessageEventContent) -> MessagePreview {
    if let Some(Relation::Replacement(replacement)) = &content.relates_to {
        let mut preview = preview::from_msgtype(&replacement.new_content.msgtype);
        preview.edited = true;
        preview
    } else {
        preview::from_msgtype(&content.msgtype)
    }
}

fn sort_rooms(rooms: &mut [DomainRoom]) {
    rooms.sort_by(|a, b| b.last_activity_ts.cmp(&a.last_activity_ts));
}

fn emit_rooms(
    client: &Client,
    room_cache: &HashMap<String, DomainRoom>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
    avatars: &mut AvatarFetcher,
) {
    let mut rooms: Vec<DomainRoom> = room_cache.values().cloned().collect();
    sort_rooms(&mut rooms);
    avatars.request(
        client,
        AvatarKind::Room,
        avatar_uris(&rooms, |room| room.avatar_mxc.as_deref()),
    );
    on_sync(SyncEvent::Rooms(rooms));
}

async fn emit_spaces(
    client: &Client,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
    avatars: &mut AvatarFetcher,
) {
    let spaces = build_spaces_meta(client).await;
    avatars.request(
        client,
        AvatarKind::Space,
        avatar_uris(&spaces, |space| space.avatar_mxc.as_deref()),
    );
    on_sync(SyncEvent::Spaces(spaces));
}

fn avatar_uris<T>(items: &[T], mxc_of: impl Fn(&T) -> Option<&str>) -> Vec<OwnedMxcUri> {
    items
        .iter()
        .filter_map(|item| mxc_of(item).map(OwnedMxcUri::from))
        .collect()
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

async fn build_spaces_meta(client: &Client) -> Vec<DomainSpace> {
    let mut spaces = Vec::new();
    for space in client.joined_space_rooms() {
        let name = space
            .cached_display_name()
            .map(|dn| dn.to_string())
            .unwrap_or_default();
        let child_room_ids = space_child_ids(&space).await;
        let avatar_mxc = space.avatar_url().map(|mxc| mxc.to_string());
        spaces.push(DomainSpace {
            id: space.room_id().to_string(),
            name,
            avatar_mxc,
            child_room_ids,
            unread: 0,
            mentions: 0,
        });
    }
    spaces
}

#[derive(Clone, Copy)]
enum AvatarKind {
    Room,
    Space,
}

struct AvatarFetcher {
    media_dir: PathBuf,
    materialized: Arc<StdMutex<HashMap<String, PathBuf>>>,
    tasks: JoinSet<()>,
    requested: HashSet<String>,
    ready_tx: UnboundedSender<AvatarKind>,
}

impl AvatarFetcher {
    fn new(
        media_dir: PathBuf,
        materialized: Arc<StdMutex<HashMap<String, PathBuf>>>,
        ready_tx: UnboundedSender<AvatarKind>,
    ) -> Self {
        Self {
            media_dir,
            materialized,
            tasks: JoinSet::new(),
            requested: HashSet::new(),
            ready_tx,
        }
    }

    fn request(&mut self, client: &Client, kind: AvatarKind, uris: Vec<OwnedMxcUri>) {
        for mxc in uris {
            let key = mxc_avatar_key(mxc.as_str());
            if lookup_materialized(&self.materialized, &key).is_some()
                || !self.requested.insert(key.clone())
            {
                continue;
            }
            self.spawn_fetch(client, kind, mxc, key);
        }
    }

    fn spawn_fetch(&mut self, client: &Client, kind: AvatarKind, mxc: OwnedMxcUri, key: String) {
        let client = client.clone();
        let media_dir = self.media_dir.clone();
        let materialized = Arc::clone(&self.materialized);
        let ready_tx = self.ready_tx.clone();
        self.tasks.spawn(async move {
            let avatar_dir = media_dir.join("avatars");
            if let Err(e) = fs::create_dir_all(&avatar_dir).await {
                tracing::warn!("failed to create avatar dir: {e}");
                return;
            }
            let cache_stem = avatar_dir.join(hex_encode_id(mxc.as_str()));
            let source = MediaSource::Plain(mxc);
            if fetch_and_materialize(&client, &materialized, &cache_stem, source, &key)
                .await
                .is_some()
            {
                ready_tx.send(kind).ok();
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

async fn refresh_room(
    client: &Client,
    room_id: &MatrixRoomId,
    room_cache: &mut HashMap<String, DomainRoom>,
) -> bool {
    let key = room_id.as_str();
    if !room_cache.contains_key(key) {
        return false;
    }
    let Some(room) = client.get_room(room_id) else {
        return false;
    };
    if room.is_space() {
        return false;
    }
    let updated = build_single_room(&room).await;
    if room_cache.get(key) == Some(&updated) {
        return false;
    }
    room_cache.insert(key.to_owned(), updated);
    true
}

async fn handle_room_info_update(
    client: &Client,
    update: Result<RoomInfoNotableUpdate, RecvError>,
    room_cache: &mut HashMap<String, DomainRoom>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
    avatars: &mut AvatarFetcher,
) -> LoopAction {
    match update {
        Ok(update) => {
            if refresh_room(client, &update.room_id, room_cache).await {
                tracing::debug!(room = %update.room_id, "refreshed room preview from notable update");
                emit_rooms(client, room_cache, on_sync, avatars);
            }
            LoopAction::Continue
        }
        Err(RecvError::Lagged(n)) => {
            tracing::warn!("room info updates lagged by {n} messages, full rebuild");
            room_cache.clear();
            seed_room_cache(client, room_cache).await;
            emit_rooms(client, room_cache, on_sync, avatars);
            LoopAction::Continue
        }
        Err(RecvError::Closed) => LoopAction::Break,
    }
}

async fn handle_avatar_ready(
    client: &Client,
    first: AvatarKind,
    ready_rx: &mut UnboundedReceiver<AvatarKind>,
    room_cache: &HashMap<String, DomainRoom>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
    avatars: &mut AvatarFetcher,
) {
    let mut rooms_ready = false;
    let mut spaces_ready = false;
    let mut mark = |kind| match kind {
        AvatarKind::Room => rooms_ready = true,
        AvatarKind::Space => spaces_ready = true,
    };

    mark(first);
    while let Ok(kind) = ready_rx.try_recv() {
        mark(kind);
    }

    if rooms_ready {
        emit_rooms(client, room_cache, on_sync, avatars);
    }
    if spaces_ready {
        emit_spaces(client, on_sync, avatars).await;
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
    avatars: &mut AvatarFetcher,
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
            emit_rooms(client, room_cache, on_sync, avatars);
            emit_spaces(client, on_sync, avatars).await;
            LoopAction::Continue
        }
        Err(RecvError::Lagged(n)) => {
            tracing::warn!("room updates lagged by {n} messages, full rebuild");
            room_cache.clear();
            seed_room_cache(client, room_cache).await;
            emit_rooms(client, room_cache, on_sync, avatars);
            emit_spaces(client, on_sync, avatars).await;
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
    avatars: &mut AvatarFetcher,
) -> LoopAction {
    match state {
        SyncState::Running => {
            tracing::info!("sliding sync running");
            seed_room_cache(client, room_cache).await;
            if !room_cache.is_empty() {
                emit_rooms(client, room_cache, on_sync, avatars);
            }
            emit_spaces(client, on_sync, avatars).await;
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
    avatars: &mut AvatarFetcher,
    ready_rx: &mut UnboundedReceiver<AvatarKind>,
) {
    let mut room_cache: HashMap<String, DomainRoom> = HashMap::new();
    let mut state_stream = sync_service.state();
    let mut room_info_rx = client.room_info_notable_update_receiver();

    seed_room_cache(client, &mut room_cache).await;
    if !room_cache.is_empty() {
        emit_rooms(client, &room_cache, on_sync, avatars);
    }
    emit_spaces(client, on_sync, avatars).await;
    on_sync(SyncEvent::Connected);

    loop {
        tokio::select! {
            biased;
            update = room_updates_rx.recv() => {
                if matches!(
                    handle_room_update(client, update, &mut room_cache, on_sync, avatars).await,
                    LoopAction::Break
                ) {
                    break;
                }
            }
            Some(state) = state_stream.next() => {
                if matches!(
                    handle_sync_state(client, state, sync_service, &mut room_cache, on_sync, avatars).await,
                    LoopAction::Break
                ) {
                    break;
                }
            }
            info = room_info_rx.recv() => {
                if matches!(
                    handle_room_info_update(client, info, &mut room_cache, on_sync, avatars).await,
                    LoopAction::Break
                ) {
                    break;
                }
            }
            Some(kind) = ready_rx.recv() => {
                handle_avatar_ready(client, kind, ready_rx, &room_cache, on_sync, avatars).await;
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
    let (ready_tx, mut ready_rx) = unbounded_channel();
    let mut avatars = AvatarFetcher::new(media_dir, materialized, ready_tx);

    sync_service.start().await;
    tracing::info!("sliding sync service started");

    run_sync_loop(
        client,
        &sync_service,
        &mut room_updates_rx,
        &on_sync,
        &mut avatars,
        &mut ready_rx,
    )
    .await;

    sync_service.stop().await;
    Ok(())
}
