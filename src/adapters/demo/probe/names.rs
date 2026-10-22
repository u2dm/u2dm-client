use crate::adapters::ui::IdentityAlertKind;
use crate::adapters::ui::schema::{
    account_activities, account_steps, attachment_kinds, audio_kinds, auth_stages,
    call_device_kinds, call_device_statuses, call_device_troubles, child_accesses,
    connection_states, create_room_statuses, device_trusts, direct_chats, encryption_activities,
    encryption_standings, encryption_steps, enum_names, identity_alert_kinds, identity_trusts,
    invite_answers, invite_kinds, invitee_states, log_levels, login_activities, login_methods,
    login_phases, member_roles, notify_modes, pending_moderations, people_search_statuses,
    presence_lines, preview_kinds, readers_statuses, recovery_statuses, removal_kinds,
    room_accesses, room_info_placements, room_kinds, room_memberships, room_preview_statuses,
    room_scopes, roster_statuses, source_encryptions, source_statuses, space_index_statuses,
    upgrade_links, user_message_kinds, variant_named, verify_offers,
};
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{
    AccountActivity, AccountStep, AttachmentKind, AuthStage, ChildAccess, CreateRoomStatus,
    DeviceStatus, DeviceTrouble, DirectChat, EncryptionActivity, EncryptionStep, InviteeState,
    LoginActivity, LoginStep, PendingAnswer, PendingModeration, PeopleSearchStatus, ReadersStatus,
    RoomInfoPlacement, RoomPreviewSheet, RoomScope, RosterStatus, SourceState, SpaceIndexStatus,
    UpgradeLink, VerifyOffer,
};
use crate::domain::auth::LoginMethod;
use crate::domain::call::devices::DeviceKind;
use crate::domain::departure::RemovalKind;
use crate::domain::encryption::{Recovery, Standing};
use crate::domain::invite::InviteKind;
use crate::domain::media::AudioKind;
use crate::domain::message::MessagePreviewKind;
use crate::domain::presence::PresenceLine;
use crate::domain::room::NotifyMode;
use crate::domain::room_creation::{RoomAccess, RoomKind};
use crate::domain::room_info::MemberRole;
use crate::domain::room_log::LogLevel;
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::SourceEncryption;
use crate::domain::user_info::{DeviceTrust, IdentityTrust, RoomMembership};

login_phases!(enum_names val login_step LoginStep;);
login_activities!(enum_names val login_activity LoginActivity;);
login_methods!(enum_names val login_method LoginMethod;);
connection_states!(enum_names ref connection_status ConnectionStatus;);
user_message_kinds!(enum_names val user_message_kind UserMessageKind;);
attachment_kinds!(enum_names val attachment_kind AttachmentKind;);
room_scopes!(enum_names val room_scope RoomScope;);
space_index_statuses!(enum_names val space_index_status SpaceIndexStatus;);
room_preview_statuses!(enum_names ref room_preview_status RoomPreviewSheet;);
child_accesses!(enum_names val child_access ChildAccess;);
audio_kinds!(enum_names val audio_kind AudioKind;);
preview_kinds!(enum_names val preview_kind MessagePreviewKind;);
notify_modes!(enum_names val notify_mode NotifyMode;);
notify_modes!(variant_named notify_mode_named NotifyMode;);
call_device_kinds!(enum_names val call_device_kind DeviceKind;);
call_device_kinds!(variant_named call_device_kind_named DeviceKind;);
call_device_statuses!(enum_names val call_device_status DeviceStatus;);
call_device_troubles!(enum_names val call_device_trouble DeviceTrouble;);
member_roles!(enum_names val member_role MemberRole;);
roster_statuses!(enum_names val roster_status RosterStatus;);
room_info_placements!(enum_names val room_info_placement RoomInfoPlacement;);
room_memberships!(enum_names val room_membership RoomMembership;);
direct_chats!(enum_names val direct_chat DirectChat;);
upgrade_links!(enum_names ref upgrade_link UpgradeLink;);
invite_kinds!(enum_names val invite_kind InviteKind;);
removal_kinds!(enum_names val removal_kind RemovalKind;);
invite_answers!(enum_names val pending_answer PendingAnswer;);
pending_moderations!(enum_names val pending_moderation PendingModeration;);
identity_trusts!(enum_names val identity_trust IdentityTrust;);
verify_offers!(enum_names val verify_offer VerifyOffer;);
device_trusts!(enum_names val device_trust DeviceTrust;);
presence_lines!(enum_names val presence_line PresenceLine;);
identity_alert_kinds!(enum_names val identity_alert_kind IdentityAlertKind;);
encryption_standings!(enum_names val encryption_standing Standing;);
recovery_statuses!(enum_names val recovery_status Recovery;);
encryption_steps!(enum_names ref encryption_step EncryptionStep;);
encryption_activities!(enum_names val encryption_activity EncryptionActivity;);
auth_stages!(enum_names val auth_stage AuthStage;);
account_steps!(enum_names ref account_step AccountStep;);
account_activities!(enum_names val account_activity AccountActivity;);
source_statuses!(enum_names ref source_status SourceState;);
source_encryptions!(enum_names ref source_encryption SourceEncryption;);
readers_statuses!(enum_names val readers_status ReadersStatus;);
room_kinds!(enum_names val room_kind RoomKind;);
room_kinds!(variant_named room_kind_named RoomKind;);
room_accesses!(enum_names val room_access RoomAccess;);
room_accesses!(variant_named room_access_named RoomAccess;);
create_room_statuses!(enum_names val create_room_status CreateRoomStatus;);
people_search_statuses!(enum_names val people_search_status PeopleSearchStatus;);
invitee_states!(enum_names val invitee_state InviteeState;);
log_levels!(enum_names val log_level LogLevel;);
