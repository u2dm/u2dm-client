use std::sync::Arc;

use crate::commands::messages::UserMessage;
use crate::domain::departure::{AdminStanding, Removal, RemovalList};
use crate::domain::invite::{Invite, InviteList};
use crate::domain::invite_people::Person;
use crate::domain::room::{NotifyMode, RoomId, RoomList, Space, UnreadFlags};
use crate::domain::room_creation::RoomKind;
use crate::domain::room_info::{RoomAbout, RosterMember};
use crate::domain::room_preview::RoomPreview;
use crate::domain::space_index::SpaceChild;
use crate::domain::user_info::{UserDevice, UserProfile};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum RoomScope {
    #[default]
    All,
    Direct,
    Space,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum InvitesSection {
    #[default]
    Folded,
    Unfolded,
    Matches,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SpaceHeading {
    pub id: String,
    pub name: String,
    pub member_count: u64,
    pub may_invite: bool,
    pub may_add_children: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SpaceMatch {
    pub id: String,
    pub name: String,
    pub avatar_mxc: Option<String>,
    pub parent: Option<String>,
    pub rooms: usize,
    pub flags: UnreadFlags,
}

#[derive(Clone, Default)]
pub struct DirectoryView {
    pub rooms: RoomList,
    pub invites: InviteList,
    pub invites_section: InvitesSection,
    pub removals: RemovalList,
    pub space_matches: Arc<[SpaceMatch]>,
    pub spaces: Arc<[Space]>,
    pub subspaces: Arc<[Space]>,
    pub scope: RoomScope,
    pub space_id: String,
    pub space_name: String,
    pub subspace_id: String,
    pub listed_space: SpaceHeading,
    pub direct_flags: UnreadFlags,
    pub invites_pending: bool,
    pub attention: usize,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum CreateRoomStatus {
    #[default]
    Closed,
    Editing,
    Creating,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct CreateRoomView {
    pub status: CreateRoomStatus,
    pub kind: RoomKind,
    pub parent_name: String,
    pub server_name: String,
    pub error: UserMessage,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum PeopleSearchStatus {
    #[default]
    Idle,
    Searching,
    Ready,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InviteeState {
    Available,
    Picked,
    Joined,
    Invited,
    Banned,
    Sending,
    Failed,
}

#[derive(Clone, PartialEq, Eq)]
pub struct InviteeRow {
    pub person: Arc<Person>,
    pub state: InviteeState,
    pub failure: UserMessage,
}

#[derive(Clone, PartialEq, Eq)]
pub struct InviteTarget {
    pub room_id: RoomId,
    pub name: String,
    pub space: bool,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct InvitePeopleView {
    pub target: Option<InviteTarget>,
    pub search: PeopleSearchStatus,
    pub results: Arc<[InviteeRow]>,
    pub picked: Arc<[InviteeRow]>,
    pub limited: bool,
    pub sending: bool,
    pub error: UserMessage,
    pub avatars_ready: usize,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum PendingAnswer {
    #[default]
    None,
    Accept,
    Decline,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct InviteView {
    pub invite: Option<Arc<Invite>>,
    pub answer: PendingAnswer,
    pub error: UserMessage,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct RemovalView {
    pub removal: Option<Arc<Removal>>,
    pub forgetting: bool,
    pub error: UserMessage,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SpaceToLeave {
    pub id: RoomId,
    pub name: String,
    pub rooms: usize,
    pub spaces: usize,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct LeaveSpaceView {
    pub space: Option<SpaceToLeave>,
    pub standing: Option<AdminStanding>,
    pub leaving: bool,
    pub error: UserMessage,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum RoomPreviewSheet {
    #[default]
    Closed,
    Idle,
    Loading,
    Loaded(ShownPreview),
    Missing,
    Unavailable,
    Failed,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ShownPreview {
    pub room: Arc<RoomPreview>,
    pub opens_event: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RoomPreviewView {
    pub sheet: RoomPreviewSheet,
    pub address: String,
    pub entry: bool,
    pub access: ChildAccess,
    pub error: UserMessage,
    pub avatars_ready: usize,
}

impl Default for RoomPreviewView {
    fn default() -> Self {
        Self {
            sheet: RoomPreviewSheet::default(),
            address: String::new(),
            entry: false,
            access: ChildAccess::Join,
            error: UserMessage::default(),
            avatars_ready: 0,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SpaceIndexStatus {
    #[default]
    Closed,
    Loading,
    Partial,
    Complete,
    LoadingMore,
    Failed,
    MoreFailed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChildAccess {
    Open,
    OpenSubspace,
    Joined,
    Joining,
    Join,
    InviteOnly,
    Knock,
    MembersOnly,
    Unavailable,
}

#[derive(Clone, PartialEq)]
pub struct SpaceIndexRow {
    pub child: Arc<SpaceChild>,
    pub access: ChildAccess,
}

#[derive(Clone, Default)]
pub struct SpaceIndexView {
    pub status: SpaceIndexStatus,
    pub space_name: String,
    pub rows: Arc<[SpaceIndexRow]>,
    pub avatars_ready: usize,
    pub pages_landed: i32,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RoomCard {
    pub id: RoomId,
    pub name: String,
    pub avatar_mxc: Option<String>,
    pub member_count: u64,
    pub is_direct: bool,
    pub topic: Option<String>,
    pub alias: Option<String>,
    pub notify: NotifyMode,
    pub may_invite: bool,
    pub readable_by_anyone: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum RosterStatus {
    #[default]
    Loading,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum RoomInfoPlacement {
    #[default]
    Closed,
    Dialog,
    Pane,
}

#[derive(Clone, PartialEq, Eq)]
pub enum RosterRow {
    Member(Arc<RosterMember>),
    InvitedHeading,
}

#[derive(Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct RoomMenuTarget {
    pub room_id: RoomId,
    pub name: String,
    pub is_space: bool,
    pub unread: bool,
    pub notify: NotifyMode,
    pub notify_busy: bool,
    pub leaving: bool,
    pub may_invite: bool,
    pub may_add_children: bool,
    pub has_call: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CardStatus {
    Loaded,
    ReadFailed,
    Retrying,
}

#[derive(Clone, PartialEq, Eq)]
pub struct UserCard {
    pub room_id: RoomId,
    pub profile: UserProfile,
    pub status: CardStatus,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum DirectChat {
    #[default]
    Hidden,
    Open,
    Start,
    Starting,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum PendingModeration {
    #[default]
    None,
    Kick,
    Ban,
    Unban,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum VerifyOffer {
    #[default]
    Hidden,
    Offered,
    Requesting,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct UserInfoView {
    pub card: Option<UserCard>,
    pub direct: DirectChat,
    pub verify: VerifyOffer,
    pub sessions: Option<Arc<[UserDevice]>>,
    pub ignore_busy: bool,
    pub moderating: PendingModeration,
    pub avatars_ready: usize,
    pub error: UserMessage,
}

#[derive(Clone, Default)]
pub struct RoomInfoView {
    pub placement: RoomInfoPlacement,
    pub card: Option<RoomCard>,
    pub about: Option<RoomAbout>,
    pub roster: RosterStatus,
    pub rows: Arc<[RosterRow]>,
    pub has_more: bool,
    pub pages_landed: i32,
    pub avatars_ready: usize,
    pub notify: NotifyMode,
    pub notify_busy: bool,
    pub leaving: bool,
    pub error: UserMessage,
}
