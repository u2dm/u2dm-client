use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

use super::backend::{UiBackend, install_render_hooks, post_effect, selected_room_key};
use super::clock::install_clock_invalidation;
use super::decode::{AvatarSlot, request_avatar, request_media, request_sticker};
use super::dto::{
    MediaFailureKind, MediaState, ReactionDto, ReactorAvatarDto, StickerCellDto, StickerPackDto,
    StickerRowDto, ThumbUpdate, enrich_to_update, message_to_dto, room_to_dto, space_to_dto,
};
#[cfg(feature = "demo")]
use super::dump;
use super::multiplex::spawn_event_multiplexer;
use super::present::{MessageKind, ServiceKind, VerifyStep};
use super::video::{self, millis_to_duration};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::reorder_rows;
use super::reduce::set_sticker_query;
use super::schema::{
    attachment_kinds, bool_props, connection_states, int_props, login_activities, login_methods, login_phases, media_failures, media_states, message_kinds, preview_kinds, send_states, service_kinds, simple_callbacks, string_props, timeline_states, user_message_kinds, verification_activities, verification_phases,
};
use super::{emoji, router};
use crate::commands::effects::{Effect, VerificationActivity};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::{UiCommand, ViewportChanged};
use crate::commands::view::{AppViewState, AttachmentKind, LoginActivity, LoginStep};
use crate::domain::auth::{LoginCredentials, LoginMethod};
use crate::domain::message::{MessagePreviewKind, SendState, TimelineMessage};
use crate::domain::room::{Room, Space};
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::{EnrichmentDelta, TimelineStatus};
use crate::domain::verification::VerificationEmoji as DomainVerificationEmoji;
use crate::error::Result;
use crate::ports::media::MediaCache;

#[allow(clippy::all, clippy::pedantic, clippy::restriction, clippy::nursery)]
mod generated {
    slint::include_modules!();
}
#[cfg(feature = "demo")]
use generated::Probe;
use generated::{
    Actions, AppWindow, AttachmentKind as UiAttachmentKind, AttachmentView, ConnectionState,
    DirectoryView, EmojiEntry, EmojiGroup, EmojiInsert, EmojiStore,
    LoginActivity as UiLoginActivity, LoginMethodKind as UiLoginMethodKind, LoginPhase, LoginView,
    MediaFailure as UiMediaFailure, MediaState as UiMediaState, MessageEntry,
    MessageKind as UiMessageKind, PreviewKind as UiPreviewKind, ReactionEntry, ReactorAvatar,
    RoomEntry, RoomView, SendState as UiSendState, ServiceKind as UiServiceKind, SessionView,
    SpaceEntry, StickerCell, StickerPackTab, StickerRow, StickerView, TimelineState,
    UserMessage as UiUserMessage, UserMessageKind as UiUserMessageKind,
    VerificationActivity as UiVerificationActivity, VerificationEmoji, VerificationPhase,
    VerificationView, VideoView,
};

fn actions(window: &AppWindow) -> Actions<'_> {
    window.global::<Actions>()
}

thread_local! {
    static TIMELINE_MODEL: RefCell<Option<Rc<VecModel<MessageEntry>>>> = const { RefCell::new(None) };
    static ROOMS_MODEL: RefCell<Option<Rc<VecModel<RoomEntry>>>> = const { RefCell::new(None) };
    static SPACES_MODEL: RefCell<Option<Rc<VecModel<SpaceEntry>>>> = const { RefCell::new(None) };
    static SUBSPACES_MODEL: RefCell<Option<Rc<VecModel<SpaceEntry>>>> = const { RefCell::new(None) };
    static STICKER_ROWS_MODEL: RefCell<Option<Rc<VecModel<StickerRow>>>> = const { RefCell::new(None) };
    static STICKER_PACKS_MODEL: RefCell<Option<Rc<VecModel<StickerPackTab>>>> = const { RefCell::new(None) };
}

