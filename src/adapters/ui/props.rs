use slint::{Image, SharedString};
use tokio::sync::mpsc;

use super::present::VerifyStep;
use super::schema::{bool_props, int_props, string_props};
use crate::commands::effects::VerificationActivity;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::UiCommand;
use crate::commands::view::{LoginActivity, LoginStep};
use crate::domain::auth::LoginMethod;
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::TimelineStatus;
use crate::domain::verification::VerificationEmoji as DomainVerificationEmoji;

pub const SLINT_INFLIGHT: usize = 32;

pub fn send_command(tx: &mpsc::UnboundedSender<UiCommand>, cmd: UiCommand) {
    if let Err(mpsc::error::SendError(cmd)) = tx.send(cmd) {
        tracing::debug!(command = %cmd, "command channel closed; dropping command");
    }
}

macro_rules! prop_enum {
    ($name:ident; $($v:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        pub enum $name { $($v,)* }
        impl $name {
            #[allow(dead_code)]
            pub fn as_str(&self) -> &'static str {
                match self { $(Self::$v => $lit,)* }
            }
            #[allow(dead_code)]
            pub fn global(&self) -> &'static str {
                match self { $(Self::$v => $gname,)* }
            }
        }
    };
}

string_props!(prop_enum StringProp;);
bool_props!(prop_enum BoolProp;);
int_props!(prop_enum IntProp;);

pub trait UiProps {
    fn set_string(&self, prop: StringProp, value: SharedString);
    fn set_bool(&self, prop: BoolProp, value: bool);
    fn set_int(&self, prop: IntProp, value: i32);
    fn set_login_phase(&self, step: LoginStep);
    fn set_login_activity(&self, activity: LoginActivity);
    fn set_login_method_kind(&self, method: LoginMethod);
    fn set_toast_message(&self, kind: UserMessageKind);
    fn set_verification_error(&self, kind: UserMessageKind);
    fn set_connection_state(&self, status: &ConnectionStatus);
    fn set_timeline_state(&self, status: TimelineStatus);
    fn set_verification_phase(&self, phase: VerifyStep);
    fn set_verification_activity(&self, activity: VerificationActivity);
    fn get_string(&self, prop: StringProp) -> SharedString;
    fn get_int(&self, prop: IntProp) -> i32;
    fn apply_user_avatar(&self, avatar: Option<Image>);
    fn apply_login_messages(&self, messages: &[UserMessage]);
    fn apply_emoji_model(&self, emojis: &[DomainVerificationEmoji]);
    fn clear_emoji_model(&self);
    fn clear_text_inputs(&self);
}
