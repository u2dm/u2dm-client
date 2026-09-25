use std::rc::Rc;
use std::sync::Arc;

use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, SharedString, StyledText,
    VecModel,
};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
#[cfg(feature = "demo")]
use u2dm_ui::Probe;
use u2dm_ui::{
    Actions, AppWindow, AttachmentKind as UiAttachmentKind, AttachmentView,
    AudioKind as UiAudioKind, AudioView, ChildAccess as UiChildAccess, ConnectionState,
    Delivery as UiDelivery, DirectoryView, EmojiEntry, EmojiGroup, EmojiInsert, EmojiStore,
    LoginActivity as UiLoginActivity, LoginMethodKind as UiLoginMethodKind, LoginPhase, LoginView,
    MediaFailure as UiMediaFailure, MediaState as UiMediaState, MessageEntry,
    MessageKind as UiMessageKind, PollAnswerEntry, PollPhase as UiPollPhase,
    PreviewKind as UiPreviewKind, ReactionEntry, ReactionSend as UiReactionSend, ReactorAvatar,
    ReplySwipe, RoomEntry, RoomScope as UiRoomScope, RoomView, SendState as UiSendState,
    ServiceKind as UiServiceKind, SessionView, SpaceChildEntry, SpaceEntry,
    SpaceIndexStatus as UiSpaceIndexStatus, SpaceIndexView, StickerCell, StickerPackTab,
    StickerRow, StickerView, TimelineState, UnsentView, UserMessage as UiUserMessage,
    UserMessageKind as UiUserMessageKind, VerificationActivity as UiVerificationActivity,
    VerificationEmoji, VerificationPhase, VerificationView, VideoView, WindowView,
};

use super::backend::{self, Models, UiBackend, reorder_spaces, selected_room_key, unread_below};
use super::decode::{AvatarSlot, request_avatar, request_media, request_sticker};
use super::dto::{
    MediaFailureKind, MediaState, MessageDto, PollAnswerDto, ReactionDto, ReactorAvatarDto,
    RoomDto, SpaceChildDto, SpaceDto, StickerCellDto, StickerPackDto, StickerRowDto,
};
#[cfg(feature = "demo")]
use super::dump;
use super::fields::{
    MessageFields, PollAnswerFields, ReactionFields, ReactorFields, RoomFields, SpaceChildFields,
    SpaceFields, StickerCellFields, StickerPackFields, StickerRowFields,
};
use super::present::{Delivery, MessageKind, PollPhase, ServiceKind, VerifyStep};
#[cfg(feature = "demo")]
use super::props::EnumProp;
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::schema::{
    attachment_kinds, audio_kinds, bool_props, child_accesses, connection_states, deliveries,
    enum_props, int_props, login_activities, login_methods, login_phases, media_failures,
    media_states, message_fields, message_kinds, model_props, poll_answer_fields, poll_phases,
    preview_kinds, reaction_fields, reaction_sends, reactor_fields, room_fields, room_scopes,
    send_states, service_kinds, simple_callbacks, space_child_fields, space_fields,
    space_index_statuses, sticker_cell_fields, sticker_pack_fields, sticker_row_fields,
    string_props, timeline_states, user_message_kinds, verification_activities,
    verification_phases,
};
use super::session::active_models;
use super::video::{self, millis_to_duration};
use super::{audio, emoji, router};
use crate::app::input::CommandSender;
use crate::commands::effects::{Effect, VerificationActivity};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::{TimelineVisibility, ViewportChanged};
use crate::commands::view::{
    AppViewState, AttachmentKind, ChildAccess, LoginActivity, LoginStep, RoomScope,
    SpaceIndexStatus,
};
use crate::domain::auth::LoginMethod;
use crate::domain::media::AudioKind;
use crate::domain::message::{MessagePreviewKind, ReactionSend, SendState};
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::TimelineStatus;
use crate::domain::verification::VerificationEmoji as DomainVerificationEmoji;
use crate::error::Result;
use crate::ports::media::MediaCache;

fn actions(window: &AppWindow) -> Actions<'_> {
    window.global::<Actions>()
}

