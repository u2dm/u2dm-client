use std::collections::{HashMap, HashSet, VecDeque};
use std::future;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream};
use matrix_sdk::deserialized_responses::SyncOrStrippedState;
use matrix_sdk::latest_events::LatestEventValue;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::room::member::MembershipState;
use matrix_sdk::ruma::events::room::message::{Relation, RoomMessageEventContent};
use matrix_sdk::ruma::events::space::child::SpaceChildEventContent;
use matrix_sdk::ruma::events::space_order::SpaceOrderEventContent;
use matrix_sdk::ruma::events::{
    AnyMessageLikeEventContent, AnySyncMessageLikeEvent, AnySyncStateEvent, AnySyncTimelineEvent,
    SyncMessageLikeEvent, SyncStateEvent,
};
use matrix_sdk::ruma::{OwnedMxcUri, OwnedUserId, RoomId as MatrixRoomId, UserId};
use matrix_sdk::sync::RoomUpdates;
use matrix_sdk::{Client, HttpError, Room};
use matrix_sdk_base::RoomInfoNotableUpdate;
use matrix_sdk_ui::encryption_sync_service::Error as EncryptionSyncError;
use matrix_sdk_ui::room_list_service::Error as RoomListError;
use matrix_sdk_ui::sync_service::{Error as SyncServiceError, State as SyncState, SyncService};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::{JoinError, JoinSet};
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;

use super::media::{MediaService, mxc_avatar_key};
use super::preview::{self, MessagePreview};
use crate::domain::models::{
    MessagePreviewKind, Room as DomainRoom, RoomId, ServiceEvent, Space as DomainSpace, SyncEvent,
    SyncOutcome,
};
use crate::error::{AppError, Result as AppResult};
use crate::ports::matrix::SyncSink as OnSync;

const EMIT_DEBOUNCE: Duration = Duration::from_millis(50);
const AVATAR_INFLIGHT: usize = 8;
const AVATAR_RETRY_COOLDOWN: Duration = Duration::from_mins(1);
const SEED_INFLIGHT: usize = 16;

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
        last_message_service: last_message.service,
        last_message_is_own: last_message.is_own,
        last_message_edited: last_message.edited,
    }
}

#[derive(Default)]
struct LastMessage {
    sender: Option<String>,
    kind: MessagePreviewKind,
    body: String,
    service: Option<ServiceEvent>,
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

