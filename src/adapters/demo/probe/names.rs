use crate::adapters::ui::schema::{
    attachment_kinds, audio_kinds, connection_states, enum_names, login_activities, login_methods,
    login_phases, room_scopes, user_message_kinds,
};
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{AttachmentKind, LoginActivity, LoginStep, RoomScope};
use crate::domain::auth::LoginMethod;
use crate::domain::media::AudioKind;
use crate::domain::sync::ConnectionStatus;

login_phases!(enum_names val login_step LoginStep;);
login_activities!(enum_names val login_activity LoginActivity;);
login_methods!(enum_names val login_method LoginMethod;);
connection_states!(enum_names ref connection_status ConnectionStatus;);
user_message_kinds!(enum_names val user_message_kind UserMessageKind;);
attachment_kinds!(enum_names val attachment_kind AttachmentKind;);
room_scopes!(enum_names val room_scope RoomScope;);
audio_kinds!(enum_names val audio_kind AudioKind;);
