use slint::{Image, Rgb8Pixel, SharedPixelBuffer, SharedString};

use super::present::VerifyStep;
use super::schema::{bool_props, enum_props, int_props, string_props};
use crate::app::input::CommandSender;
use crate::commands::effects::VerificationActivity;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::UiCommand;
use crate::commands::view::{AttachmentKind, LoginActivity, LoginStep};
use crate::domain::auth::LoginMethod;
use crate::domain::media::AudioKind;
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::TimelineStatus;
use crate::domain::verification::VerificationEmoji as DomainVerificationEmoji;

pub const SLINT_INFLIGHT: usize = 32;

pub fn send_command(tx: &CommandSender, cmd: UiCommand) {
    drop(tx.send(cmd));
}

macro_rules! prop_enum {
    ($name:ident; $($(#[$attr:meta])* $v:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
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

string_props!(prop_enum StringProp;);
bool_props!(prop_enum BoolProp;);
int_props!(prop_enum IntProp;);

macro_rules! declare_enum_setters {
    ($($fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        $( fn $fn(&self, value: $ty); )*
    };
}

pub trait UiProps {
    fn set_string(&self, prop: StringProp, value: SharedString);
    fn set_bool(&self, prop: BoolProp, value: bool);
    fn set_int(&self, prop: IntProp, value: i32);
    enum_props!(declare_enum_setters);
    fn apply_video_frame(&self, buffer: SharedPixelBuffer<Rgb8Pixel>);
    fn clear_video_frame(&self);
    fn get_string(&self, prop: StringProp) -> SharedString;
    fn get_int(&self, prop: IntProp) -> i32;
    fn apply_user_avatar(&self, avatar: Option<Image>);
    fn apply_attachment_preview(&self, preview: Option<Image>);
    fn apply_login_messages(&self, messages: &[UserMessage]);
    fn apply_emoji_model(&self, emojis: &[DomainVerificationEmoji]);
    fn clear_emoji_model(&self);
    fn clear_text_inputs(&self);
}
