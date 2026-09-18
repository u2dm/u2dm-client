use std::rc::Rc;
use std::sync::Arc;

use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

use super::backend::{self, Models, UiBackend, reorder_spaces, selected_room_key};
use super::decode::{AvatarSlot, request_avatar, request_media, request_sticker};
use super::dto::{
    AudioRowUpdate, MediaFailureKind, MediaState, MessageDto, ReactionDto, ReactorAvatarDto,
    RoomDto, SpaceDto, StickerCellDto, StickerPackDto, StickerRowDto, ThumbUpdate,
    enrich_to_update,
};
#[cfg(feature = "demo")]
use super::dump;
use super::present::{MessageKind, ServiceKind, VerifyStep};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::schema::{
    attachment_kinds, audio_kinds, bool_props, connection_states, enum_props, int_props,
    login_activities, login_methods, login_phases, media_failures, media_states, message_fields,
    message_kinds, model_props, preview_kinds, reaction_fields, reaction_sends, reactor_fields,
    room_fields, send_states, service_kinds, simple_callbacks, space_fields, sticker_cell_fields,
    sticker_pack_fields, sticker_row_fields, string_props, timeline_states, user_message_kinds,
    verification_activities, verification_phases,
};
use super::video::{self, millis_to_duration};
use super::{audio, emoji, router};
use crate::app::input::CommandSender;
use crate::commands::effects::{Effect, VerificationActivity};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::ViewportChanged;
use crate::commands::view::{AppViewState, AttachmentKind, LoginActivity, LoginStep};
use crate::domain::auth::LoginMethod;
use crate::domain::media::AudioKind;
use crate::domain::message::{MessagePreviewKind, ReactionSend, SendState};
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
    Actions, AppWindow, AttachmentKind as UiAttachmentKind, AttachmentView,
    AudioKind as UiAudioKind, AudioView, ConnectionState, DirectoryView, EmojiEntry, EmojiGroup,
    EmojiInsert, EmojiStore, LoginActivity as UiLoginActivity,
    LoginMethodKind as UiLoginMethodKind, LoginPhase, LoginView, MediaFailure as UiMediaFailure,
    MediaState as UiMediaState, MessageEntry, MessageKind as UiMessageKind,
    PreviewKind as UiPreviewKind, ReactionEntry, ReactionSend as UiReactionSend, ReactorAvatar,
    RoomEntry, RoomView, SendState as UiSendState, ServiceKind as UiServiceKind, SessionView,
    SpaceEntry, StickerCell, StickerPackTab, StickerRow, StickerView, TimelineState,
    UserMessage as UiUserMessage, UserMessageKind as UiUserMessageKind,
    VerificationActivity as UiVerificationActivity, VerificationEmoji, VerificationPhase,
    VerificationView, VideoView,
};

fn actions(window: &AppWindow) -> Actions<'_> {
    window.global::<Actions>()
}