    let is_service = preview.service.is_some();
    let sender = if is_own || (is_direct && !is_service) {
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
        service: preview.service,
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
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::CallInvite(_)) => {
            Some(MessagePreview::service(ServiceEvent::CallStarted))
        }
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RtcNotification(_)) => {
            Some(MessagePreview::service(ServiceEvent::CallNotification))
        }
        AnySyncTimelineEvent::State(AnySyncStateEvent::RoomMember(member))
            if matches!(member.membership(), MembershipState::Knock) =>
        {
            Some(MessagePreview::service(ServiceEvent::Knocked))
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

async fn build_rooms(client: &Client) -> HashMap<String, DomainRoom> {
    let joined = client
        .joined_rooms()
        .into_iter()
        .filter(|room| !room.is_space());
    stream::iter(joined)
        .map(|room| async move { build_single_room(&room).await })
        .buffer_unordered(SEED_INFLIGHT)
        .map(|room| (room.id.to_string(), room))
        .collect()
        .await
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

async fn space_order(space: &Room) -> Option<String> {
    let raw = space
        .account_data_static::<SpaceOrderEventContent>()
        .await
        .ok()??;
    let event = raw.deserialize().ok()?;
    Some(event.content.order.to_string())
}

async fn build_spaces_meta(client: &Client) -> Vec<DomainSpace> {
    let joined_spaces = client.joined_space_rooms();
    let space_ids: HashSet<String> = joined_spaces
        .iter()
        .map(|space| space.room_id().to_string())
        .collect();

    let space_ids = &space_ids;
    stream::iter(joined_spaces)
        .map(|space| async move {
            let name = space
                .cached_display_name()
                .map(|dn| dn.to_string())
                .unwrap_or_default();
            let (child_space_ids, child_room_ids) = space_child_ids(&space)
                .await
                .into_iter()
                .partition(|child| space_ids.contains(child));
            let avatar_mxc = space.avatar_url().map(|mxc| mxc.to_string());
            let order = space_order(&space).await;
            DomainSpace {
                id: space.room_id().to_string(),
                name,
                avatar_mxc,
                child_room_ids,
                child_space_ids,
                order,
                unread: 0,
                mentions: 0,
            }
        })
        .buffered(SEED_INFLIGHT)
        .collect()
        .await
}

#[derive(Clone, Copy)]
enum AvatarKind {
    Room,
    Space,
}

enum AvatarState {
    Queued,
    InFlight,
    Failed { retry_at: Instant },
}

struct TrackedAvatar {
    kind: AvatarKind,
    mxc: OwnedMxcUri,
    state: AvatarState,
}

impl TrackedAvatar {
    fn retry_due(&self) -> bool {
        matches!(self.state, AvatarState::Failed { retry_at } if retry_at <= Instant::now())
    }
}

struct FetchedAvatar {
    key: String,
    kind: AvatarKind,
    fetched: bool,
}

struct AvatarFetcher {
    media: Arc<MediaService>,
    tasks: JoinSet<FetchedAvatar>,
    tracked: HashMap<String, TrackedAvatar>,
    queue: VecDeque<String>,
}

impl AvatarFetcher {
    fn new(media: Arc<MediaService>) -> Self {
        Self {
            media,
            tasks: JoinSet::new(),
            tracked: HashMap::new(),
            queue: VecDeque::new(),
        }
    }

    fn request(&mut self, client: &Client, kind: AvatarKind, uris: Vec<OwnedMxcUri>) {
        for mxc in uris {
            let key = mxc_avatar_key(mxc.as_str());
            if self
                .tracked
                .get(&key)
                .is_some_and(|tracked| !tracked.retry_due())
            {
                continue;
            }
            self.arm(key, kind, mxc);
        }
        self.pump(client);
    }

    fn arm(&mut self, key: String, kind: AvatarKind, mxc: OwnedMxcUri) {
        if self.media.cache_get(&key).is_some() {
            self.tracked.remove(&key);
            return;
        }
        let state = if self.media.is_failed(&key) {
            AvatarState::Failed {
                retry_at: Instant::now() + AVATAR_RETRY_COOLDOWN,
            }
        } else {
            self.queue.push_back(key.clone());
            AvatarState::Queued
        };
        self.tracked.insert(key, TrackedAvatar { kind, mxc, state });
    }

    fn retry_at(&self) -> Option<Instant> {
        self.tracked
            .values()
            .filter_map(|tracked| match tracked.state {
                AvatarState::Failed { retry_at } => Some(retry_at),
                AvatarState::Queued | AvatarState::InFlight => None,
            })
            .min()
    }

    fn retry_failed(&mut self, client: &Client) {
        let due: Vec<String> = self
            .tracked
            .iter()
            .filter(|(_, tracked)| tracked.retry_due())
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            let Some(tracked) = self.tracked.get(&key) else {
                continue;
            };
            let (kind, mxc) = (tracked.kind, tracked.mxc.clone());
            self.arm(key, kind, mxc);
        }
        self.pump(client);
    }

    fn pump(&mut self, client: &Client) {
        while self.tasks.len() < AVATAR_INFLIGHT {
            let Some(key) = self.queue.pop_front() else {
                break;
            };
            let Some(tracked) = self.tracked.get_mut(&key) else {
                continue;
            };
            if !matches!(tracked.state, AvatarState::Queued) {
                continue;
            }
            tracked.state = AvatarState::InFlight;
            let (kind, mxc) = (tracked.kind, tracked.mxc.clone());
            self.spawn_fetch(client, key, kind, mxc);
        }
    }

    fn spawn_fetch(&mut self, client: &Client, key: String, kind: AvatarKind, mxc: OwnedMxcUri) {
        let client = client.clone();
        let media = Arc::clone(&self.media);
        self.tasks.spawn(async move {
            let fetched = media
                .fetch_avatar_by_mxc(&client, &key, mxc)
                .await
                .is_some();
            FetchedAvatar { key, kind, fetched }
        });
    }

    fn finish(
        &mut self,
        client: &Client,
        joined: Result<FetchedAvatar, JoinError>,
    ) -> Option<AvatarKind> {
        let ready = self.record(joined);
        self.pump(client);
        ready
    }

    fn record(&mut self, joined: Result<FetchedAvatar, JoinError>) -> Option<AvatarKind> {
        let done = match joined {
            Ok(done) => done,
            Err(e) => {
                tracing::warn!("avatar fetch task did not complete: {e}");
                return None;
            }
        };
        if done.fetched {
            self.tracked.remove(&done.key);
            return Some(done.kind);
        }
        if let Some(tracked) = self.tracked.get_mut(&done.key) {
            tracked.state = AvatarState::Failed {
                retry_at: Instant::now() + AVATAR_RETRY_COOLDOWN,
            };
        }
        None
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

#[allow(clippy::struct_excessive_bools)]
struct Directory {
    rooms: HashMap<String, DomainRoom>,
    order: Vec<String>,
    spaces: Vec<DomainSpace>,
    rooms_dirty: bool,
    order_dirty: bool,
    spaces_dirty: bool,
    spaces_structural_dirty: bool,
    flush_at: Option<Instant>,
}

impl Directory {
    fn new() -> Self {
        Self {
            rooms: HashMap::new(),
            order: Vec::new(),
            spaces: Vec::new(),
            rooms_dirty: false,
            order_dirty: false,
            spaces_dirty: false,
            spaces_structural_dirty: false,
            flush_at: None,
        }
    }

    fn arm(&mut self) {
        if self.flush_at.is_none() {
            self.flush_at = Some(Instant::now() + EMIT_DEBOUNCE);
        }
    }

    fn mark_rooms(&mut self) {
        self.rooms_dirty = true;
        self.arm();
    }

    fn mark_spaces(&mut self) {
        self.spaces_dirty = true;
        self.arm();
    }

    fn mark_spaces_structural(&mut self) {
        self.spaces_structural_dirty = true;
        self.arm();
    }

    fn mark_kind(&mut self, kind: AvatarKind) {
        match kind {
            AvatarKind::Room => self.mark_rooms(),
            AvatarKind::Space => self.mark_spaces(),
        }
    }

    async fn seed(&mut self, client: &Client) {
        self.rooms = build_rooms(client).await;
        self.spaces = build_spaces_meta(client).await;
        self.order_dirty = true;
    }

    fn upsert_room(&mut self, room: DomainRoom) {
        let key = room.id.to_string();
        match self.rooms.get(&key) {
            Some(current) if *current == room => return,
            Some(current) => {
                if current.last_activity_ts != room.last_activity_ts {
                    self.order_dirty = true;
                }
            }
            None => self.order_dirty = true,
        }
        self.rooms.insert(key, room);
        self.mark_rooms();
    }

    fn remove_room(&mut self, room_id: &str) {
        if self.rooms.remove(room_id).is_none() {
            return;
        }
        self.order_dirty = true;
        self.mark_rooms();
    }

    async fn apply_room_updates(&mut self, client: &Client, updates: &RoomUpdates) {
        for room_id in updates.left.keys() {
            let key = room_id.as_str();
            self.remove_room(key);
            if self.spaces.iter().any(|space| space.id == key) {
                self.mark_spaces_structural();
            }
        }
        for room_id in updates.joined.keys() {
            let Some(room) = client.get_room(room_id) else {
                continue;
            };
            if room.is_space() {
                self.mark_spaces_structural();
            } else {
                self.upsert_room(build_single_room(&room).await);
            }
        }
    }

    async fn refresh_room(&mut self, client: &Client, room_id: &MatrixRoomId) {
        let Some(room) = client.get_room(room_id) else {
            return;
        };
        if room.is_space() {
            self.mark_spaces_structural();
            return;
        }
        if !self.rooms.contains_key(room_id.as_str()) {
            return;
        }
        self.upsert_room(build_single_room(&room).await);
    }

    fn refresh_order(&mut self) {
        if !self.order_dirty {
            return;
        }
        self.order = self.rooms.keys().cloned().collect();
        let rooms = &self.rooms;
        self.order.sort_by(|a, b| {
            let activity = |id| {
                rooms
                    .get(id)
                    .map_or(0, |room: &DomainRoom| room.last_activity_ts)
            };
            activity(b).cmp(&activity(a)).then_with(|| a.cmp(b))
        });
        self.order_dirty = false;
    }

    async fn flush(&mut self, client: &Client, on_sync: &OnSync, avatars: &mut AvatarFetcher) {
        self.flush_at = None;
        if self.spaces_structural_dirty {
            self.spaces = build_spaces_meta(client).await;
            self.spaces_structural_dirty = false;
            self.spaces_dirty = true;
        }
        if self.rooms_dirty {
            self.emit_rooms(client, on_sync, avatars);
            self.rooms_dirty = false;
        }
        if self.spaces_dirty {
            self.emit_spaces(client, on_sync, avatars);
            self.spaces_dirty = false;
        }
    }

    fn emit_rooms(&mut self, client: &Client, on_sync: &OnSync, avatars: &mut AvatarFetcher) {
        self.refresh_order();
        let rooms: Vec<DomainRoom> = self
            .order
            .iter()
            .filter_map(|id| self.rooms.get(id))
            .cloned()
            .collect();
        avatars.request(
            client,
            AvatarKind::Room,
            avatar_uris(&rooms, |room| room.avatar_mxc.as_deref()),
        );
        on_sync(SyncEvent::Rooms(rooms.into()));
    }

    fn emit_spaces(&self, client: &Client, on_sync: &OnSync, avatars: &mut AvatarFetcher) {
        avatars.request(
            client,
            AvatarKind::Space,
            avatar_uris(&self.spaces, |space| space.avatar_mxc.as_deref()),
        );
        on_sync(SyncEvent::Spaces(Arc::from(self.spaces.as_slice())));
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
    match err {
        matrix_sdk::Error::Http(http) => matches!(http.as_ref(), HttpError::RefreshToken(_)),
        _ => false,
    }
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
    Terminal(SyncOutcome),
}

async fn resync(client: &Client, dir: &mut Directory) {
    dir.seed(client).await;
    dir.mark_rooms();
    dir.mark_spaces();
}

async fn handle_room_update(
    client: &Client,
    update: Result<RoomUpdates, RecvError>,
    dir: &mut Directory,
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
            dir.apply_room_updates(client, &updates).await;
            LoopAction::Continue
        }
        Err(RecvError::Lagged(n)) => {
            tracing::warn!("room updates lagged by {n} messages, full rebuild");
            resync(client, dir).await;
            LoopAction::Continue
        }
        Err(RecvError::Closed) => LoopAction::Terminal(SyncOutcome::Recoverable(
            "room updates channel closed".into(),
        )),
    }
}

async fn handle_room_info_update(
    client: &Client,
    update: Result<RoomInfoNotableUpdate, RecvError>,
    dir: &mut Directory,
) -> LoopAction {
    match update {
        Ok(update) => {
            dir.refresh_room(client, &update.room_id).await;
            LoopAction::Continue
        }
        Err(RecvError::Lagged(n)) => {
            tracing::warn!("room info updates lagged by {n} messages, full rebuild");
            resync(client, dir).await;
            LoopAction::Continue
        }
        Err(RecvError::Closed) => {
            LoopAction::Terminal(SyncOutcome::Recoverable("room info channel closed".into()))
        }
    }
}

#[allow(clippy::cognitive_complexity)]
async fn handle_sync_state(
    client: &Client,
    state: SyncState,
    sync_service: &SyncService,
    dir: &mut Directory,
    on_sync: &OnSync,
) -> LoopAction {
    match state {
        SyncState::Running => {
            tracing::info!("sliding sync reconnected");
            resync(client, dir).await;
            on_sync(SyncEvent::Connected);
            LoopAction::Continue
        }
        SyncState::Error(err) => {
            let msg = err.to_string();
            tracing::warn!("sliding sync error: {msg}");
            if is_auth_error(&err) {
                return LoopAction::Terminal(SyncOutcome::SessionExpired);
            }
            on_sync(SyncEvent::ConnectionError(msg));
            sync_service.start().await;
            LoopAction::Continue
        }
        SyncState::Terminated => {
            tracing::info!("sliding sync terminated");
            LoopAction::Terminal(SyncOutcome::Recoverable("sliding sync terminated".into()))
        }
        SyncState::Idle | SyncState::Offline => LoopAction::Continue,
    }
}

async fn run_sync_loop(
    client: &Client,
    sync_service: &SyncService,
    room_updates_rx: &mut Receiver<RoomUpdates>,
    on_sync: &OnSync,
    avatars: &mut AvatarFetcher,
) -> SyncOutcome {
    let mut dir = Directory::new();
    let mut state_stream = sync_service.state();
    let mut room_info_rx = client.room_info_notable_update_receiver();

    resync(client, &mut dir).await;
    dir.flush(client, on_sync, avatars).await;
    on_sync(SyncEvent::Connected);

    loop {
        let flush_at = dir.flush_at;
        let flush_fut = async move {
            match flush_at {
                Some(at) => sleep_until(at).await,
                None => future::pending::<()>().await,
            }
        };
        let retry_at = avatars.retry_at();
        let retry_fut = async move {
            match retry_at {
                Some(at) => sleep_until(at).await,
                None => future::pending::<()>().await,
            }
        };
        let action = tokio::select! {
            biased;
            state = state_stream.next() => match state {
                Some(state) => handle_sync_state(client, state, sync_service, &mut dir, on_sync).await,
                None => LoopAction::Terminal(SyncOutcome::Recoverable("sync state stream ended".into())),
            },
            () = flush_fut => {
                dir.flush(client, on_sync, avatars).await;
                LoopAction::Continue
            }
            () = retry_fut => {
                avatars.retry_failed(client);
                LoopAction::Continue
            }
            Some(joined) = avatars.tasks.join_next() => {
                if let Some(kind) = avatars.finish(client, joined) {
                    dir.mark_kind(kind);
                }
                LoopAction::Continue
            }
            update = room_updates_rx.recv() => {
                handle_room_update(client, update, &mut dir).await
            }
            info = room_info_rx.recv() => {
                handle_room_info_update(client, info, &mut dir).await
            }
        };
        if let LoopAction::Terminal(outcome) = action {
            return outcome;
        }
    }
}

pub(super) async fn start_sync(
    client: &Client,
    media: Arc<MediaService>,
    on_sync: OnSync,
    cancel: CancellationToken,
) -> SyncOutcome {
    let sync_service = match build_sync_service(client).await {
        Ok(service) => service,
        Err(e) => return SyncOutcome::Fatal(format!("failed to build sync service: {e}")),
    };
    let mut room_updates_rx = client.subscribe_to_all_room_updates();
    let mut avatars = AvatarFetcher::new(media);

    sync_service.start().await;
    tracing::info!("sliding sync service started");

    let outcome = tokio::select! {
        outcome = run_sync_loop(
            client,
            &sync_service,
            &mut room_updates_rx,
            &on_sync,
            &mut avatars,
        ) => outcome,
        () = cancel.cancelled() => {
            tracing::debug!("sync cancelled, stopping sync service");
            SyncOutcome::Cancelled
        }
    };

    sync_service.stop().await;
    outcome
}
