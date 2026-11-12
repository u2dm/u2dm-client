use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, StyledText, VecModel};
#[cfg(feature = "video")]
use slint::{Rgb8Pixel, SharedPixelBuffer};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use u2dm_ui::{
    AboutView, AccountView, Actions, AppWindow, AppearanceView, AttachmentView, AudioView,
    BodyLine, BodyPiece, CallConnectionEntry, CallDeviceEntry, CallPersonEntry, CallTileEntry,
    CallView, CallWindow, ComposerDraft, CreateRoomView, DirectoryView,
    DraftVerdict as UiDraftVerdict, EmojiEntry, EmojiRow, EmojiStore, EncryptionView, FocusKeeper,
    IdentityAlertView, IgnoredPersonEntry, InvitePeopleView, InviteView, InviteeEntry,
    KeywordEntry, LeaveSpaceView, LogLineEntry, LoginView, LogoutView, MemberEntry, MentionWord,
    MentionsView, MessageEntry, Motion, NotificationText, PackCell, PackRow, PackTabEntry,
    PackView, PollAnswerEntry, PresenceView, ReactionEntry, ReactorAvatar, ReadersView,
    RemovalView, ReplySwipe, RoomEntry, RoomInfoView, RoomLogView, RoomMenuView, RoomPreviewView,
    RoomView, SessionView, SettingsView, SourceView, SpaceChildEntry, SpaceEntry, SpaceIndexView,
    SpoilerCover, TextSplice, TouchpadView, TypingView, UnsentView, UpgradeView, UserDeviceEntry,
    UserInfoView, UserMessage as UiUserMessage, VerificationEmoji, VerificationView, VideoView,
    WindowView,
};
#[cfg(feature = "demo")]
use u2dm_ui::{Probe, RoomMenu, WindowTitle};

