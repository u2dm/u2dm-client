use serde::Serialize;

use super::names;
use crate::adapters::ui::identity_banner;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::Draft;
use crate::commands::view::{
    AppViewState, AttachmentView, AudioView, CardStatus, CopiedLink, CreateRoomView, DirectoryView,
    EncryptionStep, EncryptionView, IdentityAlertView, InvitePeopleView, InviteView, InviteeRow,
    LifecycleView, LoginStep, MentionsView, NotificationsView, PackView, PaginationView,
    PinnedView, ReadersView, RoomInfoView, RoomLogView, RoomMenuTarget, RoomPreviewSheet,
    RoomPreviewView, RosterRow, SourceState, SpaceIndexRow, SpaceIndexView, SpaceMatch, Toast,
    TrackFile, TypingView, UnsentMessage, UpgradeView, UserInfoView, VideoView,
};
use crate::domain::encryption::KeyStorage;
use crate::domain::image_pack::{ImagePack, ImageUsage, PackOrigin, PackTitle};
use crate::domain::invite::Invite;
use crate::domain::member_name::NamedUser;
use crate::domain::message::{EditKind, PinnedMessage};
use crate::domain::push::NoticeContent;
use crate::domain::room::{Room, Space};
use crate::domain::room_info::{Reader, RosterSection};
use crate::domain::room_preview::{PreviewMembership, RoomPreview};
use crate::domain::space_index::{ChildKind, JoinRule};
use crate::domain::sync::{ConnectionStatus, ReauthReason};
use crate::domain::timeline::{OlderHistory, SourceEncryption};
use crate::domain::user_info::{Moderation, Pronouns};

#[derive(Serialize)]
pub struct ViewDto {
    lifecycle: LifecycleDto,
    connection: ConnectionDto,
    directory: DirectoryDto,
    space_index: SpaceIndexDto,
    invite: InviteSheetDto,
    room_preview: RoomPreviewDto,
    create_room: CreateRoomDto,
    invite_people: InvitePeopleDto,
    room_info: RoomInfoDto,
    room_menu: Option<RoomMenuDto>,
    user_info: UserInfoDto,
    pagination: PaginationDto,
    pinned: PinnedDto,
    image_packs: PacksDto,
    attachment: AttachmentDto,
    video: VideoDto,
    audio: Option<NowPlayingDto>,
    unsent: Option<UnsentDto>,
    identity_alert: IdentityAlertDto,
    upgrade: UpgradeDto,
    typing: TypingDto,
    notifications: Vec<NotificationDto>,
    encryption: EncryptionDto,
    toast: ToastDto,
    message_link: LinkDto,
    room_link: LinkDto,
    source: SourceDto,
    readers: ReadersDto,
    mentions: MentionsDto,
    room_log: Option<RoomLogDto>,
}

