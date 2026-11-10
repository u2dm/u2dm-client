use slint::{Image, SharedString, StyledText};
#[cfg(feature = "video")]
use slint::{Rgb8Pixel, SharedPixelBuffer};

use super::dto::{
    CallNoticeMark, CallOfferMark, CallScreenMark, PackSurface, SheetPreviewMark, TestPhaseMark,
};
use super::present::{IdentityAlertKind, ServiceKind, VerifyStep};
use super::schema::{bool_props, enum_props, image_props, int_props, message_props, string_props};
use crate::app::input::CommandSender;
use crate::commands::effects::VerificationActivity;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::UiCommand;
use crate::commands::view::{
    AccountActivity, AccountRule, AccountStep, AttachmentKind, AuthStage, CallPhase, CameraView,
    ChildAccess, CreateRoomStatus, DeviceTrouble, DirectChat, DownloadedMedia, EncryptionActivity,
    EncryptionStep, HandView, JoinRefusal, LoginActivity, LoginStep, MicrophoneView, PendingAnswer,
    PendingModeration, PeopleSearchStatus, PreviewTrouble, ReactionView, ReadersStatus,
    RoomInfoPlacement, RoomPreviewSheet, RoomScope, RosterStatus, SettingsPage, SourceState,
    SpaceIndexStatus, UpgradeLink, VerifyOffer,
};
use crate::domain::auth::LoginMethod;
use crate::domain::call::devices::DeviceKind;
use crate::domain::call::video::ScreenRefusal;
use crate::domain::departure::RemovalKind;
use crate::domain::encryption::{Recovery, Standing};
use crate::domain::invite::InviteKind;
use crate::domain::media::AudioKind;
use crate::domain::message::MessagePreviewKind;
use crate::domain::presence::PresenceLine;
use crate::domain::room::NotifyMode;
use crate::domain::room_creation::RoomKind;
use crate::domain::room_info::MemberRole;
use crate::domain::server_rules::RoomDefault;
use crate::domain::settings::{CallLayout, MotionChoice, ReceiptVisibility, SendKey, ThemeChoice};
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::{SourceEncryption, TimelineStatus};
use crate::domain::user_info::{IdentityTrust, RoomMembership};
use crate::domain::verification::VerificationEmoji as DomainVerificationEmoji;

pub const SLINT_INFLIGHT: usize = 32;

pub fn send_command(tx: &CommandSender, cmd: impl Into<UiCommand>) {
    drop(tx.send(cmd.into()));
}

macro_rules! prop_enum {
    ($(#[$meta:meta])* $name:ident;
        $($(#[$attr:meta])* $v:ident $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        $(#[$meta])*
        pub enum $name { $($(#[$attr])* $v,)* }
        impl $name {
            #[allow(dead_code)]
            pub fn as_str(&self) -> &'static str {
                match self { $($(#[$attr])* Self::$v => $lit,)* }
            }
            #[allow(dead_code)]
            pub fn global(&self) -> &'static str {
                match self { $($(#[$attr])* Self::$v => $gname,)* }
            }
        }
    };
}

#[cfg(feature = "demo")]
macro_rules! enum_prop_enum {
    ($($v:ident $fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        prop_enum!(#[allow(dead_code)] EnumProp; $($v $g $gname $lit $s $get;)*);
    };
}

macro_rules! image_prop_enum {
    ($($v:ident $g:ident $gname:literal
        $image:literal $set_image:ident $has:literal $set_has:ident;)*) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ImageProp { $($v,)* }
    };
}

macro_rules! message_prop_enum {
    ($($v:ident $set_kind:ident $detail:ident;)*) => {
        #[derive(Clone, Copy)]
        pub enum MessageProp { $($v,)* }
    };
}

macro_rules! declare_message_setter {
    ($($v:ident $set_kind:ident $detail:ident;)*) => {
        fn set_message(&self, prop: MessageProp, message: &UserMessage) {
            let detail = match prop {
                $(MessageProp::$v => {
                    self.$set_kind(message.kind);
                    StringProp::$detail
                })*
            };
            self.set_text(detail, &message.detail);
        }
    };
}

string_props!(prop_enum StringProp;);
bool_props!(prop_enum BoolProp;);
int_props!(prop_enum #[derive(Clone, Copy)] IntProp;);
image_props!(image_prop_enum);
message_props!(message_prop_enum);
#[cfg(feature = "demo")]
enum_props!(enum_prop_enum);

macro_rules! declare_enum_setters {
    ($($v:ident $fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        $( fn $fn(&self, value: $ty); )*
    };
}

pub trait UiProps {
    fn set_string(&self, prop: StringProp, value: SharedString);
    fn set_bool(&self, prop: BoolProp, value: bool);
    fn set_int(&self, prop: IntProp, value: i32);
    enum_props!(declare_enum_setters);
    #[cfg(feature = "video")]
    fn apply_video_frame(&self, buffer: SharedPixelBuffer<Rgb8Pixel>);
    fn clear_video_frame(&self);
    fn get_string(&self, prop: StringProp) -> SharedString;
    fn get_int(&self, prop: IntProp) -> i32;
    fn get_bool(&self, prop: BoolProp) -> bool;
    #[cfg(feature = "demo")]
    fn get_enum(&self, prop: EnumProp) -> SharedString;
    fn set_image(&self, prop: ImageProp, image: Option<Image>);
    message_props!(declare_message_setter);
    fn apply_room_info_topic(&self, topic: StyledText);
    fn apply_login_messages(&self, messages: &[UserMessage]);
    fn apply_emoji_model(&self, emojis: &[DomainVerificationEmoji]);
    fn clear_emoji_model(&self);
    fn clear_text_inputs(&self);

    fn set_text(&self, prop: StringProp, value: impl Into<SharedString>) {
        self.set_string(prop, value.into());
    }

    fn bump(&self, prop: IntProp) {
        self.set_int(prop, self.get_int(prop).wrapping_add(1));
    }
}