macro_rules! impl_prop_setter {
    ($fn:ident $enum:ident $ty:ty; $($(#[$attr:meta])* $v:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        fn $fn(&self, prop: $enum, value: $ty) {
            match prop { $($(#[$attr])* $enum::$v => self.global::<$g>().$s(value),)* }
        }
    };
}

macro_rules! bind_compiled_callbacks {
    ($win:ident $tx:ident; $($on:ident $lit:literal $fn:ident $kind:ident $cmd:ident;)*) => {
        $( bind_compiled_callbacks!(@one $win $tx $on $fn $kind); )*
    };
    (@one $win:ident $tx:ident $on:ident $fn:ident plain) => {
        bind_compiled_callbacks!(@unit $win $tx $on $fn)
    };
    (@one $win:ident $tx:ident $on:ident $fn:ident pass) => {
        bind_compiled_callbacks!(@string $win $tx $on $fn)
    };
    (@one $win:ident $tx:ident $on:ident $fn:ident room) => {
        bind_compiled_callbacks!(@string $win $tx $on $fn)
    };
    (@one $win:ident $tx:ident $on:ident $fn:ident opt_room) => {
        bind_compiled_callbacks!(@string $win $tx $on $fn)
    };
    (@one $win:ident $tx:ident $on:ident $fn:ident manual_string) => {
        bind_compiled_callbacks!(@string $win $tx $on $fn)
    };
    (@unit $win:ident $tx:ident $on:ident $fn:ident) => {{
        let tx = $tx.clone();
        actions($win).$on(move || router::$fn(&tx));
    }};
    (@string $win:ident $tx:ident $on:ident $fn:ident) => {{
        let tx = $tx.clone();
        actions($win).$on(move |arg| router::$fn(&tx, arg.to_string()));
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

    fn set_toast_message(&self, kind: UserMessageKind) {
        self.global::<RoomView>()
            .set_toast_message(to_user_message_kind(kind));
    }

    fn set_verification_error(&self, kind: UserMessageKind) {
        self.global::<VerificationView>()
            .set_error(to_user_message_kind(kind));
    }

    fn set_attachment_error(&self, kind: UserMessageKind) {
        self.global::<AttachmentView>()
            .set_error(to_user_message_kind(kind));
    }

    fn set_attachment_kind(&self, kind: AttachmentKind) {
        self.global::<AttachmentView>()
            .set_kind(to_attachment_kind(kind));
    }

    fn set_video_error(&self, kind: UserMessageKind) {
        self.global::<VideoView>()
            .set_error(to_user_message_kind(kind));
    }

    fn apply_video_frame(&self, buffer: SharedPixelBuffer<Rgb8Pixel>) {
        let video = self.global::<VideoView>();
        video.set_frame(Image::from_rgb8(buffer));
        video.set_has_frame(true);
    }

    fn clear_video_frame(&self) {
        let video = self.global::<VideoView>();
        video.set_frame(Image::default());
        video.set_has_frame(false);
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

    fn apply_attachment_preview(&self, preview: Option<Image>) {
        let attachment = self.global::<AttachmentView>();
        match preview {
            Some(img) => {
                attachment.set_preview(img);
                attachment.set_has_preview(true);
            }
            None => attachment.set_has_preview(false),
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
user_message_kinds!(to_slint_enum val to_user_message_kind UserMessageKind UiUserMessageKind;);
media_states!(to_slint_enum val to_media_state MediaState UiMediaState;);
send_states!(to_slint_enum val to_send_state SendState UiSendState;);
media_failures!(to_slint_enum val to_media_failure MediaFailureKind UiMediaFailure;);
message_kinds!(to_slint_enum val to_message_kind MessageKind UiMessageKind;);
attachment_kinds!(to_slint_enum val to_attachment_kind AttachmentKind UiAttachmentKind;);
preview_kinds!(to_slint_enum val to_preview_kind MessagePreviewKind UiPreviewKind;);
service_kinds!(to_slint_enum val to_service_kind ServiceKind UiServiceKind;);

pub struct CompiledBackend;

impl UiBackend for CompiledBackend {
    type Window = AppWindow;
    type Message = MessageEntry;
    type Room = RoomEntry;
    type Space = SpaceEntry;
    type StickerRow = StickerRow;
    type StickerPack = StickerPackTab;

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

    fn convert_sticker_row(row: &StickerRowDto) -> StickerRow {
        StickerRow {
            title: row.title.clone(),
            is_header: row.is_header,
            cells: ModelRc::new(VecModel::from(
                row.cells.iter().map(sticker_to_entry).collect::<Vec<_>>(),
            )),
        }
    }

    fn convert_sticker_pack(pack: &StickerPackDto) -> StickerPackTab {
        StickerPackTab {
            id: pack.id.clone(),
            title: pack.title.clone(),
            header_row: pack.header_row,
            icon: pack.icon.clone().unwrap_or_default(),
            has_icon: pack.icon.is_some(),
        }
    }

    fn sticker_pack_with_icon(
        pack: &StickerPackTab,
        pack_id: &str,
        image: &Image,
    ) -> Option<StickerPackTab> {
        (!pack.has_icon && pack.id == pack_id).then(|| StickerPackTab {
            icon: image.clone(),
            has_icon: true,
            ..pack.clone()
        })
    }

    fn patch_sticker_cell(row: &StickerRow, key: &str, art: Option<&Image>) -> bool {
        let Some(index) = row.cells.iter().position(|cell| cell.key == key) else {
            return false;
        };
        let Some(cells) = row.cells.as_any().downcast_ref::<VecModel<StickerCell>>() else {
            return false;
        };
        let Some(mut cell) = cells.row_data(index) else {
            return false;
        };
        match art {
            Some(art) => {
                cell.image = art.clone();
                cell.media_state = UiMediaState::Ready;
            }
            None => cell.media_state = UiMediaState::Failed,
        }
        cells.set_row_data(index, cell);
        true
    }

    fn patch_reactor_avatar(entry: &MessageEntry, user_id: &str, image: &Image) -> bool {
        let Some(reactions) = entry
            .reactions
            .as_any()
            .downcast_ref::<VecModel<ReactionEntry>>()
        else {
            return false;
        };
        let mut patched = false;
        for reaction in reactions.iter() {
            let Some(faces) = reaction
                .avatars
                .as_any()
                .downcast_ref::<VecModel<ReactorAvatar>>()
            else {
                continue;
            };
            let Some(index) = faces
                .iter()
                .position(|face| face.user_id == user_id && !face.has_avatar)
            else {
                continue;
            };
            let Some(mut face) = faces.row_data(index) else {
                continue;
            };
            face.avatar = image.clone();
            face.has_avatar = true;
            faces.set_row_data(index, face);
            patched = true;
        }
        patched
    }

    fn message_id(entry: &MessageEntry) -> &str {
        entry.unique_id.as_str()
    }

    fn message_event_id(entry: &MessageEntry) -> &str {
        entry.event_id.as_str()
    }

    fn message_is_first_unread(entry: &MessageEntry) -> bool {
        entry.first_unread
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

    fn set_message_media_failed(entry: &mut MessageEntry, reason: MediaFailureKind) {
        entry.media_state = UiMediaState::Failed;
        entry.media_failure = to_media_failure(reason);
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

    fn with_stickers<R>(
        f: impl FnOnce(&VecModel<StickerRow>, &VecModel<StickerPackTab>) -> R,
    ) -> Option<R> {
        let rows = STICKER_ROWS_MODEL.with(|cell| cell.borrow().clone())?;
        let packs = STICKER_PACKS_MODEL.with(|cell| cell.borrow().clone())?;
        Some(f(&rows, &packs))
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
    fn bind_video_callbacks(win: &AppWindow) {
        let weak = win.as_weak();
        actions(win).on_toggle_video(move || {
            if let Some(window) = weak.upgrade() {
                video::toggle(&window);
            }
        });

        let weak = win.as_weak();
        actions(win).on_toggle_video_muted(move || {
            if let Some(window) = weak.upgrade() {
                video::toggle_muted(&window);
            }
        });

        let weak = win.as_weak();
        actions(win).on_seek_video(move |ms| {
            if let Some(window) = weak.upgrade() {
                video::seek(&window, millis_to_duration(usize::try_from(ms).ok()));
            }
        });
    }

    #[allow(clippy::unnecessary_wraps, reason = "mirrors the fallible interpreted adapter")]
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

        let tx = cmd_tx.clone();
        actions(win).on_send_sticker(move |req| {
            router::send_sticker(
                &tx,
                req.room_id.to_string(),
                req.pack_id.to_string(),
                req.shortcode.to_string(),
                req.reply_to.to_string(),
            );
        });

        let tx = cmd_tx.clone();
        actions(win).on_send_attachment(move |req| {
            router::send_attachment(
                &tx,
                req.room_id.to_string(),
                req.caption.to_string(),
                req.as_document,
                req.reply_to.to_string(),
            );
        });

        actions(win).on_request_media(move |unique_id| request_media(&unique_id));

        Self::bind_video_callbacks(win);

        actions(win).on_request_room_avatar(move |room_id| {
            request_avatar(&AvatarSlot::Room(room_id.to_string()));
        });

        actions(win).on_request_sticker(move |key| request_sticker(&key));

        let tx = cmd_tx.clone();
        actions(win).on_save_file(move |req| {
            router::save_file(&tx, req.event_id.to_string(), req.filename.to_string());
        });

        let tx = cmd_tx.clone();
        actions(win).on_toggle_reaction(move |event_id, key| {
            router::toggle_reaction(&tx, event_id.to_string(), key.to_string());
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
        let sticker_rows_model: Rc<VecModel<StickerRow>> = Rc::new(VecModel::default());
        let sticker_packs_model: Rc<VecModel<StickerPackTab>> = Rc::new(VecModel::default());

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
        self.window
            .global::<StickerView>()
            .set_rows(ModelRc::from(Rc::clone(&sticker_rows_model)));
        self.window
            .global::<StickerView>()
            .set_packs(ModelRc::from(Rc::clone(&sticker_packs_model)));

        TIMELINE_MODEL.with(|cell| *cell.borrow_mut() = Some(timeline_model));
        ROOMS_MODEL.with(|cell| *cell.borrow_mut() = Some(rooms_model));
        SPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(spaces_model));
        SUBSPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(subspaces_model));
        STICKER_ROWS_MODEL.with(|cell| *cell.borrow_mut() = Some(sticker_rows_model));
        STICKER_PACKS_MODEL.with(|cell| *cell.borrow_mut() = Some(sticker_packs_model));

        install_render_hooks::<CompiledBackend>(self.window.as_weak());
        install_clock_invalidation::<CompiledBackend>(Arc::clone(&media_cache));

        let media = Arc::clone(&media_cache);
        actions(&self.window).on_search_stickers(move |query| {
            set_sticker_query::<CompiledBackend>(&query, media.as_ref());
        });

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

    #[cfg(feature = "demo")]
    pub fn enable_probe_introspection(&self) {
        self.window.set_bool(BoolProp::ProbeEnabled, true);
    }
}

#[cfg(feature = "demo")]
pub fn install_timeline_dump(ui: &SlintUiAdapter) {
    let weak = ui.window.as_weak();
    dump::install(Box::new(move |reply| {
        let handle = weak.clone();
        let queued = handle.upgrade_in_event_loop(move |window| {
            drop(reply.send(probe_dump::collect(&window)));
        });
        if let Err(e) = queued {
            tracing::debug!("the timeline dump could not reach the event loop: {e}");
        }
    }));
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

fn reactor_to_entry(d: &ReactorAvatarDto) -> ReactorAvatar {
    ReactorAvatar {
        user_id: d.user_id.clone(),
        initial: d.initial.clone(),
        color_index: d.color_index,
        has_avatar: d.image.is_some(),
        avatar: d.image.clone().unwrap_or_default(),
    }
}

fn reactor_model(items: &[ReactorAvatarDto]) -> ModelRc<ReactorAvatar> {
    ModelRc::new(VecModel::from(
        items.iter().map(reactor_to_entry).collect::<Vec<_>>(),
    ))
}

fn reaction_to_entry(d: &ReactionDto) -> ReactionEntry {
    ReactionEntry {
        key: d.key.clone(),
        label: d.label.clone(),
        count: d.count,
        mine: d.mine,
        pending: d.pending,
        overflow: d.overflow,
        reactors: d.reactors.clone(),
        hidden_reactors: d.hidden_reactors,
        avatars: reactor_model(&d.avatars),
    }
}

fn reaction_model(items: &[ReactionDto]) -> ModelRc<ReactionEntry> {
    ModelRc::new(VecModel::from(
        items.iter().map(reaction_to_entry).collect::<Vec<_>>(),
    ))
}

fn message_to_entry(m: &TimelineMessage, media: &dyn MediaCache) -> MessageEntry {
    let d = message_to_dto(m, media);
    MessageEntry {
        unique_id: d.unique_id,
        sender: d.sender,
        sender_id: d.sender_id,
        pronouns: string_model(d.pronouns),
        body: d.body,
        styled: d.styled,
        has_links: d.has_links,
        timestamp: d.timestamp,
        message_type: to_message_kind(d.message_type),
        preview_kind: to_preview_kind(d.preview_kind),
        unsupported_kind: d.unsupported_kind,
        thumbnail: d.thumbnail.unwrap_or_default(),
        media_state: to_media_state(d.media_state),
        media_failure: to_media_failure(d.media_failure),
        image_mimetype: d.image_mimetype,
        image_extension: d.image_extension,
        image_width: d.image_width,
        image_height: d.image_height,
        duration: d.duration,
        event_id: d.event_id,
        has_avatar: d.has_avatar,
        needs_media: d.needs_media,
        avatar: d.avatar.unwrap_or_default(),
        sender_initial: d.sender_initial,
        color_index: d.color_index,
        is_own: d.is_own,
        edited: d.edited,
        first_unread: d.is_first_unread,
        send_state: to_send_state(d.send_state),
        send_progress: d.send_progress,
        has_reply: d.has_reply,
        reply_event_id: d.reply_event_id,
        reply_sender: d.reply_sender,
        reply_kind: to_preview_kind(d.reply_kind),
        reply_body: d.reply_body,
        service_kind: to_service_kind(d.service_kind),
        service_target: d.service_target,
        reactions: reaction_model(&d.reactions),
        all_reactions: reaction_model(&d.all_reactions),
    }
}

fn enrich_entry(entry: &mut MessageEntry, delta: &EnrichmentDelta, media: &dyn MediaCache) {
    let update = enrich_to_update(delta, media);
    match update.thumbnail {
        ThumbUpdate::Ready(img) => {
            entry.thumbnail = img;
            entry.media_state = UiMediaState::Ready;
        }
        ThumbUpdate::Failed(reason) => {
            entry.media_state = UiMediaState::Failed;
            entry.media_failure = to_media_failure(reason);
        }
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
        alert: d.alert,
        mention: d.mention,
        hint: d.hint,
        muted: d.muted,
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

fn sticker_to_entry(d: &StickerCellDto) -> StickerCell {
    StickerCell {
        key: d.key.clone(),
        pack_id: d.pack_id.clone(),
        shortcode: d.shortcode.clone(),
        label: d.label.clone(),
        image: d.image.clone().unwrap_or_default(),
        media_state: to_media_state(d.media_state),
    }
}

fn space_to_entry(s: &Space, media: &dyn MediaCache) -> SpaceEntry {
    let d = space_to_dto(s, media);
    SpaceEntry {
        id: d.id,
        name: d.name,
        alert: d.alert,
        mention: d.mention,
        hint: d.hint,
        initial: d.initial,
        avatar: d.avatar.unwrap_or_default(),
        has_avatar: d.has_avatar,
    }
}

#[cfg(feature = "demo")]
mod probe_dump {
    use slint::{ComponentHandle, Model};

    use super::generated::{
        MediaFailure, MediaState, MessageKind, PreviewKind, SendState, ServiceKind,
    };
    use super::{
        AppWindow, IntProp, MessageEntry, ReactionEntry, RoomView, StringProp, TIMELINE_MODEL,
        UiProps,
    };
    use crate::adapters::ui::dump::{ReactionRowDump, TimelineDump, TimelineRowDump};
    use crate::adapters::ui::schema::{
        enum_names, media_failures, media_states, message_kinds, preview_kinds, send_states,
        service_kinds,
    };

    message_kinds!(enum_names slint message_kind MessageKind;);
    preview_kinds!(enum_names slint preview_kind PreviewKind;);
    service_kinds!(enum_names slint service_kind ServiceKind;);
    media_states!(enum_names slint media_state MediaState;);
    media_failures!(enum_names slint media_failure MediaFailure;);
    send_states!(enum_names slint send_state SendState;);

    fn reaction(entry: &ReactionEntry) -> ReactionRowDump {
        ReactionRowDump {
            key: entry.key.to_string(),
            label: entry.label.to_string(),
            count: entry.count,
            mine: entry.mine,
            pending: entry.pending,
            overflow: entry.overflow,
            hidden_reactors: entry.hidden_reactors,
        }
    }

    fn row(index: usize, entry: &MessageEntry) -> TimelineRowDump {
        TimelineRowDump {
            row: index,
            unique_id: entry.unique_id.to_string(),
            event_id: entry.event_id.to_string(),
            sender: entry.sender.to_string(),
            sender_id: entry.sender_id.to_string(),
            body: entry.body.to_string(),
            timestamp: entry.timestamp.to_string(),
            message_type: message_kind(entry.message_type),
            preview_kind: preview_kind(entry.preview_kind),
            service_kind: service_kind(entry.service_kind),
            service_target: entry.service_target.to_string(),
            media_state: media_state(entry.media_state),
            media_failure: media_failure(entry.media_failure),
            send_state: send_state(entry.send_state),
            send_progress: entry.send_progress,
            is_own: entry.is_own,
            edited: entry.edited,
            first_unread: entry.first_unread,
            needs_media: entry.needs_media,
            has_avatar: entry.has_avatar,
            image_width: entry.image_width,
            image_height: entry.image_height,
            duration: entry.duration.to_string(),
            has_reply: entry.has_reply,
            reply_event_id: entry.reply_event_id.to_string(),
            reply_sender: entry.reply_sender.to_string(),
            reply_body: entry.reply_body.to_string(),
            reactions: entry.reactions.iter().map(|r| reaction(&r)).collect(),
        }
    }

    pub fn collect(window: &AppWindow) -> TimelineDump {
        let view = window.global::<RoomView>();
        let rows = TIMELINE_MODEL
            .with(|cell| cell.borrow().clone())
            .map(|model| model.iter().enumerate().map(|(i, e)| row(i, &e)).collect())
            .unwrap_or_default();
        TimelineDump {
            selected_room_id: window.get_string(StringProp::SelectedRoomId).to_string(),
            selected_room_name: view.get_selected_room_name().to_string(),
            generation: window.get_int(IntProp::SelectedGeneration),
            timeline_token: view.get_timeline_token(),
            prepend_token: view.get_prepend_token(),
            anchor_index: view.get_anchor_index(),
            focus_event_id: view.get_focus_event_id().to_string(),
            rows,
        }
    }
}
