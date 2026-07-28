use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

use super::backend::{UiBackend, install_render_hooks, post_effect, selected_room_key};
use super::clock::install_clock_invalidation;
use super::decode::{AvatarSlot, request_avatar, request_media};
use super::dto::{
    MediaState, ThumbUpdate, enrich_to_update, message_to_dto, room_to_dto, space_to_dto,
};
use super::multiplex::spawn_event_multiplexer;
use super::present::{MessageKind, ServiceKind, ToastKind, VerifyStep};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::reorder_rows;
use super::schema::{
    bool_props, connection_states, int_props, login_activities, login_methods, login_phases,
    media_states, message_kinds, preview_kinds, service_kinds, simple_callbacks, string_props,
    timeline_states, toast_kinds, user_message_kinds, verification_activities, verification_phases,
};
use super::{emoji, router};
use crate::commands::{
    AppViewState, Effect, LoginActivity, LoginStep, UiCommand, UserMessage, UserMessageKind,
    VerificationActivity, ViewportChanged,
};
use crate::domain::models::{
    ConnectionStatus, EnrichmentDelta, LoginCredentials, LoginMethod, MessagePreviewKind, Room,
    Space, TimelineMessage, TimelineStatus, VerificationEmoji as DomainVerificationEmoji,
};
use crate::error::Result;
use crate::ports::media::MediaCache;

#[allow(clippy::all, clippy::pedantic, clippy::restriction, clippy::nursery)]
mod generated {
    slint::include_modules!();
}
use generated::{
    Actions, AppWindow, ConnectionState, DirectoryView, EmojiEntry, EmojiGroup, EmojiInsert,
    EmojiStore, LoginActivity as UiLoginActivity, LoginMethodKind as UiLoginMethodKind, LoginPhase,
    LoginView, MediaState as UiMediaState, MessageEntry, MessageKind as UiMessageKind,
    PreviewKind as UiPreviewKind, RoomEntry, RoomView, ServiceKind as UiServiceKind, SessionView,
    SpaceEntry, TimelineState, ToastKind as UiToastKind, UserMessage as UiUserMessage,
    UserMessageKind as UiUserMessageKind, VerificationActivity as UiVerificationActivity,
    VerificationEmoji, VerificationPhase, VerificationView,
};

fn actions(window: &AppWindow) -> Actions<'_> {
    window.global::<Actions>()
}

thread_local! {
    static TIMELINE_MODEL: RefCell<Option<Rc<VecModel<MessageEntry>>>> = const { RefCell::new(None) };
    static ROOMS_MODEL: RefCell<Option<Rc<VecModel<RoomEntry>>>> = const { RefCell::new(None) };
    static SPACES_MODEL: RefCell<Option<Rc<VecModel<SpaceEntry>>>> = const { RefCell::new(None) };
    static SUBSPACES_MODEL: RefCell<Option<Rc<VecModel<SpaceEntry>>>> = const { RefCell::new(None) };
}