#[derive(Serialize)]
struct CreateRoomDto {
    status: &'static str,
    kind: &'static str,
    parent: Option<String>,
    server: String,
    error: MessageDto,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct InvitePeopleDto {
    open: bool,
    room_id: Option<String>,
    room_name: Option<String>,
    is_space: bool,
    search: &'static str,
    limited: bool,
    sending: bool,
    error: MessageDto,
    avatars_ready: usize,
    results: Vec<InviteeDto>,
    picked: Vec<InviteeDto>,
}

#[derive(Serialize)]
struct InviteeDto {
    user_id: String,
    shown: String,
    has_avatar_mxc: bool,
    membership: &'static str,
    state: &'static str,
    failure: Option<MessageDto>,
}

#[derive(Serialize)]
struct IdentityAlertDto {
    kind: &'static str,
    user_id: Option<String>,
    name: Option<String>,
    busy: bool,
}

#[derive(Serialize)]
struct UpgradeDto {
    room_id: Option<String>,
    successor: &'static str,
    successor_id: Option<String>,
    predecessor: &'static str,
    predecessor_id: Option<String>,
}

#[derive(Serialize)]
struct TypingDto {
    room_id: Option<String>,
    typists: Vec<TypistDto>,
}

#[derive(Serialize)]
struct NotificationDto {
    serial: u64,
    room_id: String,
    room_name: String,
    kind: &'static str,
    sender: Option<String>,
    highlight: bool,
    sound: bool,
}

#[derive(Serialize)]
struct TypistDto {
    user_id: String,
    shown: String,
}

#[derive(Serialize)]
struct EncryptionDto {
    standing: &'static str,
    verified_sessions: bool,
    recovery: &'static str,
    key_storage: &'static str,
    open: bool,
    step: Option<&'static str>,
    activity: Option<&'static str>,
    recovery_key: Option<String>,
    error: Option<MessageDto>,
}

#[derive(Serialize)]
struct MentionsDto {
    room_id: Option<String>,
    offers_room: bool,
    avatars_ready: usize,
    rows: Vec<MentionDto>,
}

#[derive(Serialize)]
struct MentionDto {
    user_id: String,
    name: String,
}

#[derive(Serialize)]
struct ReadersDto {
    status: &'static str,
    event_id: Option<String>,
    sender: Option<String>,
    kind: Option<&'static str>,
    total: usize,
    has_more: bool,
    pages_landed: i32,
    avatars_ready: usize,
    rows: Vec<ReaderDto>,
}

#[derive(Serialize)]
struct ReaderDto {
    user_id: String,
    name: String,
    role: &'static str,
    has_avatar_mxc: bool,
}

#[derive(Serialize)]
struct RoomLogDto {
    room_id: String,
    name: String,
    lines: usize,
    dropped: u64,
    lines_landed: i32,
    newest: Vec<String>,
}

#[derive(Serialize)]
struct SourceDto {
    status: &'static str,
    event_id: Option<String>,
    encryption: Option<&'static str>,
    json: Option<String>,
    edit_json: Option<String>,
    encryption_json: Option<String>,
}

#[derive(Serialize)]
struct LinkDto {
    serial: i32,
    url: String,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct RoomMenuDto {
    room_id: String,
    name: String,
    unread: bool,
    notify: &'static str,
    notify_busy: bool,
    leaving: bool,
    may_invite: bool,
}

#[derive(Serialize)]
struct MessageDto {
    kind: &'static str,
    detail: String,
}

#[derive(Serialize)]
struct LifecycleDto {
    step: &'static str,
    reauth_reason: Option<&'static str>,
    activity: &'static str,
    method: &'static str,
    resolved_homeserver: String,
    user_id: String,
    has_avatar: bool,
    messages: Vec<MessageDto>,
}

#[derive(Serialize)]
struct ConnectionDto {
    status: &'static str,
    detail: Option<String>,
}

#[derive(Serialize)]
struct PollPermissionsDto {
    vote: bool,
    end: bool,
    start: bool,
}

#[derive(Serialize)]
struct MessagePermissionsDto {
    delete_own: bool,
    delete_others: bool,
    pin: bool,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct RoomDto {
    id: String,
    name: String,
    is_direct: bool,
    is_encrypted: bool,
    poll_permissions: PollPermissionsDto,
    message_permissions: MessagePermissionsDto,
    may_invite: bool,
    member_count: u64,
    has_unread: bool,
    has_mentions: bool,
    has_activity: bool,
    muted: bool,
    alert: bool,
    mention: bool,
    hint: bool,
    last_activity_ts: u64,
    last_message_sender: Option<String>,
    last_message_body: String,
    last_message_is_own: bool,
    last_message_edited: bool,
    successor: Option<String>,
    predecessor: Option<String>,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct SpaceDto {
    id: String,
    name: String,
    member_count: u64,
    child_room_ids: Vec<String>,
    child_space_ids: Vec<String>,
    order: Option<String>,
    may_invite: bool,
    may_add_children: bool,
    alert: bool,
    mention: bool,
    hint: bool,
}

#[derive(Serialize)]
struct SpaceHeadingDto {
    id: String,
    name: String,
    member_count: u64,
    may_invite: bool,
    may_add_children: bool,
}

#[derive(Serialize)]
struct SpaceMatchDto {
    id: String,
    name: String,
    parent: Option<String>,
    rooms: usize,
    alert: bool,
    mention: bool,
    hint: bool,
}

#[derive(Serialize)]
struct InviteDto {
    id: String,
    name: String,
    kind: &'static str,
    inviter: Option<String>,
    inviter_name: Option<String>,
    topic: Option<String>,
    has_avatar_mxc: bool,
    received_ts: u64,
}

#[derive(Serialize)]
struct RoomPreviewDto {
    status: &'static str,
    address: Option<String>,
    entry: bool,
    access: &'static str,
    room: Option<PreviewedRoomDto>,
    opens_event: bool,
    error: MessageDto,
    avatars_ready: usize,
}

#[derive(Serialize)]
struct PreviewedRoomDto {
    room_id: String,
    name: String,
    alias: Option<String>,
    topic: Option<String>,
    members: u64,
    is_space: bool,
    join_rule: Option<&'static str>,
    membership: &'static str,
    has_avatar_mxc: bool,
}

#[derive(Serialize)]
struct InviteSheetDto {
    open: bool,
    invite: Option<InviteDto>,
    answer: &'static str,
    error: MessageDto,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct DirectoryDto {
    scope: &'static str,
    space_id: String,
    subspace_id: String,
    listed_space: SpaceHeadingDto,
    direct_alert: bool,
    direct_mention: bool,
    direct_hint: bool,
    invites_pending: bool,
    attention: usize,
    invites: Vec<InviteDto>,
    rooms: Vec<RoomDto>,
    space_matches: Vec<SpaceMatchDto>,
    spaces: Vec<SpaceDto>,
    subspaces: Vec<SpaceDto>,
}

#[derive(Serialize)]
struct SpaceIndexDto {
    status: &'static str,
    space_name: String,
    avatars_ready: usize,
    pages_landed: i32,
    rows: Vec<SpaceChildDto>,
}

#[derive(Serialize)]
struct SpaceChildDto {
    id: String,
    name: String,
    kind: &'static str,
    children: Option<u64>,
    members: u64,
    join_rule: &'static str,
    via: Vec<String>,
    has_avatar_mxc: bool,
    access: &'static str,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct RoomInfoDto {
    open: bool,
    placement: &'static str,
    room_id: Option<String>,
    name: Option<String>,
    member_count: Option<u64>,
    is_direct: bool,
    topic: Option<String>,
    topic_html: Option<String>,
    alias: Option<String>,
    joined_at: Option<u64>,
    link: Option<String>,
    notify: &'static str,
    notify_busy: bool,
    leaving: bool,
    may_invite: bool,
    roster: &'static str,
    has_more: bool,
    pages_landed: i32,
    avatars_ready: usize,
    error: MessageDto,
    rows: Vec<MemberRowDto>,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct UserInfoDto {
    open: bool,
    status: Option<&'static str>,
    room_id: Option<String>,
    user_id: Option<String>,
    name: Option<String>,
    has_avatar_mxc: bool,
    pronouns: Option<Vec<String>>,
    role: Option<&'static str>,
    membership: Option<&'static str>,
    identity: &'static str,
    is_self: bool,
    link: Option<String>,
    direct_room: Option<String>,
    direct: &'static str,
    ignored: bool,
    ignore_busy: bool,
    may_kick: bool,
    may_ban: bool,
    may_unban: bool,
    moderating: &'static str,
    avatars_ready: usize,
    error: MessageDto,
}

#[derive(Serialize)]
struct MemberRowDto {
    kind: &'static str,
    user_id: Option<String>,
    name: Option<String>,
    role: Option<&'static str>,
    invited: bool,
    has_avatar_mxc: bool,
}

#[derive(Serialize)]
struct PaginationDto {
    generation: i32,
    backwards_loading: bool,
    older_history: &'static str,
    forwards_loading: bool,
    new_messages: u32,
}

#[derive(Serialize)]
struct PinnedMessageDto {
    event_id: String,
    kind: &'static str,
    body: String,
}

#[derive(Serialize)]
struct PinnedDto {
    room_id: Option<String>,
    shown: usize,
    messages: Vec<PinnedMessageDto>,
    pinned_ids: Vec<String>,
}

#[derive(Serialize)]
struct PackDto {
    id: String,
    title: String,
    untitled: bool,
    origin: &'static str,
    shortcodes: Vec<String>,
    stickers: Vec<String>,
}

#[derive(Serialize)]
struct PacksDto {
    generation: i32,
    ready_images: usize,
    room_encrypted: bool,
    loading: bool,
    packs: Vec<PackDto>,
}

#[derive(Serialize)]
struct AttachmentDto {
    visible: bool,
    filename: String,
    mimetype: String,
    size: u64,
    width: u32,
    height: u32,
    kind: &'static str,
    duration_ms: Option<u128>,
    has_preview: bool,
    sending: bool,
    error: &'static str,
    error_detail: String,
}

#[derive(Serialize)]
struct VideoDto {
    visible: bool,
    loading: bool,
    has_path: bool,
    error: &'static str,
}

#[derive(Serialize)]
struct UnsentDto {
    submission: i32,
    room_id: String,
    body: String,
    reply_to: Option<String>,
    edits: Option<String>,
    caption: bool,
    emotes: Vec<String>,
}

#[derive(Serialize)]
struct NowPlayingDto {
    request: u64,
    room_id: String,
    event_id: String,
    sender: String,
    kind: &'static str,
    title: String,
    duration_ms: Option<u128>,
    has_waveform: bool,
    downloaded: bool,
}

#[derive(Serialize)]
struct ToastDto {
    kind: &'static str,
    detail: Option<String>,
}

pub fn view(source: &AppViewState) -> ViewDto {
    ViewDto {
        lifecycle: lifecycle(&source.lifecycle),
        connection: connection(&source.connection),
        directory: directory(&source.directory),
        space_index: space_index(&source.space_index),
        invite: invite_sheet(&source.invite),
        room_preview: room_preview(&source.room_preview),
        create_room: create_room(&source.create_room),
        invite_people: invite_people(&source.invite_people),
        room_info: room_info(&source.room_info),
        room_menu: source.room_menu.as_ref().map(room_menu),
        user_info: user_info(&source.user_info),
        pagination: pagination(source.pagination),
        pinned: pinned(&source.pinned),
        image_packs: image_packs(&source.image_packs),
        attachment: attachment(&source.attachment),
        video: video(&source.video),
        audio: audio(&source.audio),
        unsent: source.unsent.as_ref().map(unsent),
        identity_alert: identity_alert(&source.identity_alert),
        upgrade: upgrade(&source.upgrade),
        typing: typing(&source.typing),
        notifications: notifications(&source.notifications),
        encryption: encryption(&source.encryption),
        toast: toast(&source.toast),
        message_link: link(&source.message_link),
        room_link: link(&source.room_link),
        source: event_source(&source.source),
        readers: readers(&source.readers),
        mentions: mentions(&source.mentions),
        room_log: source.room_log.as_ref().map(room_log),
    }
}

fn create_room(source: &CreateRoomView) -> CreateRoomDto {
    CreateRoomDto {
        status: names::create_room_status(source.status),
        kind: names::room_kind(source.kind),
        parent: (!source.parent_name.is_empty()).then(|| source.parent_name.clone()),
        server: source.server_name.clone(),
        error: message(&source.error),
    }
}

fn invite_people(source: &InvitePeopleView) -> InvitePeopleDto {
    let target = source.target.as_ref();
    InvitePeopleDto {
        open: target.is_some(),
        room_id: target.map(|target| target.room_id.to_string()),
        room_name: target.map(|target| target.name.clone()),
        is_space: target.is_some_and(|target| target.space),
        search: names::people_search_status(source.search),
        limited: source.limited,
        sending: source.sending,
        error: message(&source.error),
        avatars_ready: source.avatars_ready,
        results: source.results.iter().map(invitee).collect(),
        picked: source.picked.iter().map(invitee).collect(),
    }
}

fn invitee(source: &InviteeRow) -> InviteeDto {
    InviteeDto {
        user_id: source.person.user_id.to_string(),
        shown: source.person.shown(),
        has_avatar_mxc: source.person.avatar_mxc.is_some(),
        membership: names::room_membership(source.person.membership),
        state: names::invitee_state(source.state),
        failure: (source.failure.kind != UserMessageKind::None).then(|| message(&source.failure)),
    }
}

fn identity_alert(source: &IdentityAlertView) -> IdentityAlertDto {
    let banner = identity_banner(source.alert.as_ref());
    IdentityAlertDto {
        kind: names::identity_alert_kind(banner.kind),
        user_id: (!banner.user_id.is_empty()).then(|| banner.user_id.to_owned()),
        name: (!banner.name.is_empty()).then_some(banner.name),
        busy: source.busy,
    }
}

fn upgrade(source: &UpgradeView) -> UpgradeDto {
    UpgradeDto {
        room_id: source.room_id.as_ref().map(ToString::to_string),
        successor: names::upgrade_link(&source.successor),
        successor_id: source.successor.room_id().map(ToString::to_string),
        predecessor: names::upgrade_link(&source.predecessor),
        predecessor_id: source.predecessor.room_id().map(ToString::to_string),
    }
}

fn typing(source: &TypingView) -> TypingDto {
    TypingDto {
        room_id: source.room_id.as_ref().map(ToString::to_string),
        typists: source
            .typists
            .users()
            .iter()
            .map(|user| TypistDto {
                user_id: user.user_id.clone(),
                shown: user.shown(),
            })
            .collect(),
    }
}

fn notifications(source: &NotificationsView) -> Vec<NotificationDto> {
    source
        .shown
        .iter()
        .map(|shown| {
            let notice = &shown.notice;
            let (kind, sender) = match &notice.content {
                NoticeContent::Message { sender, .. } => ("message", sender.as_ref()),
                NoticeContent::Service { actor, .. } => ("service", Some(actor)),
                NoticeContent::Invite { inviter } => ("invite", inviter.as_ref()),
            };
            NotificationDto {
                serial: shown.serial,
                room_id: notice.room_id.to_string(),
                room_name: notice.room_name.clone(),
                kind,
                sender: sender.map(NamedUser::shown),
                highlight: notice.tweaks.highlight,
                sound: notice.tweaks.sound,
            }
        })
        .collect()
}

fn encryption(source: &EncryptionView) -> EncryptionDto {
    let dialog = source.dialog.as_ref();
    EncryptionDto {
        standing: names::encryption_standing(source.standing),
        verified_sessions: source.verified_sessions,
        recovery: names::recovery_status(source.recovery),
        key_storage: match source.key_storage {
            KeyStorage::Allowed => "allowed",
            KeyStorage::TurnedOff => "turned-off",
        },
        open: dialog.is_some(),
        step: dialog.map(|dialog| names::encryption_step(&dialog.step)),
        activity: dialog.map(|dialog| names::encryption_activity(dialog.activity)),
        recovery_key: dialog.and_then(|dialog| match &dialog.step {
            EncryptionStep::ShowingKey(key) => Some(key.expose().to_owned()),
            EncryptionStep::Overview
            | EncryptionStep::EnteringKey
            | EncryptionStep::Unlocked
            | EncryptionStep::UnlockedWithoutBackup => None,
        }),
        error: dialog.map(|dialog| message(&dialog.error)),
    }
}

fn mentions(source: &MentionsView) -> MentionsDto {
    MentionsDto {
        room_id: source.room_id.as_ref().map(ToString::to_string),
        offers_room: source.offers_room,
        avatars_ready: source.avatars_ready,
        rows: source
            .rows
            .iter()
            .map(|member| MentionDto {
                user_id: member.user_id.clone(),
                name: member.name.shown(&member.user_id),
            })
            .collect(),
    }
}

fn readers(source: &ReadersView) -> ReadersDto {
    ReadersDto {
        status: names::readers_status(source.status),
        event_id: source
            .message
            .as_ref()
            .and_then(|message| message.event_id.clone()),
        sender: source
            .message
            .as_ref()
            .map(|message| message.sender.clone()),
        kind: source
            .message
            .as_ref()
            .map(|message| names::preview_kind(message.body.preview_kind())),
        total: source.total,
        has_more: source.has_more,
        pages_landed: source.pages_landed,
        avatars_ready: source.avatars_ready,
        rows: source
            .rows
            .iter()
            .map(|reader| reader_row(reader))
            .collect(),
    }
}

fn reader_row(source: &Reader) -> ReaderDto {
    ReaderDto {
        user_id: source.user_id.clone(),
        name: source.name.shown(&source.user_id),
        role: names::member_role(source.role),
        has_avatar_mxc: source.avatar_mxc.is_some(),
    }
}

const NEWEST_LOG_LINES: usize = 12;

fn room_log(source: &RoomLogView) -> RoomLogDto {
    let lines = &source.log.lines;
    RoomLogDto {
        room_id: source.room_id.to_string(),
        name: source.name.clone(),
        lines: lines.len(),
        dropped: source.log.dropped,
        lines_landed: source.lines_landed,
        newest: lines
            .iter()
            .skip(lines.len().saturating_sub(NEWEST_LOG_LINES))
            .map(|line| {
                format!(
                    "{} {}: {}",
                    names::log_level(line.level),
                    line.target,
                    line.text
                )
            })
            .collect(),
    }
}

fn link(source: &CopiedLink) -> LinkDto {
    LinkDto {
        serial: source.serial,
        url: source.url.clone(),
    }
}

fn room_menu(source: &RoomMenuTarget) -> RoomMenuDto {
    RoomMenuDto {
        room_id: source.room_id.to_string(),
        name: source.name.clone(),
        unread: source.unread,
        notify: names::notify_mode(source.notify),
        notify_busy: source.notify_busy,
        leaving: source.leaving,
        may_invite: source.may_invite,
    }
}

fn event_source(state: &SourceState) -> SourceDto {
    let status = names::source_status(state);
    match state {
        SourceState::Closed => SourceDto {
            status,
            event_id: None,
            encryption: None,
            json: None,
            edit_json: None,
            encryption_json: None,
        },
        SourceState::Locating { event_id } | SourceState::Unavailable { event_id } => SourceDto {
            status,
            event_id: Some(event_id.clone()),
            encryption: None,
            json: None,
            edit_json: None,
            encryption_json: None,
        },
        SourceState::Ready(source) => SourceDto {
            status,
            event_id: Some(source.event_id.clone()),
            encryption: Some(names::source_encryption(&source.encryption)),
            json: Some(source.json.clone()),
            edit_json: source.edit_json.clone(),
            encryption_json: match &source.encryption {
                SourceEncryption::Decrypted { details } => Some(details.clone()),
                SourceEncryption::Plain | SourceEncryption::Undecryptable => None,
            },
        },
    }
}

fn message(source: &UserMessage) -> MessageDto {
    MessageDto {
        kind: names::user_message_kind(source.kind),
        detail: source.detail.clone(),
    }
}

fn lifecycle(source: &LifecycleView) -> LifecycleDto {
    LifecycleDto {
        step: names::login_step(source.step),
        reauth_reason: reauth_reason(source.step),
        activity: names::login_activity(source.activity),
        method: names::login_method(source.method),
        resolved_homeserver: source.resolved_homeserver.clone(),
        user_id: source.user_id.clone(),
        has_avatar: source.avatar_path.is_some(),
        messages: source.messages.iter().map(message).collect(),
    }
}

fn reauth_reason(step: LoginStep) -> Option<&'static str> {
    let LoginStep::Reauthenticate(reason) = step else {
        return None;
    };
    Some(match reason {
        ReauthReason::SoftLogout => "soft-logout",
        ReauthReason::Refused => "refused",
    })
}

fn connection(source: &ConnectionStatus) -> ConnectionDto {
    ConnectionDto {
        status: names::connection_status(source),
        detail: match source {
            ConnectionStatus::Error(detail) => Some(detail.clone()),
            _ => None,
        },
    }
}

fn room(source: &Room) -> RoomDto {
    RoomDto {
        id: source.id.to_string(),
        name: source.display_name.clone(),
        is_direct: source.is_direct,
        is_encrypted: source.is_encrypted,
        poll_permissions: PollPermissionsDto {
            vote: source.poll_permissions.vote,
            end: source.poll_permissions.end,
            start: source.poll_permissions.start,
        },
        message_permissions: MessagePermissionsDto {
            delete_own: source.message_permissions.delete_own,
            delete_others: source.message_permissions.delete_others,
            pin: source.message_permissions.pin,
        },
        may_invite: source.may_invite,
        member_count: source.member_count,
        has_unread: source.has_unread,
        has_mentions: source.has_mentions,
        has_activity: source.has_activity,
        muted: source.muted(),
        alert: source.alert(),
        mention: source.mention(),
        hint: source.hint(),
        last_activity_ts: source.last_activity_ts,
        last_message_sender: source.last_message_sender.as_ref().map(NamedUser::shown),
        last_message_body: source.last_message_body.plain.clone(),
        last_message_is_own: source.last_message_is_own,
        last_message_edited: source.last_message_edited,
        successor: source.successor.as_ref().map(ToString::to_string),
        predecessor: source.predecessor.as_ref().map(ToString::to_string),
    }
}

fn space(source: &Space) -> SpaceDto {
    SpaceDto {
        id: source.id.clone(),
        name: source.name.clone(),
        member_count: source.member_count,
        child_room_ids: source.child_room_ids.clone(),
        child_space_ids: source.child_space_ids.clone(),
        order: source.order.clone(),
        may_invite: source.powers.invite,
        may_add_children: source.powers.add_children,
        alert: source.alert,
        mention: source.mention,
        hint: source.hint,
    }
}

fn space_match(source: &SpaceMatch) -> SpaceMatchDto {
    SpaceMatchDto {
        id: source.id.clone(),
        name: source.name.clone(),
        parent: source.parent.clone(),
        rooms: source.rooms,
        alert: source.flags.alert,
        mention: source.flags.mention,
        hint: source.flags.hint,
    }
}

fn directory(source: &DirectoryView) -> DirectoryDto {
    DirectoryDto {
        scope: names::room_scope(source.scope),
        space_id: source.space_id.clone(),
        subspace_id: source.subspace_id.clone(),
        listed_space: SpaceHeadingDto {
            id: source.listed_space.id.clone(),
            name: source.listed_space.name.clone(),
            member_count: source.listed_space.member_count,
            may_invite: source.listed_space.may_invite,
            may_add_children: source.listed_space.may_add_children,
        },
        direct_alert: source.direct_flags.alert,
        direct_mention: source.direct_flags.mention,
        direct_hint: source.direct_flags.hint,
        invites_pending: source.invites_pending,
        attention: source.attention,
        invites: source.invites.iter().map(|entry| invite(entry)).collect(),
        rooms: source.rooms.iter().map(|entry| room(entry)).collect(),
        space_matches: source.space_matches.iter().map(space_match).collect(),
        spaces: source.spaces.iter().map(space).collect(),
        subspaces: source.subspaces.iter().map(space).collect(),
    }
}

fn invite(source: &Invite) -> InviteDto {
    InviteDto {
        id: source.id.to_string(),
        name: source.name.clone(),
        kind: names::invite_kind(source.kind),
        inviter: source
            .inviter
            .as_ref()
            .map(|inviter| inviter.user_id.clone()),
        inviter_name: source.inviter.as_ref().map(NamedUser::shown),
        topic: source.topic.clone(),
        has_avatar_mxc: source.avatar_mxc.is_some(),
        received_ts: source.received_ts,
    }
}

fn invite_sheet(source: &InviteView) -> InviteSheetDto {
    InviteSheetDto {
        open: source.invite.is_some(),
        invite: source.invite.as_deref().map(invite),
        answer: names::pending_answer(source.answer),
        error: message(&source.error),
    }
}

fn room_preview(source: &RoomPreviewView) -> RoomPreviewDto {
    let shown = match &source.sheet {
        RoomPreviewSheet::Loaded(shown) => Some(shown),
        RoomPreviewSheet::Closed
        | RoomPreviewSheet::Idle
        | RoomPreviewSheet::Loading
        | RoomPreviewSheet::Missing
        | RoomPreviewSheet::Unavailable
        | RoomPreviewSheet::Failed => None,
    };
    RoomPreviewDto {
        status: names::room_preview_status(&source.sheet),
        address: (!source.address.is_empty()).then(|| source.address.clone()),
        entry: source.entry,
        access: names::child_access(source.access),
        room: shown.map(|shown| previewed_room(&shown.room)),
        opens_event: shown.is_some_and(|shown| shown.opens_event),
        error: message(&source.error),
        avatars_ready: source.avatars_ready,
    }
}

fn previewed_room(source: &RoomPreview) -> PreviewedRoomDto {
    PreviewedRoomDto {
        room_id: source.id.to_string(),
        name: source.name.clone(),
        alias: source.alias.clone(),
        topic: source.topic.clone(),
        members: source.member_count,
        is_space: source.is_space,
        join_rule: source.join_rule.as_ref().map(join_rule),
        membership: match source.membership {
            PreviewMembership::Outsider => "outsider",
            PreviewMembership::Joined => "joined",
            PreviewMembership::Invited => "invited",
            PreviewMembership::Knocked => "knocked",
            PreviewMembership::Banned => "banned",
        },
        has_avatar_mxc: source.avatar_mxc.is_some(),
    }
}

fn space_index(source: &SpaceIndexView) -> SpaceIndexDto {
    SpaceIndexDto {
        status: names::space_index_status(source.status),
        space_name: source.space_name.clone(),
        avatars_ready: source.avatars_ready,
        pages_landed: source.pages_landed,
        rows: source.rows.iter().map(space_child).collect(),
    }
}

fn room_info(source: &RoomInfoView) -> RoomInfoDto {
    let card = source.card.as_ref();
    let about = source.about.as_ref();
    RoomInfoDto {
        open: card.is_some(),
        placement: names::room_info_placement(source.placement),
        room_id: card.map(|card| card.id.to_string()),
        name: card.map(|card| card.name.clone()),
        member_count: card.map(|card| card.member_count),
        is_direct: card.is_some_and(|card| card.is_direct),
        topic: card.and_then(|card| card.topic.clone()),
        topic_html: about
            .and_then(|about| about.topic.as_ref())
            .and_then(|topic| topic.html().map(str::to_owned)),
        alias: card.and_then(|card| card.alias.clone()),
        joined_at: about.and_then(|about| about.joined_at),
        link: about.map(|about| about.link.clone()),
        notify: names::notify_mode(source.notify),
        notify_busy: source.notify_busy,
        leaving: source.leaving,
        may_invite: card.is_some_and(|card| card.may_invite),
        roster: names::roster_status(source.roster),
        has_more: source.has_more,
        pages_landed: source.pages_landed,
        avatars_ready: source.avatars_ready,
        error: message(&source.error),
        rows: source.rows.iter().map(member_row).collect(),
    }
}

fn user_info(source: &UserInfoView) -> UserInfoDto {
    let card = source.card.as_ref();
    let profile = card.map(|card| &card.profile);
    UserInfoDto {
        open: card.is_some(),
        status: card.map(|card| card_status(card.status)),
        room_id: card.map(|card| card.room_id.to_string()),
        user_id: profile.map(|profile| profile.user_id.to_string()),
        name: profile.and_then(|profile| profile.name.set().map(str::to_owned)),
        has_avatar_mxc: profile.is_some_and(|profile| profile.avatar_mxc.is_some()),
        pronouns: profile.and_then(|profile| match &profile.pronouns {
            Pronouns::Known(pronouns) => Some(pronouns.clone()),
            Pronouns::Unknown => None,
        }),
        role: profile.map(|profile| names::member_role(profile.role)),
        membership: profile.map(|profile| names::room_membership(profile.membership)),
        identity: profile.map_or("unverified", |profile| names::identity_trust(profile.trust)),
        is_self: profile.is_some_and(|profile| profile.is_self),
        link: profile.map(|profile| profile.link.clone()),
        direct_room: profile
            .and_then(|profile| profile.direct_room.as_ref().map(ToString::to_string)),
        direct: names::direct_chat(source.direct),
        ignored: profile.is_some_and(|profile| profile.ignored),
        ignore_busy: source.ignore_busy,
        may_kick: profile.is_some_and(|profile| profile.offers(Moderation::Kick)),
        may_ban: profile.is_some_and(|profile| profile.offers(Moderation::Ban)),
        may_unban: profile.is_some_and(|profile| profile.offers(Moderation::Unban)),
        moderating: names::pending_moderation(source.moderating),
        avatars_ready: source.avatars_ready,
        error: message(&source.error),
    }
}

fn card_status(status: CardStatus) -> &'static str {
    match status {
        CardStatus::Loaded => "loaded",
        CardStatus::ReadFailed => "read-failed",
        CardStatus::Retrying => "retrying",
    }
}

fn member_row(source: &RosterRow) -> MemberRowDto {
    match source {
        RosterRow::InvitedHeading => MemberRowDto {
            kind: "invited-heading",
            user_id: None,
            name: None,
            role: None,
            invited: true,
            has_avatar_mxc: false,
        },
        RosterRow::Member(member) => MemberRowDto {
            kind: "member",
            user_id: Some(member.user_id.clone()),
            name: Some(member.name.shown(&member.user_id)),
            role: Some(names::member_role(member.role)),
            invited: member.section == RosterSection::Invited,
            has_avatar_mxc: member.avatar_mxc.is_some(),
        },
    }
}

fn space_child(source: &SpaceIndexRow) -> SpaceChildDto {
    let child = &source.child;
    let (kind, children) = match child.kind {
        ChildKind::Room => ("room", None),
        ChildKind::Space { children } => ("space", Some(children)),
    };
    SpaceChildDto {
        id: child.id.to_string(),
        name: child.name.clone(),
        kind,
        children,
        members: child.member_count,
        join_rule: join_rule(&child.join_rule),
        via: child.via.clone(),
        has_avatar_mxc: child.avatar_mxc.is_some(),
        access: names::child_access(source.access),
    }
}

fn join_rule(source: &JoinRule) -> &'static str {
    match source {
        JoinRule::Public => "public",
        JoinRule::Restricted { .. } => "restricted",
        JoinRule::KnockRestricted { .. } => "knock_restricted",
        JoinRule::Knock => "knock",
        JoinRule::Invite => "invite",
        JoinRule::Private => "private",
        JoinRule::Unsupported => "unsupported",
    }
}

fn pagination(source: PaginationView) -> PaginationDto {
    PaginationDto {
        generation: source.generation,
        backwards_loading: source.older_history == OlderHistory::Loading,
        older_history: older_history(source.older_history),
        forwards_loading: source.forwards_loading,
        new_messages: source.new_messages,
    }
}

fn older_history(source: OlderHistory) -> &'static str {
    match source {
        OlderHistory::Unknown => "unknown",
        OlderHistory::Available => "available",
        OlderHistory::Loading => "loading",
        OlderHistory::Failed => "failed",
        OlderHistory::Ended => "ended",
    }
}

fn pinned_message(source: &PinnedMessage) -> PinnedMessageDto {
    PinnedMessageDto {
        event_id: source.event_id.clone(),
        kind: names::preview_kind(source.kind),
        body: source.body.plain.clone(),
    }
}

fn pinned(source: &PinnedView) -> PinnedDto {
    PinnedDto {
        room_id: source.room_id.as_ref().map(ToString::to_string),
        shown: source.shown,
        messages: source.messages.iter().map(pinned_message).collect(),
        pinned_ids: source.pinned_ids.iter().cloned().collect(),
    }
}

fn pack(source: &ImagePack) -> PackDto {
    let (title, untitled) = match &source.title {
        PackTitle::Named(name) => (name.clone(), false),
        PackTitle::Personal => (String::new(), true),
    };
    let origin = match source.origin {
        PackOrigin::Personal => "personal",
        PackOrigin::Room => "room",
        PackOrigin::Global => "global",
    };
    let shortcodes = |offered: fn(ImageUsage) -> bool| {
        source
            .images
            .iter()
            .filter(|image| offered(image.usage))
            .map(|image| image.shortcode.clone())
            .collect()
    };
    PackDto {
        id: source.id.to_string(),
        title,
        untitled,
        origin,
        shortcodes: shortcodes(|_| true),
        stickers: shortcodes(ImageUsage::offers_sticker),
    }
}

fn image_packs(source: &PackView) -> PacksDto {
    PacksDto {
        generation: source.generation,
        ready_images: source.ready_images,
        room_encrypted: source.room_encrypted,
        loading: source.loading,
        packs: source.packs.iter().map(pack).collect(),
    }
}

fn attachment(source: &AttachmentView) -> AttachmentDto {
    AttachmentDto {
        visible: source.visible,
        filename: source.filename.clone(),
        mimetype: source.mimetype.clone(),
        size: source.size,
        width: source.width,
        height: source.height,
        kind: names::attachment_kind(source.kind),
        duration_ms: source.duration.map(|value| value.as_millis()),
        has_preview: source.preview_path.is_some(),
        sending: source.sending,
        error: names::user_message_kind(source.error),
        error_detail: source.error_detail.clone(),
    }
}

fn video(source: &VideoView) -> VideoDto {
    VideoDto {
        visible: source.visible,
        loading: source.loading,
        has_path: source.path.is_some(),
        error: names::user_message_kind(source.error),
    }
}

fn unsent(source: &UnsentMessage) -> UnsentDto {
    let (reply_to, edits, caption) = match &source.draft {
        Draft::Message(message) => (
            message.reply.as_ref().map(|reply| reply.event_id.clone()),
            None,
            false,
        ),
        Draft::Edit(edit) => (
            None,
            edit.target
                .event_id()
                .or_else(|| edit.target.local_id())
                .map(str::to_owned),
            edit.kind == EditKind::Caption,
        ),
    };
    UnsentDto {
        submission: source.submission,
        room_id: source.room_id.to_string(),
        body: source.draft.body().to_owned(),
        reply_to,
        edits,
        caption,
        emotes: source
            .draft
            .emotes()
            .iter()
            .map(|emote| emote.token.clone())
            .collect(),
    }
}

fn audio(source: &AudioView) -> Option<NowPlayingDto> {
    let now = source.now_playing.as_ref()?;
    Some(NowPlayingDto {
        request: now.request,
        room_id: now.room_id.to_string(),
        event_id: now.event_id.clone(),
        sender: now.sender.clone(),
        kind: names::audio_kind(now.meta.kind),
        title: now.meta.filename.clone(),
        duration_ms: now.meta.duration.map(|value| value.as_millis()),
        has_waveform: now.meta.waveform.is_some(),
        downloaded: matches!(now.file, TrackFile::Ready(_)),
    })
}

fn toast(source: &Toast) -> ToastDto {
    match source {
        Toast::None => ToastDto {
            kind: "none",
            detail: None,
        },
        Toast::Error(message) => ToastDto {
            kind: names::user_message_kind(message.kind),
            detail: Some(message.detail.clone()),
        },
        Toast::FileSaved(path) => ToastDto {
            kind: "file-saved",
            detail: Some(path.clone()),
        },
    }
}
