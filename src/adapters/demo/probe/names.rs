use crate::adapters::ui::schema::{
    attachment_kinds, audio_kinds, child_accesses, connection_states, direct_chats, enum_names,
    log_levels, login_activities, login_methods, login_phases, member_roles, notify_modes,
    pending_moderations, preview_kinds, room_info_placements, room_memberships, room_scopes,
    roster_statuses, source_encryptions, source_statuses, space_index_statuses, user_message_kinds,
    variant_named,
};
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{
    AttachmentKind, ChildAccess, DirectChat, LoginActivity, LoginStep, PendingModeration,
    RoomInfoPlacement, RoomScope, RosterStatus, SourceState, SpaceIndexStatus,
};
use crate::domain::auth::LoginMethod;
use crate::domain::media::AudioKind;
use crate::domain::message::MessagePreviewKind;
use crate::domain::room::NotifyMode;
use crate::domain::room_info::MemberRole;
use crate::domain::room_log::LogLevel;
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::SourceEncryption;
use crate::domain::user_info::RoomMembership;

login_phases!(enum_names val login_step LoginStep;);
login_activities!(enum_names val login_activity LoginActivity;);
login_methods!(enum_names val login_method LoginMethod;);
connection_states!(enum_names ref connection_status ConnectionStatus;);
user_message_kinds!(enum_names val user_message_kind UserMessageKind;);
attachment_kinds!(enum_names val attachment_kind AttachmentKind;);
room_scopes!(enum_names val room_scope RoomScope;);
space_index_statuses!(enum_names val space_index_status SpaceIndexStatus;);
child_accesses!(enum_names val child_access ChildAccess;);
audio_kinds!(enum_names val audio_kind AudioKind;);
preview_kinds!(enum_names val preview_kind MessagePreviewKind;);
notify_modes!(enum_names val notify_mode NotifyMode;);
notify_modes!(variant_named notify_mode_named NotifyMode;);
member_roles!(enum_names val member_role MemberRole;);
roster_statuses!(enum_names val roster_status RosterStatus;);
room_info_placements!(enum_names val room_info_placement RoomInfoPlacement;);
room_memberships!(enum_names val room_membership RoomMembership;);
direct_chats!(enum_names val direct_chat DirectChat;);
pending_moderations!(enum_names val pending_moderation PendingModeration;);
source_statuses!(enum_names ref source_status SourceState;);
source_encryptions!(enum_names ref source_encryption SourceEncryption;);
log_levels!(enum_names val log_level LogLevel;);