macro_rules! impl_prop_setter {
    ($fn:ident $enum:ident $ty:ty; $($v:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        fn $fn(&self, prop: $enum, value: $ty) {
            match prop { $($enum::$v => self.global::<$g>().$s(value),)* }
        }
    };
}

macro_rules! bind_compiled_callbacks {
    ($win:ident $tx:ident; $($on:ident $lit:literal $g:ident $gname:literal $fn:ident $kind:ident $cmd:ident;)*) => {
        $( bind_compiled_callbacks!(@one $win $tx $g $on $fn $kind); )*
    };
    (@one $win:ident $tx:ident $g:ident $on:ident $fn:ident plain) => {
        bind_compiled_callbacks!(@unit $win $tx $g $on $fn)
    };
    (@one $win:ident $tx:ident $g:ident $on:ident $fn:ident manual_unit) => {
        bind_compiled_callbacks!(@unit $win $tx $g $on $fn)
    };
    (@one $win:ident $tx:ident $g:ident $on:ident $fn:ident pass) => {
        bind_compiled_callbacks!(@string $win $tx $g $on $fn)
    };
    (@one $win:ident $tx:ident $g:ident $on:ident $fn:ident room) => {
        bind_compiled_callbacks!(@string $win $tx $g $on $fn)
    };
    (@one $win:ident $tx:ident $g:ident $on:ident $fn:ident opt_room) => {
        bind_compiled_callbacks!(@string $win $tx $g $on $fn)
    };
    (@one $win:ident $tx:ident $g:ident $on:ident $fn:ident manual_string) => {
        bind_compiled_callbacks!(@string $win $tx $g $on $fn)
    };
    (@unit $win:ident $tx:ident $g:ident $on:ident $fn:ident) => {{
        let tx = $tx.clone();
        $win.global::<$g>().$on(move || router::$fn(&tx));
    }};
    (@string $win:ident $tx:ident $g:ident $on:ident $fn:ident) => {{
        let tx = $tx.clone();
        $win.global::<$g>().$on(move |arg| router::$fn(&tx, arg.to_string()));
    }};
}

impl UiProps for AppWindow {
    string_props!(impl_prop_setter set_string StringProp SharedString;);
    bool_props!(impl_prop_setter set_bool BoolProp bool;);
    int_props!(impl_prop_setter set_int IntProp i32;);

    fn set_login_phase(&self, step: LoginStep) {
        self.global::<LoginView>().set_step(to_login_phase(step));
    }

    fn set_login_activity(&self, activity: LoginActivity) {
        self.global::<LoginView>()
            .set_activity(to_login_activity(activity));
    }

    fn set_login_method_kind(&self, method: LoginMethod) {
        self.global::<LoginView>()
            .set_method(to_login_method(method));
    }

    fn set_toast_kind(&self, kind: ToastKind) {
        self.global::<RoomView>()
            .set_toast_kind(to_toast_kind(kind));
    }

    fn set_toast_message(&self, kind: UserMessageKind) {
        self.global::<RoomView>()
            .set_toast_message(to_user_message_kind(kind));
    }

    fn set_verification_error(&self, kind: UserMessageKind) {
        self.global::<VerificationView>()
            .set_error(to_user_message_kind(kind));
    }

    fn set_connection_state(&self, status: &ConnectionStatus) {
        self.global::<SessionView>()
            .set_connection_status(to_connection_state(status));
    }

    fn set_timeline_state(&self, status: TimelineStatus) {
        self.global::<RoomView>()
            .set_timeline_status(to_timeline_state(status));
    }

    fn set_verification_phase(&self, phase: VerifyStep) {
        self.global::<VerificationView>()
            .set_step(to_verification_phase(phase));
    }

    fn set_verification_activity(&self, activity: VerificationActivity) {
        self.global::<VerificationView>()
            .set_activity(to_verification_activity(activity));
    }

    fn get_string(&self, prop: StringProp) -> SharedString {
        match prop {
            StringProp::SelectedRoomId => self.global::<DirectoryView>().get_selected_room_id(),
            other => {
                tracing::warn!("unexpected get for property: {}", other.as_str());
                SharedString::default()
            }
        }
    }

    fn get_int(&self, prop: IntProp) -> i32 {
        match prop {
            IntProp::SelectedGeneration => self.global::<DirectoryView>().get_selected_generation(),
            other => {
                tracing::warn!("unexpected get for property: {}", other.as_str());
                0
            }
        }
    }

    fn apply_user_avatar(&self, avatar: Option<Image>) {
        let session = self.global::<SessionView>();
        match avatar {
            Some(img) => {
                session.set_user_avatar(img);
                session.set_user_has_avatar(true);
            }
            None => session.set_user_has_avatar(false),
        }
    }

    fn apply_login_messages(&self, messages: &[UserMessage]) {
        let entries: Vec<UiUserMessage> = messages
            .iter()
            .map(|m| UiUserMessage {
                kind: to_user_message_kind(m.kind),
                detail: SharedString::from(&m.detail),
            })
            .collect();
        self.global::<LoginView>()
            .set_messages(ModelRc::new(VecModel::from(entries)));
    }

    fn apply_emoji_model(&self, emojis: &[DomainVerificationEmoji]) {
        let entries: Vec<VerificationEmoji> = emojis
            .iter()
            .map(|e| VerificationEmoji {
                symbol: SharedString::from(&e.symbol),
                description: SharedString::from(&e.description),
            })
            .collect();
        self.global::<VerificationView>()
            .set_emojis(ModelRc::new(VecModel::from(entries)));
    }

    fn clear_emoji_model(&self) {
        self.global::<VerificationView>()
            .set_emojis(ModelRc::new(VecModel::<VerificationEmoji>::default()));
    }

    fn clear_text_inputs(&self) {
        self.set_input_username(SharedString::default());
        self.set_input_password(SharedString::default());
        self.set_input_message(SharedString::default());
    }
}

macro_rules! to_slint_enum {
    (val $fn:ident $src:ident $dst:ident; $($rows:tt)*) => {
        fn $fn(value: $src) -> $dst { to_slint_enum!(@arms value, $src, $dst, $($rows)*) }
    };
    (ref $fn:ident $src:ident $dst:ident; $($rows:tt)*) => {
        fn $fn(value: &$src) -> $dst { to_slint_enum!(@arms value, $src, $dst, $($rows)*) }
    };
    (@arms $v:ident, $src:ident, $dst:ident,
        $($rust:ident $(($($p:tt)*))? $({$($b:tt)*})? $ui:ident $lit:literal;)*) => {
        match $v { $($src::$rust $(($($p)*))? $({$($b)*})? => $dst::$ui,)* }
    };
}

login_phases!(to_slint_enum val to_login_phase LoginStep LoginPhase;);
login_activities!(to_slint_enum val to_login_activity LoginActivity UiLoginActivity;);
login_methods!(to_slint_enum val to_login_method LoginMethod UiLoginMethodKind;);
connection_states!(to_slint_enum ref to_connection_state ConnectionStatus ConnectionState;);
timeline_states!(to_slint_enum val to_timeline_state TimelineStatus TimelineState;);
verification_phases!(to_slint_enum val to_verification_phase VerifyStep VerificationPhase;);
verification_activities!(
    to_slint_enum val to_verification_activity VerificationActivity UiVerificationActivity;
);
toast_kinds!(to_slint_enum val to_toast_kind ToastKind UiToastKind;);
user_message_kinds!(to_slint_enum val to_user_message_kind UserMessageKind UiUserMessageKind;);
media_states!(to_slint_enum val to_media_state MediaState UiMediaState;);
message_kinds!(to_slint_enum val to_message_kind MessageKind UiMessageKind;);
preview_kinds!(to_slint_enum val to_preview_kind MessagePreviewKind UiPreviewKind;);
service_kinds!(to_slint_enum val to_service_kind ServiceKind UiServiceKind;);

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

    fn message_id(entry: &MessageEntry) -> &str {
        entry.unique_id.as_str()
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
        entry.media_state = UiMediaState::Ready;
    }

    fn set_message_media_failed(entry: &mut MessageEntry) {
        entry.media_state = UiMediaState::Failed;
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

    #[allow(clippy::unnecessary_wraps)]
    pub fn register_callbacks(
        &self,
        cmd_tx: &mpsc::UnboundedSender<UiCommand>,
        scroll_tx: &watch::Sender<ViewportChanged>,
    ) -> Result<()> {
        setup_emoji_store(&self.window);

        let win = &self.window;
        simple_callbacks!(bind_compiled_callbacks win cmd_tx;);

        let tx = cmd_tx.clone();
        actions(win).on_login_password(move |req| {
            router::login_password(
                &tx,
                LoginCredentials {
                    username: req.username.to_string(),
                    password: req.password.to_string(),
                },
            );
        });

        let tx = cmd_tx.clone();
        actions(win).on_move_space(move |from, to| {
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
        actions(win).on_send_message(move |req| {
            router::send_message(
                &tx,
                req.room_id.to_string(),
                req.body.to_string(),
                req.reply_to.to_string(),
            );
        });

        actions(win).on_request_media(move |unique_id| request_media(&unique_id));

        actions(win).on_request_room_avatar(move |room_id| {
            request_avatar(&AvatarSlot::Room(room_id.to_string()));
        });

        let tx = cmd_tx.clone();
        actions(win).on_save_file(move |req| {
            router::save_file(&tx, req.event_id.to_string(), req.filename.to_string());
        });

        let scroll_tx = scroll_tx.clone();
        let weak = self.window.as_weak();
        actions(win).on_scroll_position_changed(move |at_bottom| {
            router::scroll_position(
                &scroll_tx,
                selected_room_key::<CompiledBackend>(&weak),
                at_bottom,
            );
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        actions(win).on_paginate_backwards(move || {
            router::paginate_backwards(&tx, selected_room_key::<CompiledBackend>(&weak));
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        actions(win).on_paginate_forwards(move || {
            router::paginate_forwards(&tx, selected_room_key::<CompiledBackend>(&weak));
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        actions(win).on_jump_to_latest(move || {
            router::jump_to_latest(&tx, selected_room_key::<CompiledBackend>(&weak));
        });

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
            .global::<RoomView>()
            .set_timeline(ModelRc::from(Rc::clone(&timeline_model)));
        self.window
            .global::<DirectoryView>()
            .set_rooms(ModelRc::from(Rc::clone(&rooms_model)));
        self.window
            .global::<DirectoryView>()
            .set_spaces(ModelRc::from(Rc::clone(&spaces_model)));
        self.window
            .global::<DirectoryView>()
            .set_subspaces(ModelRc::from(Rc::clone(&subspaces_model)));

        TIMELINE_MODEL.with(|cell| *cell.borrow_mut() = Some(timeline_model));
        ROOMS_MODEL.with(|cell| *cell.borrow_mut() = Some(rooms_model));
        SPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(spaces_model));
        SUBSPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(subspaces_model));

        install_render_hooks::<CompiledBackend>(self.window.as_weak());
        install_clock_invalidation::<CompiledBackend>(Arc::clone(&media_cache));

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
        message_type: to_message_kind(d.message_type),
        preview_kind: to_preview_kind(d.preview_kind),
        unsupported_kind: d.unsupported_kind,
        thumbnail: d.thumbnail.unwrap_or_default(),
        media_state: to_media_state(d.media_state),
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
        reply_kind: to_preview_kind(d.reply_kind),
        reply_body: d.reply_body,
        service_kind: to_service_kind(d.service_kind),
        service_target: d.service_target,
    }
}

fn enrich_entry(entry: &mut MessageEntry, delta: &EnrichmentDelta, media: &dyn MediaCache) {
    let update = enrich_to_update(delta, media);
    match update.thumbnail {
        ThumbUpdate::Ready(img) => {
            entry.thumbnail = img;
            entry.media_state = UiMediaState::Ready;
        }
        ThumbUpdate::Failed => entry.media_state = UiMediaState::Failed,
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
        last_message_kind: to_preview_kind(d.last_message_kind),
        last_message_body: d.last_message_body,
        last_message_service_kind: to_service_kind(d.last_message_service_kind),
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