use super::backend::{
    self, Models, RECENT_SPEAKERS, UiBackend, adopted_room_key, current_edited_row,
    last_editable_row, on_main, recent_speakers, reorder_spaces, row_of_message, selected_room_key,
    shown_room_log_text, unread_below,
};
use super::call_window::{self, WindowHooks};
use super::decode::{
    AvatarSlot, request_avatar, request_custom_emoji, request_media, request_pack_cell,
};
use super::dto::{
    BodyLineDto, BodyPieceDto, CallConnectionDto, CallDeviceDto, CallNoticeMark, CallOfferMark,
    CallPersonDto, CallScreenMark, CallTileDto, CallTileMark, EmojiArt, IgnoredPersonDto,
    InviteeDto, KeywordDto, LogLineDto, MediaFailureKind, MediaState, MemberRowDto, MemberRowKind,
    MessageDto, PackCellDto, PackRowDto, PackSurface, PackTabDto, PollAnswerDto, ReactionDto,
    ReactorAvatarDto, RoomCallMark, RoomDto, RoomRowKind, SheetPreviewMark, SpaceChildDto,
    SpaceDto, SpoilerState, TestPhaseMark, UserDeviceDto, pack_surface_named,
};
#[cfg(feature = "demo")]
use super::dump;
use super::fields::{
    BodyLineFields, BodyPieceFields, CallConnectionFields, CallDeviceFields, CallPersonFields,
    CallTileFields, IgnoredPersonFields, InviteeFields, KeywordFields, LogLineFields,
    MemberRowFields, MessageFields, PackCellFields, PackRowFields, PackTabFields, PollAnswerFields,
    ReactionFields, ReactorFields, RoomFields, SpaceChildFields, SpaceFields, UserDeviceFields,
};
use super::notices::NoticeWords;
use super::present::{
    Delivery, IdentityAlertKind, MessageKind, PollPhase, ServiceKind, ShieldReason, VerifyStep,
};
#[cfg(feature = "demo")]
use super::props::EnumProp;
use super::props::{BoolProp, ImageProp, IntProp, StringProp, UiProps};
use super::router::{DraftVerdict, EditedRow, EditedText};
use super::schema::{
    body_line_fields, body_piece_fields, bool_props, call_connection_fields, call_device_fields,
    call_person_fields, call_tile_fields, enum_props, ignored_person_fields, image_props,
    int_props, invitee_fields, keyword_fields, log_line_fields, member_row_fields, message_fields,
    model_props, pack_cell_fields, pack_row_fields, pack_tab_fields, poll_answer_fields,
    reaction_fields, reactor_fields, room_fields, simple_callbacks, slint_enums,
    space_child_fields, space_fields, string_props, user_device_fields,
};
use super::session::active_models;
use super::video::{self, millis_to_duration};
use super::{about, audio, drafts, emoji, emote, mention, motion, reduce, router, settings_search};
use crate::app::input::CommandSender;
use crate::commands::effects::{Effect, VerificationActivity};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::{ComposerActivity, TimelineVisibility, ViewportChanged, WindowAttention};
use crate::commands::view::{
    AccountActivity, AccountRule, AccountStep, AppViewState, AttachmentKind, AuthStage, CallPhase,
    CameraView, ChildAccess, CreateRoomStatus, DeviceStatus, DeviceTrouble, DirectChat,
    DownloadedMedia, EncryptionActivity, EncryptionStep, HandView, InviteeState, JoinRefusal,
    LinkView, LoginActivity, LoginStep, MicrophoneView, PendingAnswer, PendingModeration,
    PeopleSearchStatus, PreviewTrouble, ReactionView, ReadersStatus, RoomInfoPlacement,
    RoomPreviewSheet, RoomScope, RosterStatus, SettingsLayout, SettingsPage, SourceState,
    SpaceIndexStatus, UpgradeLink, VerifyOffer,
};
use crate::domain::auth::LoginMethod;
use crate::domain::call::devices::DeviceKind;
use crate::domain::call::video::ScreenRefusal;
use crate::domain::departure::RemovalKind;
use crate::domain::encryption::{Recovery, Standing};
use crate::domain::image_pack::PackOrigin;
use crate::domain::invite::InviteKind;
use crate::domain::media::AudioKind;
use crate::domain::message::{
    DecryptionFailure, MessagePreviewKind, ReactionSend, SendState, Shield,
};
use crate::domain::presence::PresenceLine;
use crate::domain::room::NotifyMode;
use crate::domain::room_creation::{RoomAccess, RoomKind};
use crate::domain::room_info::MemberRole;
use crate::domain::room_log::LogLevel;
use crate::domain::server_rules::{RoomClass, RoomDefault};
use crate::domain::settings::quick_reactions::QuickReactions;
use crate::domain::settings::{
    CallLayout, ClockChoice, MotionChoice, ReceiptVisibility, SendKey, SettingFlag, Settings,
    ThemeChoice,
};
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::{SourceEncryption, TimelineStatus};
use crate::domain::user_info::{DeviceTrust, IdentityTrust, RoomMembership};
use crate::domain::verification::VerificationEmoji as DomainVerificationEmoji;
use crate::error::Result;
use crate::ports::appearance::AppearancePort;
use crate::ports::call::CallVideoPort;
use crate::ports::handoff::HandoffPort;
use crate::ports::media::MediaCache;
use crate::ports::notifier::NotifierPort;

fn draft_verdict(verdict: &DraftVerdict) -> UiDraftVerdict {
    UiDraftVerdict {
        accepted: verdict.accepted(),
        shorten_by: verdict.shorten_by(),
    }
}

fn actions(window: &AppWindow) -> Actions<'_> {
    window.global::<Actions>()
}

fn call_actions(window: &CallWindow) -> Actions<'_> {
    window.global::<Actions>()
}

