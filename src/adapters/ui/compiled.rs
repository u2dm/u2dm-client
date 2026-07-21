use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

use super::backend::{UiBackend, install_render_hooks, post_effect, selected_room_key};
use super::decode::{AvatarSlot, request_avatar, request_media};
use super::dto::{ThumbUpdate, enrich_to_update, message_to_dto, room_to_dto, space_to_dto};
use super::multiplex::spawn_event_multiplexer;
use super::present::VerifyStep;
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::reorder_rows;
use super::schema::{bool_props, int_props, string_props};
use super::{emoji, router};
use crate::commands::{AppViewState, Effect, LoginStep, UiCommand, ViewportChanged};
use crate::domain::models::{
    ConnectionStatus, EnrichmentDelta, LoginCredentials, Room, Space, TimelineMessage,
    TimelineStatus, VerificationEmoji as DomainVerificationEmoji,
};
use crate::error::Result;
use crate::ports::media::MediaCache;

#[allow(clippy::all, clippy::pedantic, clippy::restriction, clippy::nursery)]
mod generated {
    slint::include_modules!();
}
use generated::{
    AppWindow, ConnectionState, EmojiEntry, EmojiGroup, EmojiInsert, EmojiStore, LoginPhase,
    MessageEntry, RoomEntry, SpaceEntry, TimelineState, VerificationEmoji, VerificationPhase,
};

thread_local! {
    static TIMELINE_MODEL: RefCell<Option<Rc<VecModel<MessageEntry>>>> = const { RefCell::new(None) };
    static ROOMS_MODEL: RefCell<Option<Rc<VecModel<RoomEntry>>>> = const { RefCell::new(None) };
    static SPACES_MODEL: RefCell<Option<Rc<VecModel<SpaceEntry>>>> = const { RefCell::new(None) };
    static SUBSPACES_MODEL: RefCell<Option<Rc<VecModel<SpaceEntry>>>> = const { RefCell::new(None) };
}

macro_rules! impl_prop_setter {
    ($fn:ident $enum:ident $ty:ty; $($v:ident $c:ident $lit:literal $s:ident;)*) => {
        fn $fn(&self, prop: $enum, value: $ty) {
            match prop { $($enum::$v => self.$s(value),)* }
        }
    };
}

impl UiProps for AppWindow {
    string_props!(impl_prop_setter set_string StringProp SharedString;);
    bool_props!(impl_prop_setter set_bool BoolProp bool;);
    int_props!(impl_prop_setter set_int IntProp i32;);

    fn set_login_phase(&self, step: LoginStep) {
        self.set_login_step(to_login_phase(step));
    }

    fn set_connection_state(&self, status: &ConnectionStatus) {
        self.set_connection_status(to_connection_state(status));
    }

    fn set_timeline_state(&self, status: TimelineStatus) {
        self.set_timeline_status(to_timeline_state(status));
    }

    fn set_verification_phase(&self, phase: VerifyStep) {
        self.set_verification_step(to_verification_phase(phase));
    }

    fn get_string(&self, prop: StringProp) -> SharedString {
        match prop {
            StringProp::SelectedRoomId => self.get_selected_room_id(),
            other => {
                tracing::warn!("unexpected get for property: {}", other.as_str());
                SharedString::default()
            }
        }
    }

    fn get_int(&self, prop: IntProp) -> i32 {
        match prop {
            IntProp::SelectedGeneration => self.get_selected_generation(),
            other => {
                tracing::warn!("unexpected get for property: {}", other.as_str());
                0
            }
        }
    }

    fn apply_user_avatar(&self, avatar: Option<Image>) {
        match avatar {
            Some(img) => {
                self.set_user_avatar(img);
                self.set_user_has_avatar(true);
            }
            None => self.set_user_has_avatar(false),
        }
    }

