use std::collections::HashMap;

use serde::Deserialize;

use super::messages::{HistoryDto, ServiceDto};
use super::{ago_ms, rich_text};
use crate::adapters::demo::{create_room, data, invite_people, powers};
use crate::domain::event_size::EventIds;
use crate::domain::invite::{Invite, InviteKind};
use crate::domain::member_name::NamedUser;
use crate::domain::message::{MessagePreviewKind, RichText};
use crate::domain::room::{NotifyMode, Room, RoomId, Space};
use crate::domain::room_info::MemberRole;
use crate::domain::room_preview::{PreviewMembership, RoomPreview};
use crate::domain::room_state::HistoryVisibility;
use crate::domain::space_index::{ChildKind, JoinRule, SpaceChild};
use crate::domain::user_info::UserId;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviteDto {
    pub id: String,
    name: String,
    #[serde(default)]
    avatar: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    pub inviter: Option<String>,
    #[serde(default)]
    pub direct: bool,
    #[serde(default)]
    pub space: bool,
    #[serde(default)]
    encrypted: bool,
    #[serde(default)]
    members: u64,
    #[serde(default)]
    minutes_ago: u64,
    #[serde(default)]
    rooms: Vec<String>,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum NotifyDto {
    #[default]
    All,
    Mentions,
    Muted,
}

impl NotifyDto {
    fn to_mode(self) -> NotifyMode {
        match self {
            Self::All => NotifyMode::AllMessages,
            Self::Mentions => NotifyMode::MentionsOnly,
            Self::Muted => NotifyMode::Muted,
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum RoleDto {
    Owner,
    Admin,
    Moderator,
}

impl RoleDto {
    pub fn to_role(self) -> MemberRole {
        match self {
            Self::Owner => MemberRole::Owner,
            Self::Admin => MemberRole::Admin,
            Self::Moderator => MemberRole::Moderator,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDto {
    pub id: String,
    name: String,
    avatar: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    topic_html: Option<String>,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    joined_days_ago: Option<u64>,
    #[serde(default)]
    pub roles: HashMap<String, RoleDto>,
    #[serde(default)]
    pub invited: Vec<String>,
    #[serde(default)]
    direct: bool,
    #[serde(default)]
    pub encrypted: bool,
    #[serde(default)]
    history_visibility: HistoryDto,
    #[serde(default)]
    pub members: u64,
    #[serde(default)]
    pub unread: u64,
    #[serde(default)]
    mentions: u64,
    #[serde(default)]
    notify: NotifyDto,
    #[serde(default)]
    minutes_ago: u64,
    #[serde(default)]
    days_ago: u64,
    #[serde(default)]
    pub last_message: LastMessageDto,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default)]
    pub successor: Option<String>,
    #[serde(default)]
    pub predecessor: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LastMessageDto {
    pub sender: Option<String>,
    pub sender_id: Option<String>,
    #[serde(default)]
    kind: KindDto,
    #[serde(default)]
    pub body: String,
    html: Option<String>,
    service: Option<ServiceDto>,
    #[serde(default)]
    pub own: bool,
    #[serde(default)]
    edited: bool,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(super) enum KindDto {
    #[default]
    Text,
    Image,
    Video,
    Audio,
    Voice,
    File,
    Location,
    Encrypted,
    Sticker,
    Poll,
    None,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceDto {
    id: String,
    name: String,
    avatar: Option<String>,
    #[serde(default)]
    members: u64,
    #[serde(default)]
    rooms: Vec<String>,
    #[serde(default)]
    spaces: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnjoinedDto {
    pub id: String,
    name: String,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    avatar: Option<String>,
    #[serde(default)]
    members: u64,
    #[serde(default)]
    join_rule: JoinRuleDto,
    #[serde(default)]
    allowed: Vec<String>,
    #[serde(default)]
    pub space: bool,
    #[serde(default)]
    pub rooms: Vec<String>,
    #[serde(default)]
    pub spaces: Vec<String>,
    #[serde(default)]
    pub successor: Option<String>,
    #[serde(default)]
    pub predecessor: Option<String>,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(super) enum JoinRuleDto {
    #[default]
    Public,
    Restricted,
    KnockRestricted,
    Knock,
    Invite,
    Private,
}

impl JoinRuleDto {
    pub(super) fn to_rule(self, allowed: Vec<RoomId>) -> JoinRule {
        match self {
            Self::Public => JoinRule::Public,
            Self::Restricted => JoinRule::Restricted { allowed },
            Self::KnockRestricted => JoinRule::KnockRestricted { allowed },
            Self::Knock => JoinRule::Knock,
            Self::Invite => JoinRule::Invite,
            Self::Private => JoinRule::Private,
        }
    }
}

impl RoomDto {
    pub fn is_direct(&self) -> bool {
        self.direct
    }

    pub fn to_room(&self, now_ms: u64, notify: Option<NotifyMode>) -> Room {
        let notify = notify.unwrap_or_else(|| self.notify.to_mode());
        let last_activity_ts = ago_ms(now_ms, self.minutes_ago, self.days_ago);
        Room {
            avatar_mxc: self.avatar.clone(),
            topic: self.topic.clone(),
            canonical_alias: self.alias.clone(),
            is_direct: self.direct,
            direct_partner: direct_partner(self.direct, self.avatar.as_deref()),
            is_encrypted: self.encrypted,
            history_visibility: self.history_visibility.to_visibility(),
            member_count: self.members,
            has_unread: self.unread > 0 && notify == NotifyMode::AllMessages,
            has_mentions: self.mentions > 0,
            has_activity: self.unread > 0,
            notify,
            last_message_sender: self.last_message.named_sender(&self.id),
            last_message_kind: self.last_message.kind.to_kind(),
            last_message_body: self.last_message.rich_body(),
            last_message_service: self.last_message.service.as_ref().map(ServiceDto::to_event),
            last_message_is_own: self.last_message.own,
            last_message_edited: self.last_message.edited,
            successor: self.successor.as_deref().map(RoomId::new),
            predecessor: self.predecessor.as_deref().map(RoomId::new),
            ..quiet_room(RoomId::new(&self.id), self.name.clone(), last_activity_ts)
        }
    }

    pub fn joined_at(&self, now_ms: u64) -> Option<u64> {
        self.joined_days_ago.map(|days| ago_ms(now_ms, 0, days))
    }

    pub fn rich_topic(&self) -> Option<RichText> {
        let plain = self.topic.clone()?;
        Some(RichText::with_html(plain, self.topic_html.clone()))
    }
}

impl UnjoinedDto {
    pub fn to_child(&self, via: &[String]) -> SpaceChild {
        SpaceChild {
            id: RoomId::new(&self.id),
            name: self.name.clone(),
            alias: self.alias.clone(),
            topic: self.topic.clone(),
            avatar_mxc: self.avatar.clone(),
            member_count: self.members,
            join_rule: self
                .join_rule
                .to_rule(self.allowed.iter().map(RoomId::new).collect()),
            kind: if self.space {
                ChildKind::Space {
                    children: child_count(&self.rooms, &self.spaces),
                }
            } else {
                ChildKind::Room
            },
            via: via.to_vec(),
        }
    }

    pub fn to_room(&self, now_ms: u64) -> Room {
        Room {
            avatar_mxc: self.avatar.clone(),
            topic: self.topic.clone(),
            canonical_alias: self.alias.clone(),
            member_count: self.joined_members(),
            successor: self.successor.as_deref().map(RoomId::new),
            predecessor: self.predecessor.as_deref().map(RoomId::new),
            ..quiet_room(RoomId::new(&self.id), self.name.clone(), now_ms)
        }
    }

    pub fn to_space(&self) -> Space {
        Space {
            id: self.id.clone(),
            name: self.name.clone(),
            avatar_mxc: self.avatar.clone(),
            member_count: self.members.saturating_add(1),
            child_room_ids: self.rooms.clone(),
            child_space_ids: self.spaces.clone(),
            order: None,
            powers: create_room::space_powers(),
            alert: false,
            mention: false,
            hint: false,
        }
    }
}

impl InviteDto {
    pub fn to_invite(&self, now_ms: u64, inviter: Option<NamedUser>) -> Invite {
        Invite {
            id: RoomId::new(&self.id),
            name: self.name.clone(),
            avatar_mxc: self.avatar.clone(),
            topic: self.topic.clone(),
            kind: InviteKind::of(self.space, self.direct),
            inviter,
            received_ts: ago_ms(now_ms, self.minutes_ago, 0),
        }
    }

    pub fn to_room(&self, now_ms: u64) -> Room {
        Room {
            avatar_mxc: self.avatar.clone(),
            topic: self.topic.clone(),
            is_direct: self.direct,
            direct_partner: direct_partner(self.direct, self.inviter.as_deref()),
            is_encrypted: self.encrypted,
            member_count: self.joined_members(),
            ..quiet_room(RoomId::new(&self.id), self.name.clone(), now_ms)
        }
    }

    pub fn joined_members(&self) -> u64 {
        self.members.saturating_add(1)
    }

    pub fn rich_topic(&self) -> Option<RichText> {
        self.topic.clone().map(RichText::plain)
    }

    pub fn to_preview(&self) -> RoomPreview {
        RoomPreview {
            id: RoomId::new(&self.id),
            name: self.name.clone(),
            alias: None,
            topic: self.topic.clone(),
            avatar_mxc: self.avatar.clone(),
            member_count: self.joined_members(),
            is_space: self.space,
            join_rule: Some(JoinRule::Invite),
            membership: PreviewMembership::Invited,
        }
    }

    pub fn to_space(&self) -> Space {
        Space {
            id: self.id.clone(),
            name: self.name.clone(),
            avatar_mxc: self.avatar.clone(),
            member_count: self.joined_members(),
            child_room_ids: self.rooms.clone(),
            child_space_ids: Vec::new(),
            order: None,
            powers: create_room::space_powers(),
            alert: false,
            mention: false,
            hint: false,
        }
    }
}

impl UnjoinedDto {
    pub fn avatar(&self) -> Option<&str> {
        self.avatar.as_deref()
    }

    pub fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    pub fn rich_topic(&self) -> Option<RichText> {
        self.topic.clone().map(RichText::plain)
    }

    pub fn joined_members(&self) -> u64 {
        self.members.saturating_add(1)
    }
}

pub fn child_count(rooms: &[String], spaces: &[String]) -> u64 {
    u64::try_from(rooms.len().saturating_add(spaces.len())).unwrap_or(u64::MAX)
}

impl RoomDto {
    pub fn to_child(&self, via: &[String]) -> SpaceChild {
        SpaceChild {
            id: RoomId::new(&self.id),
            name: self.name.clone(),
            alias: None,
            topic: None,
            avatar_mxc: self.avatar.clone(),
            member_count: self.members,
            join_rule: JoinRule::Public,
            kind: ChildKind::Room,
            via: via.to_vec(),
        }
    }
}

impl SpaceDto {
    pub fn to_child(&self, via: &[String]) -> SpaceChild {
        SpaceChild {
            id: RoomId::new(&self.id),
            name: self.name.clone(),
            alias: None,
            topic: None,
            avatar_mxc: self.avatar.clone(),
            member_count: self.members,
            join_rule: JoinRule::Public,
            kind: ChildKind::Space {
                children: child_count(&self.rooms, &self.spaces),
            },
            via: via.to_vec(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn children(&self) -> impl Iterator<Item = &String> {
        self.rooms.iter().chain(&self.spaces)
    }

    pub fn to_space(&self) -> Space {
        Space {
            id: self.id.clone(),
            name: self.name.clone(),
            avatar_mxc: self.avatar.clone(),
            member_count: self.members,
            child_room_ids: self.rooms.clone(),
            child_space_ids: self.spaces.clone(),
            order: None,
            powers: create_room::space_powers(),
            alert: false,
            mention: false,
            hint: false,
        }
    }
}

impl LastMessageDto {
    fn rich_body(&self) -> RichText {
        rich_text(&self.body, self.html.as_deref())
    }

    fn named_sender(&self, room_id: &str) -> Option<NamedUser> {
        if self.sender.is_none() && self.sender_id.is_none() {
            return None;
        }
        let user_id = self.sender_id.clone().unwrap_or_default();
        let book = data::names_in(&RoomId::new(room_id), []);
        let name = book.name(&user_id, self.sender.as_deref());
        Some(NamedUser::new(user_id, name))
    }
}

impl KindDto {
    pub(super) fn to_kind(self) -> MessagePreviewKind {
        match self {
            Self::Text => MessagePreviewKind::Text,
            Self::Image => MessagePreviewKind::Image,
            Self::Video => MessagePreviewKind::Video,
            Self::Audio => MessagePreviewKind::Audio,
            Self::Voice => MessagePreviewKind::Voice,
            Self::File => MessagePreviewKind::File,
            Self::Location => MessagePreviewKind::Location,
            Self::Encrypted => MessagePreviewKind::Encrypted,
            Self::Sticker => MessagePreviewKind::Sticker,
            Self::Poll => MessagePreviewKind::Poll,
            Self::None => MessagePreviewKind::None,
        }
    }
}

fn direct_partner(direct: bool, person: Option<&str>) -> Option<UserId> {
    person
        .filter(|user_id| direct && user_id.starts_with('@'))
        .map(UserId::new)
}

pub fn quiet_room(id: RoomId, display_name: String, last_activity_ts: u64) -> Room {
    Room {
        id,
        display_name,
        avatar_mxc: None,
        topic: None,
        canonical_alias: None,
        is_direct: false,
        direct_partner: None,
        is_encrypted: false,
        event_ids: EventIds::Hashed,
        history_visibility: HistoryVisibility::default(),
        poll_permissions: powers::polls(),
        message_permissions: powers::messages(),
        may_invite: invite_people::may_invite(),
        may_join_calls: true,
        call: None,
        member_count: 1,
        has_unread: false,
        has_mentions: false,
        has_activity: false,
        notify: NotifyMode::AllMessages,
        last_activity_ts,
        last_message_sender: None,
        last_message_kind: MessagePreviewKind::None,
        last_message_body: RichText::default(),
        last_message_service: None,
        last_message_is_own: false,
        last_message_edited: false,
        successor: None,
        predecessor: None,
    }
}