macro_rules! impl_prop_setter {
    ($fn:ident $enum:ident $ty:ty;
        $($(#[$attr:meta])* $v:ident $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        fn $fn(&self, prop: $enum, value: $ty) {
            match prop { $($(#[$attr])* $enum::$v => self.global::<$g>().$s(value),)* }
        }
    };
}

macro_rules! impl_prop_getter {
    ($fn:ident $enum:ident $ty:ty;
        $($(#[$attr:meta])* $v:ident $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        fn $fn(&self, prop: $enum) -> $ty {
            match prop { $($(#[$attr])* $enum::$v => self.global::<$g>().$get(),)* }
        }
    };
}

macro_rules! impl_enum_setters {
    ($($v:ident $fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        $( fn $fn(&self, value: $ty) { self.global::<$g>().$s(value.slint()); } )*
    };
}

#[cfg(feature = "demo")]
macro_rules! impl_enum_getter {
    ($($v:ident $fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        fn get_enum(&self, prop: EnumProp) -> SharedString {
            match prop { $(EnumProp::$v => self.global::<$g>().$get().slint_name().into(),)* }
        }
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
            router::$fn(&tx, $(bind_compiled_callbacks!(@field req $field $field_kind)),*)
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
    (@field $req:ident $field:ident list) => { $req.$field.iter().map(String::from).collect() };
}

impl UiProps for AppWindow {
    string_props!(impl_prop_setter set_string StringProp SharedString;);
    bool_props!(impl_prop_setter set_bool BoolProp bool;);
    int_props!(impl_prop_setter set_int IntProp i32;);
    enum_props!(impl_enum_setters);
    string_props!(impl_prop_getter get_string StringProp SharedString;);
    int_props!(impl_prop_getter get_int IntProp i32;);
    bool_props!(impl_prop_getter get_bool BoolProp bool;);
    #[cfg(feature = "demo")]
    enum_props!(impl_enum_getter);

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

trait SlintName {
    fn slint_name(&self) -> &'static str;
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

        impl SlintName for $dst {
            fn slint_name(&self) -> &'static str {
                match self { $($dst::$ui => $lit,)* }
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
deliveries!(impl_slint_enum Delivery UiDelivery;);
media_failures!(impl_slint_enum MediaFailureKind UiMediaFailure;);
message_kinds!(impl_slint_enum MessageKind UiMessageKind;);
poll_phases!(impl_slint_enum PollPhase UiPollPhase;);
attachment_kinds!(impl_slint_enum AttachmentKind UiAttachmentKind;);
room_scopes!(impl_slint_enum RoomScope UiRoomScope;);
space_index_statuses!(impl_slint_enum SpaceIndexStatus UiSpaceIndexStatus;);
child_accesses!(impl_slint_enum ChildAccess UiChildAccess;);
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

macro_rules! entry_accessors {
    ($f:ident $set:ident text) => {
        fn $f(&self) -> &str { self.$f.as_str() }
        fn $set(&mut self, value: SharedString) { self.$f = value; }
    };
    ($f:ident $set:ident enumk($ty:ident)) => {
        fn $f(&self) -> &str { self.$f.slint_name() }
        fn $set(&mut self, value: $ty) { self.$f = value.slint(); }
    };
    ($f:ident $set:ident list) => {
        fn $f(&self) -> Vec<SharedString> { self.$f.iter().collect() }
        fn $set(&mut self, value: Vec<SharedString>) { self.$f = string_model(value); }
    };
    ($f:ident $set:ident floats) => {
        fn $f(&self) -> Vec<f32> { self.$f.iter().collect() }
        fn $set(&mut self, value: Vec<f32>) { self.$f = ModelRc::new(VecModel::from(value)); }
    };
    ($f:ident $set:ident int) => { entry_accessors!(@copy $f $set i32); };
    ($f:ident $set:ident ratio) => { entry_accessors!(@copy $f $set f32); };
    ($f:ident $set:ident flag) => { entry_accessors!(@copy $f $set bool); };
    ($f:ident $set:ident image) => { entry_accessors!(@clone $f $set Image); };
    ($f:ident $set:ident styled) => { entry_accessors!(@clone $f $set StyledText); };
    ($f:ident $set:ident structs($row:ident)) => {
        entry_accessors!(@clone $f $set ModelRc<<CompiledBackend as UiBackend>::$row>);
    };
    (@copy $f:ident $set:ident $ty:ty) => {
        fn $f(&self) -> $ty { self.$f }
        fn $set(&mut self, value: $ty) { self.$f = value; }
    };
    (@clone $f:ident $set:ident $ty:ty) => {
        fn $f(&self) -> $ty { self.$f.clone() }
        fn $set(&mut self, value: $ty) { self.$f = value; }
    };
}

macro_rules! impl_entry {
    ($dto:ident $fields:ident $entry:ident;
        $($f:ident $set:ident $lit:literal $k:ident $(($arg:ident))?;)*) => {
        impl From<$dto> for $entry {
            fn from(d: $dto) -> Self {
                Self { $( $f: entry_field!(d.$f, $k), )* }
            }
        }

        impl $fields<CompiledBackend> for $entry {
            $( entry_accessors!($f $set $k $(($arg))?); )*
        }
    };
}

message_fields!(impl_entry MessageDto MessageFields MessageEntry;);
reaction_fields!(impl_entry ReactionDto ReactionFields ReactionEntry;);
reactor_fields!(impl_entry ReactorAvatarDto ReactorFields ReactorAvatar;);
poll_answer_fields!(impl_entry PollAnswerDto PollAnswerFields PollAnswerEntry;);
room_fields!(impl_entry RoomDto RoomFields RoomEntry;);
space_fields!(impl_entry SpaceDto SpaceFields SpaceEntry;);
space_child_fields!(impl_entry SpaceChildDto SpaceChildFields SpaceChildEntry;);
sticker_cell_fields!(impl_entry StickerCellDto StickerCellFields StickerCell;);
sticker_pack_fields!(impl_entry StickerPackDto StickerPackFields StickerPackTab;);
sticker_row_fields!(impl_entry StickerRowDto StickerRowFields StickerRow;);

macro_rules! attach_compiled_models {
    ($window:ident $models:ident;
        $($field:ident $row:ident $model:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        $( $window.global::<$g>().$s(ModelRc::from(Rc::clone(&$models.$field))); )*
    };
}

pub struct CompiledBackend;

impl UiBackend for CompiledBackend {
    type Window = AppWindow;
    type Message = MessageEntry;
    type Reaction = ReactionEntry;
    type Reactor = ReactorAvatar;
    type PollAnswer = PollAnswerEntry;
    type Room = RoomEntry;
    type Space = SpaceEntry;
    type SpaceChild = SpaceChildEntry;
    type StickerRow = StickerRow;
    type StickerCell = StickerCell;
    type StickerPack = StickerPackTab;

    fn models() -> Rc<Models<Self>> {
        active_models()
    }

    fn attach_models(window: &AppWindow, models: &Models<Self>) {
        model_props!(attach_compiled_models window models;);
    }

    fn bind_sticker_search(window: &AppWindow, search: impl Fn(&str) + 'static) {
        actions(window).on_search_stickers(move |query| search(&query));
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
        visibility_tx: &watch::Sender<TimelineVisibility>,
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

        let tx = cmd_tx.clone();
        actions(win).on_dismiss_unsent(move |submission| router::dismiss_unsent(&tx, submission));

        let tx = cmd_tx.clone();
        actions(win)
            .on_paste_attachment(move |room_id| router::paste_attachment(&tx, room_id.to_string()));

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
        actions(win).on_scroll_position_changed(move |at_bottom, first_unseen_row| {
            router::scroll_position(
                &scroll_tx,
                selected_room_key::<CompiledBackend>(&weak),
                at_bottom,
                unread_below::<CompiledBackend>(first_unseen_row),
            );
        });

        let visibility_tx = visibility_tx.clone();
        actions(win).on_timeline_visibility_changed(move |visible| {
            router::timeline_visibility(&visibility_tx, visible);
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
    dump::install_probe::<CompiledBackend>(&ui.window);
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
