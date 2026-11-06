use slint::{Image, StyledText};
#[cfg(feature = "video")]
use slint::{Rgb8Pixel, SharedPixelBuffer};
use slint_interpreter::{ComponentInstance, SharedString, Value};

use super::names::{
    login_view, room_info_view, user_message, verification_emoji, verification_view, video_view,
    window_props,
};
#[cfg(feature = "demo")]
use super::values::variant_of;
use super::values::{enum_value, flag_of, int_of, model_value, struct_value, text_value};
use super::{set_global, set_prop};
use crate::adapters::ui::call_window;
use crate::adapters::ui::dto::{
    CallNoticeMark, CallOfferMark, CallScreenMark, PackSurface, SheetPreviewMark, TestPhaseMark,
};
use crate::adapters::ui::present::{IdentityAlertKind, ServiceKind, VerifyStep};
#[cfg(feature = "demo")]
use crate::adapters::ui::props::EnumProp;
use crate::adapters::ui::props::{BoolProp, ImageProp, IntProp, StringProp, UiProps};
use crate::adapters::ui::schema::{bool_props, enum_props, image_props, int_props, string_props};
use crate::commands::effects::VerificationActivity;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{
    AccountActivity, AccountStep, AttachmentKind, AuthStage, CallPhase, CameraView, ChildAccess,
    CreateRoomStatus, DeviceTrouble, DirectChat, EncryptionActivity, EncryptionStep, HandView,
    JoinRefusal, LoginActivity, LoginStep, MicrophoneView, PendingAnswer, PendingModeration,
    PeopleSearchStatus, PreviewTrouble, ReactionView, ReadersStatus, RoomInfoPlacement,
    RoomPreviewSheet, RoomScope, RosterStatus, SourceState, SpaceIndexStatus, UpgradeLink,
    VerifyOffer,
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
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::{SourceEncryption, TimelineStatus};
use crate::domain::user_info::{IdentityTrust, RoomMembership};
use crate::domain::verification::VerificationEmoji as DomainVerificationEmoji;

macro_rules! impl_enum_setters {
    ($($v:ident $fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        $( fn $fn(&self, value: $ty) { set_view(self, $gname, $lit, enum_value(&value)); } )*
    };
}

macro_rules! impl_image_setter {
    ($($v:ident $g:ident $gname:literal
        $image:literal $set_image:ident $has:literal $set_has:ident;)*) => {
        fn set_image(&self, prop: ImageProp, image: Option<Image>) {
            let (global, shown, has) = match prop { $(ImageProp::$v => ($gname, $image, $has),)* };
            match image {
                Some(image) => {
                    set_global(self, global, shown, Value::Image(image));
                    set_global(self, global, has, Value::Bool(true));
                }
                None => set_global(self, global, has, Value::Bool(false)),
            }
        }
    };
}

macro_rules! mirror_props {
    ($name:ident;
        $($(#[$attr:meta])* $v:ident $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
        pub(super) fn $name(from: &ComponentInstance, to: &ComponentInstance) {
            $( $(#[$attr])* if call_window::mirrors($gname) { copy_global(from, to, $gname, $lit); } )*
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

fn set_view(inst: &ComponentInstance, global: &str, name: &str, value: Value) {
    if call_window::mirrors(global) {
        call_window::mirror(&value, |window, value| {
            set_global(window, global, name, value);
        });
    }
    set_global(inst, global, name, value);
}

fn copy_global(from: &ComponentInstance, to: &ComponentInstance, global: &str, name: &str) {
    match from.get_global_property(global, name) {
        Ok(value) => set_global(to, global, name, value),
        Err(e) => tracing::warn!("failed to read property '{global}.{name}': {e:?}"),
    }
}

impl UiProps for ComponentInstance {
    fn set_string(&self, prop: StringProp, value: SharedString) {
        set_view(self, prop.global(), prop.as_str(), Value::String(value));
    }

    fn set_bool(&self, prop: BoolProp, value: bool) {
        set_view(self, prop.global(), prop.as_str(), Value::Bool(value));
    }

    fn set_int(&self, prop: IntProp, value: i32) {
        set_view(
            self,
            prop.global(),
            prop.as_str(),
            Value::Number(value.into()),
        );
    }

    enum_props!(impl_enum_setters);

    #[cfg(feature = "video")]
    fn apply_video_frame(&self, buffer: SharedPixelBuffer<Rgb8Pixel>) {
        let frame = Value::Image(Image::from_rgb8(buffer));
        set_global(self, video_view::NAME, video_view::FRAME, frame);
        set_global(
            self,
            video_view::NAME,
            video_view::HAS_FRAME,
            Value::Bool(true),
        );
    }

    fn clear_video_frame(&self) {
        let frame = Value::Image(Image::default());
        set_global(self, video_view::NAME, video_view::FRAME, frame);
        set_global(
            self,
            video_view::NAME,
            video_view::HAS_FRAME,
            Value::Bool(false),
        );
    }

    fn get_string(&self, prop: StringProp) -> SharedString {
        self.get_global_property(prop.global(), prop.as_str())
            .ok()
            .and_then(|v| match v {
                Value::String(s) => Some(s),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn get_int(&self, prop: IntProp) -> i32 {
        let value = self.get_global_property(prop.global(), prop.as_str()).ok();
        int_of(value.as_ref())
    }

    fn get_bool(&self, prop: BoolProp) -> bool {
        let value = self.get_global_property(prop.global(), prop.as_str()).ok();
        flag_of(value.as_ref())
    }

    #[cfg(feature = "demo")]
    fn get_enum(&self, prop: EnumProp) -> SharedString {
        let value = self.get_global_property(prop.global(), prop.as_str()).ok();
        variant_of(value.as_ref()).into()
    }

    image_props!(impl_image_setter);

    fn apply_room_info_topic(&self, topic: StyledText) {
        set_global(
            self,
            room_info_view::NAME,
            room_info_view::TOPIC_STYLED,
            Value::StyledText(topic),
        );
    }

    fn apply_login_messages(&self, messages: &[UserMessage]) {
        let entries: Vec<Value> = messages
            .iter()
            .map(|m| {
                struct_value([
                    (user_message::KIND, enum_value(&m.kind)),
                    (user_message::DETAIL, text_value(&m.detail)),
                ])
            })
            .collect();
        set_global(
            self,
            login_view::NAME,
            login_view::MESSAGES,
            model_value(entries),
        );
    }

    fn apply_emoji_model(&self, emojis: &[DomainVerificationEmoji]) {
        let entries: Vec<Value> = emojis
            .iter()
            .map(|e| {
                struct_value([
                    (verification_emoji::SYMBOL, text_value(&e.symbol)),
                    (verification_emoji::DESCRIPTION, text_value(&e.description)),
                ])
            })
            .collect();
        set_global(
            self,
            verification_view::NAME,
            verification_view::EMOJIS,
            model_value(entries),
        );
    }

    fn clear_emoji_model(&self) {
        set_global(
            self,
            verification_view::NAME,
            verification_view::EMOJIS,
            model_value(Vec::new()),
        );
    }

    fn clear_text_inputs(&self) {
        let empty = Value::String(SharedString::default());
        set_prop(self, window_props::INPUT_USERNAME, empty.clone());
        set_prop(self, window_props::INPUT_MESSAGE, empty);
    }
}