    fn apply_emoji_model(&self, emojis: &[DomainVerificationEmoji]) {
        let entries: Vec<VerificationEmoji> = emojis
            .iter()
            .map(|e| VerificationEmoji {
                symbol: SharedString::from(&e.symbol),
                description: SharedString::from(&e.description),
            })
            .collect();
        self.set_verification_emojis(ModelRc::new(VecModel::from(entries)));
    }

    fn clear_emoji_model(&self) {
        self.set_verification_emojis(ModelRc::new(VecModel::<VerificationEmoji>::default()));
    }
}

fn to_login_phase(step: LoginStep) -> LoginPhase {
    match step {
        LoginStep::Homeserver => LoginPhase::Homeserver,
        LoginStep::Credentials => LoginPhase::Credentials,
        LoginStep::LoggedIn => LoginPhase::LoggedIn,
    }
}

fn to_connection_state(status: &ConnectionStatus) -> ConnectionState {
    match status {
        ConnectionStatus::Disconnected => ConnectionState::Disconnected,
        ConnectionStatus::Connecting => ConnectionState::Connecting,
        ConnectionStatus::Connected => ConnectionState::Connected,
        ConnectionStatus::Error(_) => ConnectionState::Error,
    }
}

fn to_timeline_state(status: TimelineStatus) -> TimelineState {
    match status {
        TimelineStatus::Loading => TimelineState::Loading,
        TimelineStatus::Ready => TimelineState::Ready,
        TimelineStatus::Failed { .. } => TimelineState::Failed,
        TimelineStatus::Disconnected => TimelineState::Disconnected,
    }
}

fn to_verification_phase(phase: VerifyStep) -> VerificationPhase {
    match phase {
        VerifyStep::None => VerificationPhase::None,
        VerifyStep::Requested => VerificationPhase::Requested,
        VerifyStep::Emojis => VerificationPhase::Emojis,
        VerifyStep::Confirming => VerificationPhase::Confirming,
        VerifyStep::Done => VerificationPhase::Done,
        VerifyStep::Cancelled => VerificationPhase::Cancelled,
    }
}

pub struct CompiledBackend;

impl UiBackend for CompiledBackend {
    type Window = AppWindow;
    type Message = MessageEntry;
    type Room = RoomEntry;
    type Space = SpaceEntry;

    fn convert_message(message: &TimelineMessage, media: &dyn MediaCache) -> MessageEntry {
        message_to_entry(message, media)
    }

    fn enrich_message(entry: &mut MessageEntry, delta: &EnrichmentDelta, media: &dyn MediaCache) {
        enrich_entry(entry, delta, media);
    }

    fn convert_room(room: &Room, media: &dyn MediaCache) -> RoomEntry {
        room_to_entry(room, media)
    }

    fn convert_space(space: &Space, media: &dyn MediaCache) -> SpaceEntry {
        space_to_entry(space, media)
    }

    fn message_id(entry: &MessageEntry) -> String {
        entry.unique_id.to_string()
    }

    fn room_id(entry: &RoomEntry) -> &str {
        entry.id.as_str()
    }

    fn space_id(entry: &SpaceEntry) -> &str {
        entry.id.as_str()
    }

    fn set_message_avatar(entry: &mut MessageEntry, image: &Image) {
        entry.avatar = image.clone();
        entry.has_avatar = true;
    }

    fn set_room_avatar(entry: &mut RoomEntry, image: &Image) {
        entry.avatar = image.clone();
        entry.has_avatar = true;
    }

    fn set_space_avatar(entry: &mut SpaceEntry, image: &Image) {
        entry.avatar = image.clone();
        entry.has_avatar = true;
    }

    fn set_message_thumbnail(entry: &mut MessageEntry, image: &Image) {
        entry.thumbnail = image.clone();
        entry.has_thumbnail = true;
        entry.media_failed = false;
    }

    fn set_message_media_failed(entry: &mut MessageEntry) {
        entry.media_failed = true;
    }

    fn set_message_frame(entry: &mut MessageEntry, image: Image) {
        entry.thumbnail = image;
    }