macro_rules! impl_prop_setter {
    ($fn:ident $enum:ident $ty:ty;
        $($(#[$attr:meta])* $v:ident $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        fn $fn(&self, prop: $enum, value: $ty) {
            match prop { $($(#[$attr])* $enum::$v => {
                if call_window::mirrors($gname) {
                    call_window::mirror(&value, |window, value| window.global::<$g>().$s(value));
                }
                self.global::<$g>().$s(value)
            },)* }
        }
    };
}

macro_rules! impl_image_setter {
    ($($v:ident $g:ident $gname:literal
        $image:literal $set_image:ident $has:literal $set_has:ident;)*) => {
        fn set_image(&self, prop: ImageProp, image: Option<Image>) {
            match prop { $(ImageProp::$v => {
                let view = self.global::<$g>();
                match image {
                    Some(image) => {
                        view.$set_image(image);
                        view.$set_has(true);
                    }
                    None => view.$set_has(false),
                }
            })* }
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
        $( fn $fn(&self, value: $ty) {
            let value = value.slint();
            if call_window::mirrors($gname) {
                call_window::mirror(&value, |window, value| window.global::<$g>().$s(value));
            }
            self.global::<$g>().$s(value);
        } )*
    };
}

macro_rules! mirror_props {
    ($name:ident;
        $($(#[$attr:meta])* $v:ident $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        fn $name(from: &AppWindow, to: &CallWindow) {
            $( $(#[$attr])* if call_window::mirrors($gname) {
                to.global::<$g>().$s(from.global::<$g>().$get());
            } )*
        }
    };
}

macro_rules! mirror_enum_props {
    ($name:ident;
        $($v:ident $fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        mirror_props!($name; $($v $g $gname $lit $s $get;)*);
    };
}

string_props!(mirror_props mirror_strings;);
bool_props!(mirror_props mirror_bools;);
int_props!(mirror_props mirror_ints;);
enum_props!(mirror_enum_props mirror_enums;);

#[cfg(feature = "demo")]
macro_rules! impl_enum_getter {
    ($($v:ident $fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        fn get_enum(&self, prop: EnumProp) -> SharedString {
            match prop { $(EnumProp::$v => self.global::<$g>().$get().slint_name().into(),)* }
        }
    };
}

macro_rules! bind_compiled_callbacks {
    ($actions:expr, $main:ident, $tx:ident;
        $($on:ident $lit:literal $fn:ident $kind:ident $(($($arg:tt)*))?
            $cmd:ident $(:: $variant:ident)?;)*) => {
        $( bind_compiled_callbacks!(@one $actions, $main, $tx, $on $fn $kind $(($($arg)*))?); )*
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident plain) => {
        bind_compiled_callbacks!(@unit $actions, $tx, $on $fn)
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident pass) => {
        bind_compiled_callbacks!(@string $actions, $tx, $on $fn)
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident room) => {
        bind_compiled_callbacks!(@string $actions, $tx, $on $fn)
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident opt_room) => {
        bind_compiled_callbacks!(@string $actions, $tx, $on $fn)
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident user) => {
        bind_compiled_callbacks!(@string $actions, $tx, $on $fn)
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident manual) => {
        bind_compiled_callbacks!(@unit $actions, $tx, $on $fn)
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident manual_string) => {
        bind_compiled_callbacks!(@string $actions, $tx, $on $fn)
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident event) => {
        bind_compiled_callbacks!(@string $actions, $tx, $on $fn)
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident local) => {
        bind_compiled_callbacks!(@string $actions, $tx, $on $fn)
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident
        flag($field:ident $name:literal)) => {
        bind_compiled_callbacks!(@one $actions, $main, $tx, $on $fn request($field $name flag))
    };
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident room_key) => {{
        let tx = $tx.clone();
        let weak = $main.as_weak();
        $actions.$on(move || router::$fn(&tx, selected_room_key::<CompiledBackend>(&weak)));
    }};
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident
        request($($field:ident $name:literal $field_kind:ident),*)) => {{
        let tx = $tx.clone();
        $actions.$on(move |req| {
            router::$fn(&tx, $(bind_compiled_callbacks!(@field req $field $field_kind)),*)
        });
    }};
    (@one $actions:expr, $main:ident, $tx:ident, $on:ident $fn:ident
        verdict($($field:ident $name:literal $field_kind:ident),*)) => {{
        let tx = $tx.clone();
        $actions.$on(move |req| {
            draft_verdict(&router::$fn(
                &tx,
                $(bind_compiled_callbacks!(@field req $field $field_kind)),*
            ))
        });
    }};
    (@unit $actions:expr, $tx:ident, $on:ident $fn:ident) => {{
        let tx = $tx.clone();
        $actions.$on(move || router::$fn(&tx));
    }};
    (@string $actions:expr, $tx:ident, $on:ident $fn:ident) => {{
        let tx = $tx.clone();
        $actions.$on(move |arg| router::$fn(&tx, arg.to_string()));
    }};
    (@field $req:ident $field:ident text) => { $req.$field.to_string() };
    (@field $req:ident $field:ident str) => { $req.$field.as_str() };
    (@field $req:ident $field:ident int) => { $req.$field };
    (@field $req:ident $field:ident flag) => { $req.$field };
    (@field $req:ident $field:ident list) => { $req.$field.iter().map(String::from).collect() };
    (@field $req:ident $field:ident variant) => { $req.$field.slint_name() };
}

fn bind_simple_callbacks(actions: &Actions<'_>, main: &AppWindow, tx: &CommandSender) {
    simple_callbacks!(bind_compiled_callbacks actions, main, tx;);
}

fn bind_hooks(actions: &Actions<'_>, hooks: WindowHooks) {
    let WindowHooks {
        call_tile_seen,
        call_media_shown,
        watch_microphone,
        watch_connections,
        show_call_window,
        close_call_window,
        show_call_chat,
        show_main_window,
    } = hooks;
    actions.on_call_tile_seen(move |feed, width, height, shown| {
        call_tile_seen(&feed, width, height, shown);
    });
    actions.on_call_media_shown(call_media_shown);
    actions.on_watch_microphone(move |request| watch_microphone(request.on));
    actions.on_watch_connections(move |request| watch_connections(request.on));
    actions.on_show_call_window(show_call_window);
    actions.on_close_call_window(close_call_window);
    actions.on_show_call_chat(show_call_chat);
    actions.on_show_main_window(show_main_window);
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

    #[cfg(feature = "video")]
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

    image_props!(impl_image_setter);

    fn apply_room_info_topic(&self, topic: StyledText) {
        self.global::<RoomInfoView>().set_topic_styled(topic);
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

    fn apply_quick_reactions(&self, reactions: &QuickReactions) {
        let keys = reactions
            .keys()
            .iter()
            .map(|key| SharedString::from(key.as_str()))
            .collect();
        self.global::<SettingsView>()
            .set_quick_reactions(string_model(keys));
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
            type Slint = u2dm_ui::$dst;
            fn slint(&self) -> u2dm_ui::$dst {
                match self { $($src::$rust $(($($p)*))? $({$($b)*})? => u2dm_ui::$dst::$ui,)* }
            }
        }

        impl SlintName for u2dm_ui::$dst {
            fn slint_name(&self) -> &'static str {
                match self { $(u2dm_ui::$dst::$ui => $lit,)* }
            }
        }
    };
}

slint_enums!(impl_slint_enum);

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
body_line_fields!(impl_entry BodyLineDto BodyLineFields BodyLine;);
body_piece_fields!(impl_entry BodyPieceDto BodyPieceFields BodyPiece;);
reaction_fields!(impl_entry ReactionDto ReactionFields ReactionEntry;);
reactor_fields!(impl_entry ReactorAvatarDto ReactorFields ReactorAvatar;);
poll_answer_fields!(impl_entry PollAnswerDto PollAnswerFields PollAnswerEntry;);
room_fields!(impl_entry RoomDto RoomFields RoomEntry;);
space_fields!(impl_entry SpaceDto SpaceFields SpaceEntry;);
space_child_fields!(impl_entry SpaceChildDto SpaceChildFields SpaceChildEntry;);
member_row_fields!(impl_entry MemberRowDto MemberRowFields MemberEntry;);
invitee_fields!(impl_entry InviteeDto InviteeFields InviteeEntry;);
log_line_fields!(impl_entry LogLineDto LogLineFields LogLineEntry;);
call_tile_fields!(impl_entry CallTileDto CallTileFields CallTileEntry;);
call_person_fields!(impl_entry CallPersonDto CallPersonFields CallPersonEntry;);
call_connection_fields!(impl_entry CallConnectionDto CallConnectionFields CallConnectionEntry;);
call_device_fields!(impl_entry CallDeviceDto CallDeviceFields CallDeviceEntry;);
user_device_fields!(impl_entry UserDeviceDto UserDeviceFields UserDeviceEntry;);
keyword_fields!(impl_entry KeywordDto KeywordFields KeywordEntry;);
ignored_person_fields!(impl_entry IgnoredPersonDto IgnoredPersonFields IgnoredPersonEntry;);
pack_cell_fields!(impl_entry PackCellDto PackCellFields PackCell;);
pack_tab_fields!(impl_entry PackTabDto PackTabFields PackTabEntry;);
pack_row_fields!(impl_entry PackRowDto PackRowFields PackRow;);

macro_rules! attach_compiled_models {
    ($window:ident $models:ident;
        $($field:ident $row:ident $model:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        $( $window.global::<$g>().$s(ModelRc::from(Rc::clone(&$models.$field))); )*
    };
}

macro_rules! attach_mirrored_models {
    ($($field:ident $row:ident $model:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        fn attach_mirrored_models(window: &CallWindow, models: &Models<CompiledBackend>) {
            $( if call_window::mirrors($gname) {
                window.global::<$g>().$s(ModelRc::from(Rc::clone(&models.$field)));
            } )*
        }
    };
}

model_props!(attach_mirrored_models);

pub struct CompiledBackend;

impl UiBackend for CompiledBackend {
    type Window = AppWindow;
    type CallWindow = CallWindow;
    type Message = MessageEntry;
    type BodyLine = BodyLine;
    type BodyPiece = BodyPiece;
    type Reaction = ReactionEntry;
    type Reactor = ReactorAvatar;
    type PollAnswer = PollAnswerEntry;
    type Room = RoomEntry;
    type Space = SpaceEntry;
    type SpaceChild = SpaceChildEntry;
    type MemberRow = MemberEntry;
    type Invitee = InviteeEntry;
    type LogLine = LogLineEntry;
    type CallTile = CallTileEntry;
    type CallPerson = CallPersonEntry;
    type CallConnection = CallConnectionEntry;
    type CallDevice = CallDeviceEntry;
    type UserDevice = UserDeviceEntry;
    type Keyword = KeywordEntry;
    type IgnoredPerson = IgnoredPersonEntry;
    type PackRow = PackRow;
    type PackCell = PackCell;
    type PackTab = PackTabEntry;
    type Draft = ComposerDraft;

    fn models() -> Rc<Models<Self>> {
        active_models()
    }

    fn attach_models(window: &AppWindow, models: &Models<Self>) {
        model_props!(attach_compiled_models window models;);
    }

    fn bind_browse_packs(window: &AppWindow, browse: impl Fn(PackSurface, &str, i32) + 'static) {
        actions(window).on_browse_packs(move |surface, query, emote_columns| {
            if let Some(surface) = pack_surface_named(surface.slint_name()) {
                browse(surface, &query, emote_columns);
            }
        });
    }

    fn bind_reveal_spoilers(window: &AppWindow, reveal: impl Fn(&str) + 'static) {
        actions(window).on_reveal_spoilers(move |unique_id| reveal(&unique_id));
    }

    fn bind_window_hooks(window: &AppWindow, hooks: WindowHooks) {
        bind_hooks(&actions(window), hooks);
    }

    fn bind_call_preview_seen(window: &AppWindow, seen: impl Fn(&str, i32, i32, bool) + 'static) {
        actions(window).on_call_preview_seen(move |feed, width, height, shown| {
            seen(&feed, width, height, shown);
        });
    }

    fn new_call_window(main: &AppWindow, hooks: WindowHooks) -> Option<CallWindow> {
        let window = CallWindow::new()
            .inspect_err(|e| tracing::warn!("the call window could not be built: {e}"))
            .ok()?;
        mirror_strings(main, &window);
        mirror_bools(main, &window);
        mirror_ints(main, &window);
        mirror_enums(main, &window);
        attach_mirrored_models(&window, &Self::models());
        if let Some(tx) = call_window::commands() {
            bind_simple_callbacks(&call_actions(&window), main, &tx);
        }
        bind_hooks(&call_actions(&window), hooks);
        window.set_ready(true);
        Some(window)
    }

    fn call_window() -> Option<CallWindow> {
        call_window::opened()
    }

    fn keep_call_window(window: Option<CallWindow>) -> Option<CallWindow> {
        call_window::keep(window)
    }

    fn recover_focus(window: &AppWindow, level: i32, eager: bool) -> SharedString {
        window.global::<FocusKeeper>().invoke_recover(level, eager)
    }

    fn recover_call_focus(window: &CallWindow, level: i32, eager: bool) -> SharedString {
        window.global::<FocusKeeper>().invoke_recover(level, eager)
    }

    fn notification_text(window: &AppWindow, words: &NoticeWords<'_>) -> SharedString {
        let text = window.global::<NotificationText>();
        match *words {
            NoticeWords::Message { kind, body, sender } => {
                text.invoke_message(kind.slint(), body.into(), sender.into())
            }
            NoticeWords::HiddenMessage => text.invoke_hidden_message(),
            NoticeWords::Service {
                kind,
                actor,
                target,
            } => text.invoke_service(kind.slint(), actor.into(), target.into()),
            NoticeWords::Invite { inviter } => text.invoke_invite(inviter.into()),
            NoticeWords::Ring { video, caller } => text.invoke_ring(video, caller.into()),
        }
    }
}

pub struct SlintUiAdapter {
    window: AppWindow,
}

impl SlintUiAdapter {
    pub fn compile(_rt: &Runtime) -> Result<Self> {
        let window = AppWindow::new()?;
        backend::identify_the_app();
        about::seed_about(&window);
        Ok(Self { window })
    }

    fn bind_room_callbacks(win: &AppWindow, cmd_tx: &CommandSender) {
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
        actions(win).on_set_room_notify(move |room_id, mode| {
            router::set_room_notify(&tx, room_id.to_string(), mode.slint_name());
        });

        let tx = cmd_tx.clone();
        actions(win)
            .on_paste_attachment(move |room_id| router::paste_attachment(&tx, room_id.to_string()));

        let tx = cmd_tx.clone();
        actions(win).on_add_keyword(move |text| router::add_keyword(&tx, &text));

        actions(win).on_room_log_text(|| SharedString::from(shown_room_log_text()));
    }

    fn bind_reaction_callbacks(win: &AppWindow, cmd_tx: &CommandSender) {
        let tx = cmd_tx.clone();
        actions(win).on_toggle_reaction(move |event_id, key| {
            router::toggle_reaction(&tx, event_id.to_string(), key.to_string());
        });

        let tx = cmd_tx.clone();
        actions(win).on_react_with_emote(move |event_id, pack, shortcode| {
            router::react_with_emote(
                &tx,
                event_id.to_string(),
                pack.to_string(),
                shortcode.to_string(),
            );
        });

        let tx = cmd_tx.clone();
        actions(win).on_set_pack_global(move |pack, global| {
            router::set_pack_global(&tx, pack.to_string(), global);
        });
    }

    fn bind_decode_requests(win: &AppWindow) {
        actions(win).on_request_media(move |unique_id| request_media(&unique_id));

        actions(win).on_request_room_avatar(move |room_id| {
            request_avatar(&AvatarSlot::Room(room_id.to_string()));
        });

        actions(win).on_request_pack_cell(move |key| request_pack_cell(&key));

        actions(win).on_request_custom_emoji(move |mxc| request_custom_emoji(&mxc));
    }

    fn bind_video_callbacks(win: &AppWindow) {
        actions(win).on_toggle_video(on_main::<CompiledBackend>(win, video::toggle));
        actions(win).on_toggle_video_muted(on_main::<CompiledBackend>(win, video::toggle_muted));

        let weak = win.as_weak();
        actions(win).on_seek_video(move |ms| {
            if let Some(window) = weak.upgrade() {
                video::seek(&window, millis_to_duration(usize::try_from(ms).ok()));
            }
        });
    }

    fn bind_edit_callbacks(win: &AppWindow, cmd_tx: &CommandSender) {
        let tx = cmd_tx.clone();
        let weak = win.as_weak();
        actions(win).on_edit_message(move |req| {
            let row =
                current_edited_row::<CompiledBackend>(&weak, &req.unique_id, req.timeline_token)
                    .unwrap_or_else(|| EditedRow {
                        event_id: req.event_id.to_string(),
                        local_id: req.local_id.to_string(),
                    });
            draft_verdict(&router::edit_message(
                &tx,
                req.room_id.to_string(),
                row,
                EditedText {
                    caption: req.caption,
                    restored: req.restored,
                    original: req.original.to_string(),
                    body: req.body.to_string(),
                },
            ))
        });

        actions(win).on_last_editable_row(last_editable_row::<CompiledBackend>);
        actions(win).on_row_of_message(|event_id, local_id| {
            row_of_message::<CompiledBackend>(&event_id, &local_id)
        });

        actions(win).on_poll_answer_texts(|answers| {
            ModelRc::new(
                answers
                    .iter()
                    .map(|answer| answer.label)
                    .collect::<VecModel<SharedString>>(),
            )
        });

        actions(win).on_settings_matches(|query, entries| {
            ModelRc::new(VecModel::from(settings_search::matching_entries(
                &query,
                entries
                    .iter()
                    .map(|entry| [entry.label, entry.page_title, entry.terms]),
            )))
        });
    }

    fn bind_draft_callbacks(win: &AppWindow) {
        actions(win).on_keep_draft(|draft| drafts::keep(draft.room_id.to_string(), draft));
        actions(win).on_take_draft(|room_id| drafts::take(&room_id).unwrap_or_default());
    }

    fn bind_mention_callbacks(win: &AppWindow, cmd_tx: &CommandSender) {
        let tx = cmd_tx.clone();
        let weak = win.as_weak();
        actions(win).on_suggest_mentions(move |query| {
            router::suggest_mentions(
                &tx,
                selected_room_key::<CompiledBackend>(&weak),
                query.to_string(),
                recent_speakers::<CompiledBackend>(RECENT_SPEAKERS),
            );
        });

        actions(win).on_mention_at(|text, caret| {
            let (start, query) = mention::word_at(&text, caret);
            MentionWord {
                start,
                query: SharedString::from(query),
            }
        });

        actions(win).on_complete_mention(|text, caret, insert| {
            let (text, caret) = mention::complete(&text, caret, &insert);
            TextSplice {
                text: SharedString::from(text),
                caret,
            }
        });

        actions(win).on_emote_at(|text, caret| {
            let (start, query) = emote::word_at(&text, caret);
            MentionWord {
                start,
                query: SharedString::from(query),
            }
        });

        actions(win).on_place_emote(|text, caret, pack_id, shortcode, replaces_word| {
            let (text, caret) =
                emote::place_picked(&text, caret, &pack_id, &shortcode, replaces_word);
            TextSplice {
                text: SharedString::from(text),
                caret,
            }
        });
    }

    fn bind_audio_callbacks(win: &AppWindow, cmd_tx: &CommandSender) {
        audio::install_commands(cmd_tx);

        actions(win).on_toggle_audio(on_main::<CompiledBackend>(win, audio::toggle));

        let weak = win.as_weak();
        actions(win).on_seek_audio(move |ms| {
            if let Some(window) = weak.upgrade() {
                audio::seek(&window, millis_to_duration(usize::try_from(ms).ok()));
            }
        });
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "mirrors the fallible interpreted adapter"
    )]
    pub fn register_callbacks(
        &self,
        cmd_tx: &CommandSender,
        scroll_tx: &watch::Sender<ViewportChanged>,
        visibility_tx: &watch::Sender<TimelineVisibility>,
        composer_tx: &watch::Sender<ComposerActivity>,
        attention_tx: &watch::Sender<WindowAttention>,
    ) -> Result<()> {
        let win = &self.window;
        setup_emoji_store(win);
        call_window::install_commands(cmd_tx);
        bind_simple_callbacks(&actions(win), win, cmd_tx);
        Self::bind_room_callbacks(win, cmd_tx);
        Self::bind_reaction_callbacks(win, cmd_tx);
        Self::bind_edit_callbacks(win, cmd_tx);
        Self::bind_draft_callbacks(win);
        Self::bind_mention_callbacks(win, cmd_tx);
        Self::bind_decode_requests(win);
        Self::bind_video_callbacks(win);
        Self::bind_audio_callbacks(win, cmd_tx);
        Self::bind_window_watches(win, scroll_tx, visibility_tx, composer_tx, attention_tx);
        Ok(())
    }

    fn bind_window_watches(
        win: &AppWindow,
        scroll_tx: &watch::Sender<ViewportChanged>,
        visibility_tx: &watch::Sender<TimelineVisibility>,
        composer_tx: &watch::Sender<ComposerActivity>,
        attention_tx: &watch::Sender<WindowAttention>,
    ) {
        let scroll_tx = scroll_tx.clone();
        let weak = win.as_weak();
        actions(win).on_scroll_position_changed(
            move |at_bottom, first_unseen_row, seen_timeline_token| {
                router::scroll_position(
                    &scroll_tx,
                    adopted_room_key::<CompiledBackend>(&weak, seen_timeline_token),
                    at_bottom,
                    unread_below::<CompiledBackend>(first_unseen_row),
                );
            },
        );

        let visibility_tx = visibility_tx.clone();
        actions(win).on_timeline_visibility_changed(move |visible| {
            router::timeline_visibility(&visibility_tx, visible);
        });

        let composer_tx = composer_tx.clone();
        actions(win).on_composer_changed(move |room_id, composing, edits| {
            router::composer_changed(&composer_tx, room_id.to_string(), composing, edits);
        });

        let focus_tx = attention_tx.clone();
        actions(win).on_window_focus_changed(move |attended| {
            router::window_focus(&focus_tx, attended);
        });

        let activity_tx = attention_tx.clone();
        actions(win).on_user_activity_changed(move |active| {
            router::user_activity(&activity_tx, active);
        });
    }

    pub fn spawn_event_handler(
        &self,
        ui_rx: mpsc::Receiver<Effect>,
        view_rx: watch::Receiver<Arc<AppViewState>>,
        media_cache: Arc<dyn MediaCache>,
        notifier: Arc<dyn NotifierPort>,
        handoff: &dyn HandoffPort,
        call_video: Option<Arc<dyn CallVideoPort>>,
    ) {
        backend::spawn_event_handler::<CompiledBackend>(
            &self.window,
            ui_rx,
            view_rx,
            media_cache,
            notifier,
            handoff,
            call_video,
        );
    }

    pub fn follow_motion(&self, appearance: &dyn AppearancePort) {
        motion::follow_motion::<CompiledBackend>(&self.window, appearance);
    }

    pub fn seed_settings(&self, settings: &Settings) {
        reduce::seed_settings(&self.window, settings);
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

type LaidOutEmoji = HashMap<usize, (ModelRc<EmojiRow>, ModelRc<i32>)>;

fn emoji_rows_to_ui(rows: &[emoji::EmojiRow]) -> ModelRc<EmojiRow> {
    let rows: Vec<EmojiRow> = rows
        .iter()
        .map(|row| EmojiRow {
            is_header: row.is_header,
            group: i32::try_from(row.group).unwrap_or(0),
            cells: ModelRc::new(VecModel::from(
                row.cells.iter().map(emoji_entry_to_ui).collect::<Vec<_>>(),
            )),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn header_rows_to_ui(header_rows: &[usize]) -> ModelRc<i32> {
    let header_rows: Vec<i32> = header_rows
        .iter()
        .map(|row| i32::try_from(*row).unwrap_or(0))
        .collect();
    ModelRc::new(VecModel::from(header_rows))
}

fn setup_emoji_store(window: &AppWindow) {
    let store = window.global::<EmojiStore>();
    let columns = Rc::new(Cell::new(1_usize));
    let laid_out: Rc<RefCell<LaidOutEmoji>> = Rc::default();

    let weak = window.as_weak();
    let layout_columns = Rc::clone(&columns);
    store.on_layout(move |count| {
        let Some(w) = weak.upgrade() else {
            return;
        };
        let count = usize::try_from(count).unwrap_or(1).max(1);
        layout_columns.set(count);
        let (rows, header_rows) = laid_out
            .borrow_mut()
            .entry(count)
            .or_insert_with(|| {
                let laid = emoji::rows(count);
                (
                    emoji_rows_to_ui(&laid.rows),
                    header_rows_to_ui(&laid.header_rows),
                )
            })
            .clone();
        let store = w.global::<EmojiStore>();
        store.set_rows(rows);
        store.set_header_rows(header_rows);
    });

    let weak = window.as_weak();
    store.on_search(move |query| {
        let Some(w) = weak.upgrade() else {
            return;
        };
        w.global::<EmojiStore>()
            .set_result_rows(emoji_rows_to_ui(&emoji::result_rows(&query, columns.get())));
    });

    store.on_insert(|text, offset, glyph| {
        let (inserted, caret) = emoji::insert_at(text.as_str(), offset, glyph.as_str());
        TextSplice {
            text: SharedString::from(inserted),
            caret,
        }
    });
}
