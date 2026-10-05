use serde::Serialize;

use super::names;
use crate::commands::messages::UserMessage;
use crate::commands::ui::Draft;
use crate::commands::view::{
    AppViewState, AttachmentView, AudioView, CardStatus, CopiedLink, DirectoryView, LifecycleView,
    PaginationView, PinnedView, RoomInfoView, RoomLogView, RoomMenuTarget, RosterRow, SourceState,
    SpaceIndexRow, SpaceIndexView, StickerView, Toast, TrackFile, UnsentMessage, UserInfoView,
    VideoView,
};
use crate::domain::message::{EditKind, PinnedMessage};
use crate::domain::room::{Room, Space};
use crate::domain::room_info::RosterSection;
use crate::domain::space_index::{ChildKind, JoinRule};
use crate::domain::sticker::StickerPack;
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::{OlderHistory, SourceEncryption};
use crate::domain::user_info::{IdentityTrust, Moderation, Pronouns};

#[derive(Serialize)]
pub struct ViewDto {
    lifecycle: LifecycleDto,
    connection: ConnectionDto,
    directory: DirectoryDto,
    space_index: SpaceIndexDto,
    room_info: RoomInfoDto,
    room_menu: Option<RoomMenuDto>,
    user_info: UserInfoDto,
    pagination: PaginationDto,
    pinned: PinnedDto,
    stickers: StickersDto,
    attachment: AttachmentDto,
    video: VideoDto,
    audio: Option<NowPlayingDto>,
    unsent: Option<UnsentDto>,
    toast: ToastDto,
    message_link: LinkDto,
    room_link: LinkDto,
    source: SourceDto,
    room_log: Option<RoomLogDto>,
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
struct RoomMenuDto {
    room_id: String,
    name: String,
    unread: bool,
    notify: &'static str,
    notify_busy: bool,
    leaving: bool,
}

#[derive(Serialize)]
struct MessageDto {
    kind: &'static str,
    detail: String,
}

#[derive(Serialize)]
struct LifecycleDto {
    step: &'static str,
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
}

#[derive(Serialize)]
struct SpaceDto {
    id: String,
    name: String,
    member_count: u64,
    child_room_ids: Vec<String>,
    child_space_ids: Vec<String>,
    order: Option<String>,
    alert: bool,
    mention: bool,
    hint: bool,
}

#[derive(Serialize)]
struct SpaceHeadingDto {
    name: String,
    member_count: u64,
}

#[derive(Serialize)]
struct DirectoryDto {
    scope: &'static str,
    space_id: String,
    subspace_id: String,
    listed_space: SpaceHeadingDto,
    direct_alert: bool,
    direct_mention: bool,
    direct_hint: bool,
    rooms: Vec<RoomDto>,
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
    verified: bool,
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
    shortcodes: Vec<String>,
}

#[derive(Serialize)]
struct StickersDto {
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
        room_info: room_info(&source.room_info),
        room_menu: source.room_menu.as_ref().map(room_menu),
        user_info: user_info(&source.user_info),
        pagination: pagination(source.pagination),
        pinned: pinned(&source.pinned),
        stickers: stickers(&source.stickers),
        attachment: attachment(&source.attachment),
        video: video(&source.video),
        audio: audio(&source.audio),
        unsent: source.unsent.as_ref().map(unsent),
        toast: toast(&source.toast),
        message_link: link(&source.message_link),
        room_link: link(&source.room_link),
        source: event_source(&source.source),
        room_log: source.room_log.as_ref().map(room_log),
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
        activity: names::login_activity(source.activity),
        method: names::login_method(source.method),
        resolved_homeserver: source.resolved_homeserver.clone(),
        user_id: source.user_id.clone(),
        has_avatar: source.avatar_path.is_some(),
        messages: source.messages.iter().map(message).collect(),
    }
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
        member_count: source.member_count,
        has_unread: source.has_unread,
        has_mentions: source.has_mentions,
        has_activity: source.has_activity,
        muted: source.muted(),
        alert: source.alert(),
        mention: source.mention(),
        hint: source.hint(),
        last_activity_ts: source.last_activity_ts,
        last_message_sender: source.last_message_sender.clone(),
        last_message_body: source.last_message_body.plain.clone(),
        last_message_is_own: source.last_message_is_own,
        last_message_edited: source.last_message_edited,
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
        alert: source.alert,
        mention: source.mention,
        hint: source.hint,
    }
}

fn directory(source: &DirectoryView) -> DirectoryDto {
    DirectoryDto {
        scope: names::room_scope(source.scope),
        space_id: source.space_id.clone(),
        subspace_id: source.subspace_id.clone(),
        listed_space: SpaceHeadingDto {
            name: source.listed_space.name.clone(),
            member_count: source.listed_space.member_count,
        },
        direct_alert: source.direct_flags.alert,
        direct_mention: source.direct_flags.mention,
        direct_hint: source.direct_flags.hint,
        rooms: source.rooms.iter().map(|entry| room(entry)).collect(),
        spaces: source.spaces.iter().map(space).collect(),
        subspaces: source.subspaces.iter().map(space).collect(),
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
            .and_then(|topic| topic.html.clone()),
        alias: card.and_then(|card| card.alias.clone()),
        joined_at: about.and_then(|about| about.joined_at),
        link: about.map(|about| about.link.clone()),
        notify: names::notify_mode(source.notify),
        notify_busy: source.notify_busy,
        leaving: source.leaving,
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
        name: profile.and_then(|profile| profile.display_name.clone()),
        has_avatar_mxc: profile.is_some_and(|profile| profile.avatar_mxc.is_some()),
        pronouns: profile.and_then(|profile| match &profile.pronouns {
            Pronouns::Known(pronouns) => Some(pronouns.clone()),
            Pronouns::Unknown => None,
        }),
        role: profile.map(|profile| names::member_role(profile.role)),
        membership: profile.map(|profile| names::room_membership(profile.membership)),
        verified: profile.is_some_and(|profile| profile.trust == IdentityTrust::Verified),
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
            name: Some(member.label().to_owned()),
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

fn pack(source: &StickerPack) -> PackDto {
    PackDto {
        id: source.id.to_string(),
        title: source.title.clone(),
        shortcodes: source
            .images
            .iter()
            .map(|image| image.shortcode.clone())
            .collect(),
    }
}

fn stickers(source: &StickerView) -> StickersDto {
    StickersDto {
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