macro_rules! impl_prop_setter {
    ($fn:ident $enum:ident $ty:ty; $($(#[$attr:meta])* $v:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        fn $fn(&self, prop: $enum, value: $ty) {
            match prop { $($(#[$attr])* $enum::$v => self.global::<$g>().$s(value),)* }
        }
    };
}

macro_rules! impl_enum_setters {
    ($($fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        $( fn $fn(&self, value: $ty) { self.global::<$g>().$s(value.slint()); } )*
    };
}

macro_rules! bind_compiled_callbacks {
    ($win:ident $tx:ident;
        $($on:ident $lit:literal $fn:ident $kind:ident $(($($arg:tt)*))? $cmd:ident;)*) => {
        $( bind_compiled_callbacks!(@one $win $tx $on $fn $kind $(($($arg)*))?); )*
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
    (@one $win:ident $tx:ident $on:ident $fn:ident room_key) => {{
        let tx = $tx.clone();
        let weak = $win.as_weak();
        actions($win).$on(move || router::$fn(&tx, selected_room_key::<CompiledBackend>(&weak)));
    }};
    (@one $win:ident $tx:ident $on:ident $fn:ident
        request($($field:ident $name:literal $field_kind:ident),*)) => {{
        let tx = $tx.clone();
        actions($win).$on(move |req| {
            router::$fn(&tx, $(bind_compiled_callbacks!(@field req $field $field_kind)),*);
        });
    }};
    (@unit $win:ident $tx:ident $on:ident $fn:ident) => {{
        let tx = $tx.clone();
        actions($win).$on(move || router::$fn(&tx));
    }};
    (@string $win:ident $tx:ident $on:ident $fn:ident) => {{
        let tx = $tx.clone();
        actions($win).$on(move |arg| router::$fn(&tx, arg.to_string()));
    }};
    (@field $req:ident $field:ident text) => { $req.$field.to_string() };
    (@field $req:ident $field:ident flag) => { $req.$field };
}

impl UiProps for AppWindow {
    string_props!(impl_prop_setter set_string StringProp SharedString;);
    bool_props!(impl_prop_setter set_bool BoolProp bool;);
    int_props!(impl_prop_setter set_int IntProp i32;);
    enum_props!(impl_enum_setters);

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
                kind: m.kind.slint(),
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

trait SlintEnum {
    type Slint;
    fn slint(&self) -> Self::Slint;
}

macro_rules! impl_slint_enum {
    ($src:ident $dst:ident;
        $($rust:ident $(($($p:tt)*))? $({$($b:tt)*})? $ui:ident $lit:literal;)*) => {
        impl SlintEnum for $src {
            type Slint = $dst;
            fn slint(&self) -> $dst {
                match self { $($src::$rust $(($($p)*))? $({$($b)*})? => $dst::$ui,)* }
            }
        }
    };
}

login_phases!(impl_slint_enum LoginStep LoginPhase;);
login_activities!(impl_slint_enum LoginActivity UiLoginActivity;);
login_methods!(impl_slint_enum LoginMethod UiLoginMethodKind;);
connection_states!(impl_slint_enum ConnectionStatus ConnectionState;);
timeline_states!(impl_slint_enum TimelineStatus TimelineState;);
verification_phases!(impl_slint_enum VerifyStep VerificationPhase;);
verification_activities!(impl_slint_enum VerificationActivity UiVerificationActivity;);
user_message_kinds!(impl_slint_enum UserMessageKind UiUserMessageKind;);
media_states!(impl_slint_enum MediaState UiMediaState;);
send_states!(impl_slint_enum SendState UiSendState;);
reaction_sends!(impl_slint_enum ReactionSend UiReactionSend;);
media_failures!(impl_slint_enum MediaFailureKind UiMediaFailure;);
message_kinds!(impl_slint_enum MessageKind UiMessageKind;);
attachment_kinds!(impl_slint_enum AttachmentKind UiAttachmentKind;);
preview_kinds!(impl_slint_enum MessagePreviewKind UiPreviewKind;);
audio_kinds!(impl_slint_enum AudioKind UiAudioKind;);
service_kinds!(impl_slint_enum ServiceKind UiServiceKind;);

fn string_model(items: Vec<SharedString>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items))
}

fn entry_model<T, E: From<T> + Clone + 'static>(items: Vec<T>) -> ModelRc<E> {
    ModelRc::new(items.into_iter().map(E::from).collect::<VecModel<E>>())
}

macro_rules! entry_field {
    ($val:expr, text) => {
        $val
    };
    ($val:expr, int) => {
        $val
    };
    ($val:expr, ratio) => {
        $val
    };
    ($val:expr, flag) => {
        $val
    };
    ($val:expr, styled) => {
        $val
    };
    ($val:expr, list) => {
        string_model($val)
    };
    ($val:expr, floats) => {
        ModelRc::new(VecModel::from($val))
    };
    ($val:expr, structs) => {
        entry_model($val)
    };
    ($val:expr, image) => {
        $val.unwrap_or_default()
    };
    ($val:expr, enumk) => {
        $val.slint()
    };
}

macro_rules! impl_entry_from {
    ($dto:ident $entry:ident; $($f:ident $c:ident $lit:literal $k:ident;)*) => {
        impl From<$dto> for $entry {
            fn from(d: $dto) -> Self {
                Self { $( $f: entry_field!(d.$f, $k), )* }
            }
        }
    };
}

message_fields!(impl_entry_from MessageDto MessageEntry;);
reaction_fields!(impl_entry_from ReactionDto ReactionEntry;);
reactor_fields!(impl_entry_from ReactorAvatarDto ReactorAvatar;);
room_fields!(impl_entry_from RoomDto RoomEntry;);
space_fields!(impl_entry_from SpaceDto SpaceEntry;);
sticker_cell_fields!(impl_entry_from StickerCellDto StickerCell;);
sticker_pack_fields!(impl_entry_from StickerPackDto StickerPackTab;);
sticker_row_fields!(impl_entry_from StickerRowDto StickerRow;);

macro_rules! attach_compiled_models {
    ($window:ident $models:ident;
        $($field:ident $row:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        $( $window.global::<$g>().$s(ModelRc::from(Rc::clone(&$models.$field))); )*
    };
}

pub struct CompiledBackend;

impl UiBackend for CompiledBackend {
    type Window = AppWindow;
    type Message = MessageEntry;
    type Room = RoomEntry;
    type Space = SpaceEntry;
    type StickerRow = StickerRow;
    type StickerPack = StickerPackTab;

    fn attach_models(window: &AppWindow, models: &Models<Self>) {
        model_props!(attach_compiled_models window models;);
    }

    fn bind_sticker_search(window: &AppWindow, search: impl Fn(&str) + 'static) {
        actions(window).on_search_stickers(move |query| search(&query));
    }

    fn enrich_message(entry: &mut MessageEntry, delta: &EnrichmentDelta, media: &dyn MediaCache) {
        let update = enrich_to_update(delta, media);
        match update.thumbnail {
            ThumbUpdate::Ready(img) => {
                entry.thumbnail = img;
                entry.media_state = UiMediaState::Ready;
            }
            ThumbUpdate::Failed(reason) => {
                entry.media_state = UiMediaState::Failed;
                entry.media_failure = reason.slint();
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
        entry.media_failure = reason.slint();
    }

    fn set_message_audio(entry: &mut MessageEntry, update: &AudioRowUpdate) {
        entry.media_state = update.media_state.slint();
        entry.media_failure = update.media_failure.slint();
        entry.waveform = ModelRc::new(VecModel::from(update.waveform.clone()));
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

    fn bind_audio_callbacks(win: &AppWindow, cmd_tx: &CommandSender) {
        audio::install_commands(cmd_tx);

        let weak = win.as_weak();
        actions(win).on_toggle_audio(move || {
            if let Some(window) = weak.upgrade() {
                audio::toggle(&window);
            }
        });

        let weak = win.as_weak();
        actions(win).on_seek_audio(move |ms| {
            if let Some(window) = weak.upgrade() {
                audio::seek(&window, millis_to_duration(usize::try_from(ms).ok()));
            }
        });
    }

    #[allow(clippy::unnecessary_wraps, reason = "mirrors the fallible interpreted adapter")]
    pub fn register_callbacks(
        &self,
        cmd_tx: &CommandSender,
        scroll_tx: &watch::Sender<ViewportChanged>,
    ) -> Result<()> {
        setup_emoji_store(&self.window);

        let win = &self.window;
        simple_callbacks!(bind_compiled_callbacks win cmd_tx;);

        let tx = cmd_tx.clone();
        actions(win).on_move_space(move |from, to| {
            let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                return;
            };
            router::move_space(&tx, from, to, reorder_spaces::<CompiledBackend>);
        });

        actions(win).on_request_media(move |unique_id| request_media(&unique_id));

        Self::bind_video_callbacks(win);
        Self::bind_audio_callbacks(win, cmd_tx);

        actions(win).on_request_room_avatar(move |room_id| {
            request_avatar(&AvatarSlot::Room(room_id.to_string()));
        });

        actions(win).on_request_sticker(move |key| request_sticker(&key));

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

        Ok(())
    }

    pub fn spawn_event_handler(
        &self,
        ui_rx: mpsc::Receiver<Effect>,
        view_rx: watch::Receiver<Arc<AppViewState>>,
        media_cache: Arc<dyn MediaCache>,
    ) {
        backend::spawn_event_handler::<CompiledBackend>(&self.window, ui_rx, view_rx, media_cache);
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
    pub fn prefer_silent_audio() {
        audio::prefer_silent();
    }

    #[cfg(feature = "demo")]
    pub fn enable_probe_introspection(&self) {
        self.window.set_bool(BoolProp::ProbeEnabled, true);
    }
}

#[cfg(feature = "demo")]
pub fn install_timeline_dump(ui: &SlintUiAdapter) {
    let weak = ui.window.as_weak();
    let dump_weak = weak.clone();
    dump::install(Box::new(move |reply| {
        let handle = dump_weak.clone();
        let queued = handle.upgrade_in_event_loop(move |window| {
            drop(reply.send(probe_dump::collect(&window)));
        });
        if let Err(e) = queued {
            tracing::debug!("the timeline dump could not reach the event loop: {e}");
        }
    }));
    dump::install_pokes(Box::new(move |poke| {
        let queued = weak.upgrade_in_event_loop(move |window| match poke {
            dump::Poke::ToggleAudio => audio::toggle(&window),
            dump::Poke::SeekAudio(position) => audio::seek(&window, position),
        });
        if let Err(e) = queued {
            tracing::debug!("a probe poke could not reach the event loop: {e}");
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

#[cfg(feature = "demo")]
mod probe_dump {
    use slint::{ComponentHandle, Model};

    use super::generated::{
        AudioKind, AudioView, MediaFailure, MediaState, MessageKind, PreviewKind, ReactionSend,
        SendState, ServiceKind,
    };
    use super::{
        AppWindow, CompiledBackend, IntProp, MessageEntry, ReactionEntry, RoomView, StringProp,
        UiBackend, UiProps,
    };
    use crate::adapters::ui::dump::{AudioDump, ReactionRowDump, TimelineDump, TimelineRowDump};
    use crate::adapters::ui::schema::{
        audio_kinds, enum_names, media_failures, media_states, message_kinds, preview_kinds,
        reaction_sends, send_states, service_kinds,
    };

    message_kinds!(enum_names slint message_kind MessageKind;);
    preview_kinds!(enum_names slint preview_kind PreviewKind;);
    service_kinds!(enum_names slint service_kind ServiceKind;);
    media_states!(enum_names slint media_state MediaState;);
    media_failures!(enum_names slint media_failure MediaFailure;);
    send_states!(enum_names slint send_state SendState;);
    reaction_sends!(enum_names slint reaction_send ReactionSend;);
    audio_kinds!(enum_names slint audio_kind AudioKind;);

    fn reaction(entry: &ReactionEntry) -> ReactionRowDump {
        ReactionRowDump {
            key: entry.key.to_string(),
            label: entry.label.to_string(),
            count: entry.count,
            mine: entry.mine,
            send: reaction_send(entry.send),
            overflow: entry.overflow,
            hidden_reactors: entry.hidden_reactors,
        }
    }

    fn row(index: usize, entry: &MessageEntry) -> TimelineRowDump {
        TimelineRowDump {
            row: index,
            unique_id: entry.unique_id.to_string(),
            local_id: entry.local_id.to_string(),
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
            filename: entry.filename.to_string(),
            size: entry.size.to_string(),
            audio_kind: audio_kind(entry.audio_kind),
            waveform: entry.waveform.iter().collect(),
            has_reply: entry.has_reply,
            reply_event_id: entry.reply_event_id.to_string(),
            reply_sender: entry.reply_sender.to_string(),
            reply_body: entry.reply_body.to_string(),
            reactions: entry.reactions.iter().map(|r| reaction(&r)).collect(),
        }
    }

    fn audio(window: &AppWindow) -> AudioDump {
        let view = window.global::<AudioView>();
        let state = match (view.get_visible(), view.get_loading(), view.get_playing()) {
            (false, _, _) => "hidden",
            (true, true, _) => "loading",
            (true, false, true) => "playing",
            (true, false, false) => "paused",
        };
        AudioDump {
            state,
            output: if view.get_silent() {
                "silent"
            } else {
                "device"
            },
            event_id: view.get_event_id().to_string(),
            room_id: view.get_room_id().to_string(),
            kind: audio_kind(view.get_kind()),
            sender: view.get_sender().to_string(),
            title: view.get_title().to_string(),
            position_ms: view.get_position_ms(),
            duration_ms: view.get_duration_ms(),
        }
    }

    pub fn collect(window: &AppWindow) -> TimelineDump {
        let view = window.global::<RoomView>();
        let rows = CompiledBackend::with_timeline(|model| {
            model
                .iter()
                .enumerate()
                .map(|(i, e)| row(i, &e))
                .collect::<Vec<_>>()
        })
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
            audio: audio(window),
        }
    }
}