    fn with_models<R>(
        f: impl FnOnce(
            &VecModel<MessageEntry>,
            &VecModel<RoomEntry>,
            &VecModel<SpaceEntry>,
            &VecModel<SpaceEntry>,
        ) -> R,
    ) -> Option<R> {
        let timeline = TIMELINE_MODEL.with(|cell| cell.borrow().clone())?;
        let rooms = ROOMS_MODEL.with(|cell| cell.borrow().clone())?;
        let spaces = SPACES_MODEL.with(|cell| cell.borrow().clone())?;
        let subspaces = SUBSPACES_MODEL.with(|cell| cell.borrow().clone())?;
        Some(f(&timeline, &rooms, &spaces, &subspaces))
    }

    fn with_timeline<R>(f: impl FnOnce(&VecModel<MessageEntry>) -> R) -> Option<R> {
        let timeline = TIMELINE_MODEL.with(|cell| cell.borrow().clone())?;
        Some(f(&timeline))
    }
}

pub struct SlintUiAdapter {
    window: AppWindow,
}

impl SlintUiAdapter {
    pub fn compile(_rt: &Runtime) -> Result<Self> {
        let window = AppWindow::new()?;
        Ok(Self { window })
    }

    #[allow(clippy::too_many_lines, clippy::unnecessary_wraps)]
    pub fn register_callbacks(
        &self,
        cmd_tx: &mpsc::UnboundedSender<UiCommand>,
        scroll_tx: &watch::Sender<ViewportChanged>,
    ) -> Result<()> {
        setup_emoji_store(&self.window);

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_check_server(move |homeserver| {
            let w = weak.upgrade();
            router::check_server(props(w.as_ref()), &tx, homeserver.to_string());
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_login_password(move |req| {
            let creds = LoginCredentials {
                homeserver: req.homeserver.to_string(),
                username: req.username.to_string(),
                password: req.password.to_string(),
            };
            let w = weak.upgrade();
            router::login_password(props(w.as_ref()), &tx, creds);
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_login_oauth(move || {
            let w = weak.upgrade();
            router::login_oauth(props(w.as_ref()), &tx);
        });

        let tx = cmd_tx.clone();
        self.window
            .on_cancel_oauth(move || router::cancel_oauth(&tx));

        let tx = cmd_tx.clone();
        self.window
            .on_select_room(move |room_id| router::select_room(&tx, room_id.to_string()));

        let tx = cmd_tx.clone();
        self.window
            .on_select_space(move |space_id| router::select_space(&tx, space_id.to_string()));

        let tx = cmd_tx.clone();
        self.window
            .on_select_subspace(move |space_id| router::select_subspace(&tx, space_id.to_string()));

        let tx = cmd_tx.clone();
        self.window.on_move_space(move |from, to| {
            let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                return;
            };
            router::move_space(&tx, from, to, |from, to| {
                SPACES_MODEL.with(|cell| {
                    if let Some(model) = cell.borrow().as_ref() {
                        reorder_rows(model, from, to);
                    }
                });
            });
        });

        let tx = cmd_tx.clone();
        self.window.on_logout(move || router::logout(&tx));

        let tx = cmd_tx.clone();
        self.window.on_send_message(move |req| {
            router::send_message(
                &tx,
                req.room_id.to_string(),
                req.body.to_string(),
                req.reply_to.to_string(),
            );
        });

        let tx = cmd_tx.clone();
        self.window
            .on_accept_verification(move || router::accept_verification(&tx));

        let tx = cmd_tx.clone();
        self.window
            .on_confirm_verification(move || router::confirm_verification(&tx));

        let tx = cmd_tx.clone();
        self.window
            .on_reject_verification(move || router::reject_verification(&tx));

        let tx = cmd_tx.clone();
        self.window
            .on_open_media(move |event_id| router::open_media(&tx, event_id.to_string()));

        self.window
            .on_request_media(move |unique_id| request_media(&unique_id));

        self.window.on_request_room_avatar(move |room_id| {
            request_avatar(&AvatarSlot::Room(room_id.to_string()));
        });

        let tx = cmd_tx.clone();
        self.window.on_save_file(move |req| {
            router::save_file(&tx, req.event_id.to_string(), req.filename.to_string());
        });

        let scroll_tx = scroll_tx.clone();
        let weak = self.window.as_weak();
        self.window
            .on_scroll_position_changed(move |at_top, at_bottom| {
                router::scroll_position(
                    &scroll_tx,
                    selected_room_key::<CompiledBackend>(&weak),
                    at_top,
                    at_bottom,
                );
            });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_paginate_backwards(move || {
            router::paginate_backwards(&tx, selected_room_key::<CompiledBackend>(&weak));
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_paginate_forwards(move || {
            router::paginate_forwards(&tx, selected_room_key::<CompiledBackend>(&weak));
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_jump_to_latest(move || {
            router::jump_to_latest(&tx, selected_room_key::<CompiledBackend>(&weak));
        });

        let tx = cmd_tx.clone();
        self.window
            .on_retry_timeline(move || router::retry_timeline(&tx));

        Ok(())
    }

    pub fn spawn_event_handler(
        &self,
        ui_rx: mpsc::Receiver<Effect>,
        view_rx: watch::Receiver<Arc<AppViewState>>,
        media_cache: Arc<dyn MediaCache>,
    ) {
        let weak = self.window.as_weak();
        let timeline_model: Rc<VecModel<MessageEntry>> = Rc::new(VecModel::default());
        let rooms_model: Rc<VecModel<RoomEntry>> = Rc::new(VecModel::default());
        let spaces_model: Rc<VecModel<SpaceEntry>> = Rc::new(VecModel::default());
        let subspaces_model: Rc<VecModel<SpaceEntry>> = Rc::new(VecModel::default());

        self.window
            .set_timeline(ModelRc::from(Rc::clone(&timeline_model)));
        self.window
            .set_rooms(ModelRc::from(Rc::clone(&rooms_model)));
        self.window
            .set_spaces(ModelRc::from(Rc::clone(&spaces_model)));
        self.window
            .set_subspaces(ModelRc::from(Rc::clone(&subspaces_model)));

        TIMELINE_MODEL.with(|cell| *cell.borrow_mut() = Some(timeline_model));
        ROOMS_MODEL.with(|cell| *cell.borrow_mut() = Some(rooms_model));
        SPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(spaces_model));
        SUBSPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(subspaces_model));

        install_render_hooks::<CompiledBackend>(self.window.as_weak());

        spawn_event_multiplexer(ui_rx, view_rx, media_cache, move |event, media, permit| {
            post_effect::<CompiledBackend>(&weak, media, event, permit);
        });
    }

    pub fn run(&self) -> Result<()> {
        self.window.run()?;
        Ok(())
    }

    #[cfg(feature = "demo")]
    pub fn set_window_size(&self, width: f32, height: f32) {
        self.window
            .window()
            .set_size(slint::LogicalSize::new(width, height));
    }
}

fn props(window: Option<&AppWindow>) -> Option<&dyn UiProps> {
    window.map(|w| w as &dyn UiProps)
}

fn emoji_entry_to_ui(e: &emoji::EmojiEntry) -> EmojiEntry {
    let tones: Vec<SharedString> = e
        .tones
        .iter()
        .map(|t| SharedString::from(t.as_str()))
        .collect();
    EmojiEntry {
        base: SharedString::from(&e.base),
        tones: ModelRc::new(VecModel::from(tones)),
        name: SharedString::from(&e.name),
    }
}

fn setup_emoji_store(window: &AppWindow) {
    let store = window.global::<EmojiStore>();
    let groups: Vec<EmojiGroup> = emoji::groups()
        .iter()
        .map(|items| {
            let entries: Vec<EmojiEntry> = items.iter().map(emoji_entry_to_ui).collect();
            EmojiGroup {
                items: ModelRc::new(VecModel::from(entries)),
            }
        })
        .collect();
    store.set_groups(ModelRc::new(VecModel::from(groups)));

    let weak = window.as_weak();
    store.on_search(move |query| {
        let Some(w) = weak.upgrade() else {
            return;
        };
        let results: Vec<EmojiEntry> = emoji::search(&query)
            .iter()
            .map(emoji_entry_to_ui)
            .collect();
        w.global::<EmojiStore>()
            .set_results(ModelRc::new(VecModel::from(results)));
    });

    store.on_insert(|text, offset, glyph| {
        let (inserted, caret) = emoji::insert_at(text.as_str(), offset, glyph.as_str());
        EmojiInsert {
            text: SharedString::from(inserted),
            caret,
        }
    });
}

fn string_model(items: Vec<SharedString>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items))
}

fn message_to_entry(m: &TimelineMessage, media: &dyn MediaCache) -> MessageEntry {
    let d = message_to_dto(m, media);
    MessageEntry {
        unique_id: d.unique_id,
        sender: d.sender,
        pronouns: string_model(d.pronouns),
        body: d.body,
        timestamp: d.timestamp,
        message_type: d.message_type,
        preview_kind: d.preview_kind,
        unsupported_kind: d.unsupported_kind,
        has_thumbnail: d.has_thumbnail,
        thumbnail: d.thumbnail.unwrap_or_default(),
        media_failed: d.media_failed,
        image_width: d.image_width,
        image_height: d.image_height,
        event_id: d.event_id,
        has_avatar: d.has_avatar,
        avatar: d.avatar.unwrap_or_default(),
        sender_initial: d.sender_initial,
        color_index: d.color_index,
        is_own: d.is_own,
        edited: d.edited,
        has_reply: d.has_reply,
        reply_sender: d.reply_sender,
        reply_kind: d.reply_kind,
        reply_body: d.reply_body,
        service_kind: d.service_kind,
        service_target: d.service_target,
    }
}

fn enrich_entry(entry: &mut MessageEntry, delta: &EnrichmentDelta, media: &dyn MediaCache) {
    let update = enrich_to_update(delta, media);
    match update.thumbnail {
        ThumbUpdate::Ready(img) => {
            entry.thumbnail = img;
            entry.has_thumbnail = true;
            entry.media_failed = false;
        }
        ThumbUpdate::Failed => entry.media_failed = true,
        ThumbUpdate::Unchanged => {}
    }
    if let Some(img) = update.avatar {
        entry.avatar = img;
        entry.has_avatar = true;
    }
    if let Some(pronouns) = update.pronouns {
        entry.pronouns = string_model(pronouns);
    }
}

fn room_to_entry(r: &Room, media: &dyn MediaCache) -> RoomEntry {
    let d = room_to_dto(r, media);
    RoomEntry {
        id: d.id,
        name: d.name,
        initial: d.initial,
        avatar: d.avatar.unwrap_or_default(),
        has_avatar: d.has_avatar,
        color_index: d.color_index,
        members: d.members,
        unread: d.unread,
        mentions: d.mentions,
        last_message_sender: d.last_message_sender,
        last_message_kind: d.last_message_kind,
        last_message_body: d.last_message_body,
        last_message_service_kind: d.last_message_service_kind,
        last_message_service_target: d.last_message_service_target,
        last_message_is_own: d.last_message_is_own,
        last_message_edited: d.last_message_edited,
        last_message_time: d.last_message_time,
    }
}

fn space_to_entry(s: &Space, media: &dyn MediaCache) -> SpaceEntry {
    let d = space_to_dto(s, media);
    SpaceEntry {
        id: d.id,
        name: d.name,
        unread: d.unread,
        mentions: d.mentions,
        initial: d.initial,
        avatar: d.avatar.unwrap_or_default(),
        has_avatar: d.has_avatar,
    }
}
