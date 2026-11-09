use std::fmt;
use std::sync::Arc;

use crate::domain::call::MemberKey;
use crate::domain::call::devices::{ByKind, DeviceKey, DeviceKind};
use crate::domain::call::media::MediaChoice;
use crate::domain::call::reactions::Reaction;
use crate::domain::call::ring::RingKind;
use crate::domain::call::video::{FeedId, FrameSize, ScreenRefusal, VideoKind};
use crate::domain::member_name::NamedUser;
use crate::domain::room::{RoomId, UnreadFlags};

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct RingView {
    pub serial: u64,
    pub room_id: RoomId,
    pub room_name: String,
    pub room_avatar_mxc: Option<String>,
    pub direct: bool,
    pub caller: NamedUser,
    pub video: bool,
    pub quiet: bool,
    pub sounds: bool,
    pub desktop: bool,
    pub answerable: bool,
    pub refusal: Option<JoinRefusal>,
    pub devices: Option<CallDevices>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomCallSummary {
    pub room_id: RoomId,
    pub people: usize,
    pub faces: Arc<[CallFace]>,
    pub faces_ready: u64,
    pub video: bool,
    pub joined_by_you: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFace {
    pub user: NamedUser,
    pub avatar_mxc: Option<String>,
    pub you: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinRefusal {
    NotBuilt,
    MediaBroken,
    NoServer,
    ServerRefused,
    NotAllowed,
    SessionUnverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinOffer {
    Start,
    Join,
    InCall,
    Refused(JoinRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferView {
    pub room_id: RoomId,
    pub offer: JoinOffer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeavingCall {
    pub room_id: RoomId,
    pub room_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSheetView {
    pub serial: i32,
    pub room_id: RoomId,
    pub room_name: String,
    pub direct: bool,
    pub offer: JoinOffer,
    pub video: bool,
    pub people: usize,
    pub others: Arc<[NamedUser]>,
    pub bridges: Arc<[NamedUser]>,
    pub devices: Option<CallDevices>,
    pub proposed: MediaChoice,
    pub preview: Option<SheetPreview>,
    pub tests: SheetTests,
    pub leaving: Option<LeavingCall>,
    pub ended: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetTests {
    NotOffered,
    Idle,
    Testing(DeviceTestView),
}

impl SheetTests {
    pub const fn offered(self) -> bool {
        !matches!(self, Self::NotOffered)
    }

    pub const fn running(self) -> Option<DeviceTestView> {
        match self {
            Self::Testing(test) => Some(test),
            Self::NotOffered | Self::Idle => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceTestView {
    pub kind: DeviceKind,
    pub phase: TestPhase,
    pub level: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestPhase {
    Starting,
    Running,
    Failed,
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetPreview {
    Starting,
    Local(FeedId),
    InCall { feed: FeedId, room_name: String },
    Elsewhere { room_name: String },
    Failed(PreviewTrouble),
}

impl SheetPreview {
    pub const fn feed(&self) -> Option<&FeedId> {
        match self {
            Self::Local(feed) | Self::InCall { feed, .. } => Some(feed),
            Self::Starting | Self::Elsewhere { .. } | Self::Failed(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewTrouble {
    NoCamera,
    NotStarted,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPhase {
    Joining,
    Calling,
    Connecting,
    Waiting,
    Live,
    Reconnecting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallNotice {
    NotAnnounced(RingKind),
    MicrophoneFailed,
    CameraFailed,
    ScreenFailed,
    ChoosingScreen,
    SharingNeverStarted,
    ConnectionPoor,
    CantDecrypt {
        who: NamedUser,
        why: CantDecrypt,
        others: usize,
    },
    NotReceived {
        who: NamedUser,
        what: MissingMedia,
        others: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallFailure {
    ServerUnreachable,
    ServerRefused,
    JoinFailed,
    MediaFailed,
    CutOff,
    NotEncrypted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedCall {
    pub room_id: RoomId,
    pub room_name: String,
    pub failure: CallFailure,
    pub retry: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CantDecrypt {
    NotCrossSigned,
    NoKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingMedia {
    Audio,
    Video,
    AudioAndVideo,
    Screen,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileState {
    Joining,
    WaitingForKeys,
    Live,
    PoorConnection,
    VideoStalled,
    NotReceived(MissingMedia),
    ConnectionLost,
    CantDecrypt(CantDecrypt),
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileKind {
    Person,
    Screen,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub member: MemberKey,
    pub kind: TileKind,
}

impl fmt::Display for TileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            TileKind::Person => "person",
            TileKind::Screen => "screen",
        };
        write!(f, "{} {kind}", self.member)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct CallTile {
    pub member: MemberKey,
    pub name: NamedUser,
    pub device: u8,
    pub avatar_mxc: Option<String>,
    pub own: bool,
    pub kind: TileKind,
    pub state: TileState,
    pub muted: bool,
    pub muted_for_you: bool,
    pub speaking: bool,
    pub hand: Option<usize>,
    pub reaction: Option<Reaction>,
    pub video: Option<FeedId>,
}

impl CallTile {
    pub fn key(&self) -> TileKey {
        TileKey {
            member: self.member.clone(),
            kind: self.kind,
        }
    }

    pub const fn is_self_view(&self) -> bool {
        self.own && matches!(self.kind, TileKind::Person)
    }

    pub const fn is_own_share(&self) -> bool {
        self.own && matches!(self.kind, TileKind::Screen)
    }

    pub const fn is_remote_share(&self) -> bool {
        !self.own && matches!(self.kind, TileKind::Screen)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareNews {
    pub serial: u64,
    pub who: NamedUser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct CallPerson {
    pub user: NamedUser,
    pub avatar_mxc: Option<String>,
    pub you: bool,
    pub devices: usize,
    pub muted: bool,
    pub muted_for_you: bool,
    pub hand: Option<usize>,
    pub reaction: Option<Reaction>,
    pub sharing: bool,
    pub sees_no_video: bool,
    pub exposes_e2ee: bool,
    pub state: TileState,
    pub tile: Option<TileKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    SystemDefault,
    Connected,
    Missing,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceOption {
    pub key: Option<DeviceKey>,
    pub name: String,
    pub status: DeviceStatus,
    pub chosen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTrouble {
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceNews {
    pub serial: u64,
    pub kind: DeviceKind,
    pub name: String,
    pub trouble: DeviceTrouble,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallDevices {
    pub options: ByKind<Arc<[DeviceOption]>>,
    pub news: Option<DeviceNews>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrophoneView {
    On,
    Off,
    Starting,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraView {
    Off,
    Starting,
    On,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenView {
    Off,
    Choosing,
    On,
    Failed,
    Unavailable(ScreenRefusal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandView {
    Unavailable,
    Down,
    Raising,
    Up,
    Lowering,
    RaiseFailed,
    LowerFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandNews {
    pub serial: u64,
    pub who: NamedUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionView {
    Unavailable,
    Ready,
    Sending,
    Showing,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionNews {
    pub serial: u64,
    pub who: NamedUser,
    pub reaction: Reaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct ActiveCall {
    pub call: u64,
    pub room_id: RoomId,
    pub room_name: String,
    pub room_unread: UnreadFlags,
    pub phase: CallPhase,
    pub live_since_ms: Option<u64>,
    pub encrypted: bool,
    pub video: bool,
    pub microphone: MicrophoneView,
    pub microphone_level: Option<u8>,
    pub camera: CameraView,
    pub screen: ScreenView,
    pub hand: HandView,
    pub hands_raised: usize,
    pub hand_news: Option<HandNews>,
    pub reaction: ReactionView,
    pub reaction_news: Option<ReactionNews>,
    pub share_preview: bool,
    pub share_change_cancelled: u64,
    pub notice: Option<CallNotice>,
    pub stage: Option<CallTile>,
    pub stage_chosen: bool,
    pub view_chosen: bool,
    pub tiles: Arc<[CallTile]>,
    pub compact_self: Option<CallTile>,
    pub self_view: bool,
    pub follows_speaker: bool,
    pub faces_ready: u64,
    pub shares: Arc<[NamedUser]>,
    pub offered_share: Option<NamedUser>,
    pub started_share: Option<ShareNews>,
    pub ended_share: Option<ShareNews>,
    pub bridges: Arc<[NamedUser]>,
    pub people: Arc<[CallPerson]>,
    pub devices: CallDevices,
    pub connection: Option<Arc<[ServerDetails]>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDetails {
    pub host: String,
    pub publishing: bool,
    pub state: LinkView,
    pub sent_kbps: Option<u32>,
    pub received_kbps: Option<u32>,
    pub loss_permille: Option<u16>,
    pub round_trip_ms: Option<u32>,
    pub sent_video: Vec<(VideoKind, FrameSize)>,
    pub received_video: Vec<(NamedUser, VideoKind, FrameSize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkView {
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallView {
    pub ring: Option<RingView>,
    pub here: Option<RoomCallSummary>,
    pub offer: Option<OfferView>,
    pub sheet: Option<CallSheetView>,
    pub active: Option<ActiveCall>,
    pub failed: Option<FailedCall>,
}
