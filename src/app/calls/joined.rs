use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::Span;

mod hands;
mod reactions;

use self::hands::HandRequest;
use self::reactions::ReactionRequest;
use super::device_test::{PERCENT, level_percent};
use super::devices::Devices;
use super::media::MediaCommand;
use super::people;
use super::stage::Stage;
use crate::commands::view::{
    ActiveCall, CallFailure, CallNotice, CallPhase, CallTile, CameraView, CantDecrypt, HandNews,
    LinkView, MicrophoneView, MissingMedia, ReactionNews, ScreenView, ServerDetails, TileKey,
    TileKind, TileState,
};
use crate::domain::call::devices::{
    DeviceChoice, DeviceChoices, DeviceKey, DeviceKind, DeviceLists,
};
use crate::domain::call::hands::Hands;
use crate::domain::call::identity::{IdentityTable, MediaIdentity, member_spellings, spellings};
use crate::domain::call::keys::{
    KeyAction, KeyExchange, KeyHolder, MediaKey, OWN_KEY_LEN, ReceivedKey,
};
use crate::domain::call::media::{
    ConnectionQuality, Decryption, MediaChoice, MediaEvent, MediaSource, Microphone, SfuState,
};
use crate::domain::call::reactions::{ReactionOption, Reactions};
use crate::domain::call::ring::{RingId, RingKind};
use crate::domain::call::sfu::{GrantRequest, GrantResult, SfuId, SfuRole, SfuTarget, plan};
use crate::domain::call::stats::{ConnectionReading, Rates, ReceivedVideo, SentVideo, rates};
use crate::domain::call::video::{
    Camera, FeedId, FrameSize, ScreenRefusal, ScreenShare, VideoKind,
};
use crate::domain::call::{
    CallIntent, CallMember, DeviceStanding, FocusSelection, MemberKey, OutgoingKey, SentMembership,
    Transport, bridges_in_order,
};
use crate::domain::member_name::NamedUser;
use crate::domain::room::{RoomId, UnreadFlags};
use crate::util::unix_now_ms;

const SHARES_MISSED_BEFORE_HINT: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LinkState {
    Asking,
    Connecting,
    Connected,
    Reconnecting,
    Failed(String),
}

struct Link {
    role: SfuRole,
    state: LinkState,
}

impl Link {
    fn gave_up(&self) -> bool {
        self.role == SfuRole::Subscribe && matches!(self.state, LinkState::Failed(_))
    }
}

#[derive(Default)]
struct Heard {
    present: HashMap<SfuId, HashSet<MediaIdentity>>,
    decryption: HashMap<MediaIdentity, Decryption>,
    muted: HashMap<SfuId, HashSet<MediaIdentity>>,
    speaking: HashMap<SfuId, Vec<MediaIdentity>>,
    video: HashMap<SfuId, HashMap<MediaIdentity, FeedId>>,
    screens: HashMap<SfuId, HashMap<MediaIdentity, FeedId>>,
    missing: HashMap<SfuId, HashMap<MediaIdentity, HashSet<MediaSource>>>,
    quality: HashMap<SfuId, HashMap<MediaIdentity, ConnectionQuality>>,
    stalled: HashMap<SfuId, HashSet<MediaIdentity>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnMicrophone {
    Off,
    Starting,
    On,
    Failed,
}

impl OwnMicrophone {
    const fn sends(self) -> bool {
        matches!(self, Self::Starting | Self::On)
    }

    const fn switched(self, on: bool, published: bool) -> Option<Self> {
        match (self, on) {
            (Self::Starting | Self::On, true) | (Self::Off, false) => None,
            (_, true) if published => Some(Self::On),
            (_, true) => Some(Self::Starting),
            (_, false) => Some(Self::Off),
        }
    }

    const fn reported(self, report: &Microphone) -> Self {
        match (self, report) {
            (Self::Starting, Microphone::On) => Self::On,
            (Self::Starting | Self::On, Microphone::Failed(_)) => Self::Failed,
            _ => self,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OwnCamera {
    Off,
    Starting,
    On(FeedId),
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    First,
    Replacement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OwnScreen {
    Off,
    Choosing(Pick),
    On(FeedId),
    Failed,
    Unavailable(ScreenRefusal),
}

impl Heard {
    fn is_present(&self, identity: &MediaIdentity) -> bool {
        self.present
            .values()
            .any(|present| present.contains(identity))
    }

    fn is_speaking(&self, identity: &MediaIdentity) -> bool {
        self.speaking
            .values()
            .any(|speaking| speaking.contains(identity))
    }

    fn is_muted_on(&self, sfu: &SfuId, identity: &MediaIdentity) -> bool {
        self.muted
            .get(sfu)
            .is_some_and(|muted| muted.contains(identity))
    }

    fn video_on(
        &self,
        sfu: &SfuId,
        identities: &[MediaIdentity],
        kind: VideoKind,
    ) -> Option<FeedId> {
        let feeds = match kind {
            VideoKind::Camera => &self.video,
            VideoKind::Screen => &self.screens,
        }
        .get(sfu)?;
        identities
            .iter()
            .find_map(|identity| feeds.get(identity))
            .cloned()
    }

    fn video_heard(
        &mut self,
        sfu: SfuId,
        identity: MediaIdentity,
        kind: VideoKind,
        feed: Option<FeedId>,
    ) {
        if kind == VideoKind::Camera
            && feed.is_none()
            && let Some(stalled) = self.stalled.get_mut(&sfu)
        {
            stalled.remove(&identity);
        }
        let feeds = match kind {
            VideoKind::Camera => &mut self.video,
            VideoKind::Screen => &mut self.screens,
        }
        .entry(sfu)
        .or_default();
        match feed {
            Some(feed) => feeds.insert(identity, feed),
            None => feeds.remove(&identity),
        };
    }

    fn source_heard(
        &mut self,
        sfu: SfuId,
        identity: MediaIdentity,
        source: MediaSource,
        missing: bool,
    ) {
        let sources = self
            .missing
            .entry(sfu)
            .or_default()
            .entry(identity)
            .or_default();
        if missing {
            sources.insert(source);
        } else {
            sources.remove(&source);
        }
    }

    fn missing_on(&self, sfu: &SfuId, identities: &[MediaIdentity]) -> HashSet<MediaSource> {
        self.missing
            .get(sfu)
            .into_iter()
            .flat_map(|missing| {
                identities
                    .iter()
                    .filter_map(|identity| missing.get(identity))
            })
            .flatten()
            .copied()
            .collect()
    }

    fn rated(&mut self, sfu: SfuId, identity: MediaIdentity, quality: ConnectionQuality) {
        let ratings = self.quality.entry(sfu).or_default();
        if quality == ConnectionQuality::Good {
            ratings.remove(&identity);
        } else {
            ratings.insert(identity, quality);
        }
    }

    fn quality_on(&self, sfu: &SfuId, identities: &[MediaIdentity]) -> ConnectionQuality {
        let ratings = self.quality.get(sfu);
        let rated_as = |quality| {
            identities
                .iter()
                .any(|identity| ratings.and_then(|ratings| ratings.get(identity)) == Some(&quality))
        };
        if rated_as(ConnectionQuality::Lost) {
            ConnectionQuality::Lost
        } else if rated_as(ConnectionQuality::Poor) {
            ConnectionQuality::Poor
        } else {
            ConnectionQuality::Good
        }
    }

    fn stalled_heard(&mut self, sfu: SfuId, identity: MediaIdentity, stalled: bool) {
        let identities = self.stalled.entry(sfu).or_default();
        if stalled {
            identities.insert(identity);
        } else {
            identities.remove(&identity);
        }
    }

    fn is_stalled_on(&self, sfu: &SfuId, identities: &[MediaIdentity]) -> bool {
        self.stalled
            .get(sfu)
            .is_some_and(|stalled| identities.iter().any(|identity| stalled.contains(identity)))
    }

    fn forget(&mut self, sfu: &SfuId) {
        self.stalled.remove(sfu);
        self.present.remove(sfu);
        self.muted.remove(sfu);
        self.speaking.remove(sfu);
        self.video.remove(sfu);
        self.screens.remove(sfu);
        self.missing.remove(sfu);
        self.quality.remove(sfu);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Announcement {
    Sending,
    Sent(RingId),
    Failed,
}

struct Outgoing {
    kind: RingKind,
    announcement: Announcement,
    answered: bool,
}

pub(super) enum Effect {
    SendKey {
        to: Vec<MemberKey>,
        key: OutgoingKey,
    },
    WakeKeys(Duration),
    Grant(Box<GrantRequest>),
    Announce {
        membership_event_id: String,
        kind: RingKind,
    },
    Standings(Vec<MemberKey>),
    RaiseHand {
        membership_event: String,
    },
    LowerHand {
        reaction: String,
    },
    FetchHands(Vec<String>),
    SendReaction {
        membership_event: String,
        reaction: ReactionOption,
    },
}

pub(super) struct Joining {
    pub(super) call: u64,
    pub(super) span: Span,
    pub(super) room_id: RoomId,
    pub(super) room_name: String,
    pub(super) intent: CallIntent,
    pub(super) encrypted: bool,
    pub(super) me: MemberKey,
    pub(super) member_id: String,
    pub(super) transport: Transport,
    pub(super) leave_tx: oneshot::Sender<()>,
    pub(super) intent_tx: watch::Sender<CallIntent>,
    pub(super) media_tx: mpsc::UnboundedSender<MediaCommand>,
    pub(super) media: MediaChoice,
    pub(super) devices: DeviceChoices,
    pub(super) outgoing: Option<RingKind>,
}

pub(super) struct Joined {
    pub(super) call: u64,
    pub(super) span: Span,
    pub(super) room_id: RoomId,
    room_name: String,
    intent: CallIntent,
    encrypted: bool,
    started: Instant,
    live_since_ms: Option<u64>,
    me: MemberKey,
    member_id: String,
    transport: Transport,
    membership: Option<SentMembership>,
    leave_tx: Option<oneshot::Sender<()>>,
    intent_tx: watch::Sender<CallIntent>,
    media_tx: mpsc::UnboundedSender<MediaCommand>,
    keys: KeyExchange,
    identities: IdentityTable,
    members: Vec<CallMember>,
    links: HashMap<SfuId, Link>,
    heard: Heard,
    own_quality: ConnectionQuality,
    standings: HashMap<MemberKey, DeviceStanding>,
    microphone: OwnMicrophone,
    microphone_published: bool,
    camera: OwnCamera,
    screen: OwnScreen,
    shares_missed: u8,
    changes_cancelled: u64,
    outgoing_kind: Option<RingKind>,
    outgoing: Option<Outgoing>,
    stage: Stage,
    devices: Devices,
    unheard: BTreeSet<String>,
    unheard_sent: Vec<MediaIdentity>,
    microphone_meter: Meter,
    readings: Option<Readings>,
    hands: Hands,
    hand_request: HandRequest,
    hand_news: Option<HandNews>,
    reactions: Reactions,
    reaction_request: ReactionRequest,
    reaction_news: Option<ReactionNews>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meter {
    Off,
    Waiting,
    Level(u8),
}

type Readings = HashMap<SfuId, (Instant, ConnectionReading, Rates)>;

fn sent_videos(videos: &[SentVideo]) -> Vec<(VideoKind, FrameSize)> {
    let mut videos: Vec<(VideoKind, FrameSize)> = videos
        .iter()
        .map(|video| (video.kind, video.size))
        .collect();
    videos.sort_by_key(|(kind, _)| *kind == VideoKind::Screen);
    videos
}

pub(super) fn fresh_key() -> MediaKey {
    let mut bytes = [0_u8; OWN_KEY_LEN];
    rand::fill(&mut bytes);
    MediaKey::new(bytes)
}

impl Joined {
    pub(super) fn start(joining: Joining, members: &[CallMember]) -> (Self, Vec<Effect>) {
        let media = joining.media;
        let mut joined = Self {
            call: joining.call,
            span: joining.span,
            room_id: joining.room_id,
            room_name: joining.room_name,
            intent: joining.intent,
            encrypted: joining.encrypted,
            started: Instant::now(),
            live_since_ms: None,
            me: joining.me,
            member_id: joining.member_id,
            transport: joining.transport,
            membership: None,
            leave_tx: Some(joining.leave_tx),
            intent_tx: joining.intent_tx,
            media_tx: joining.media_tx,
            keys: KeyExchange::default(),
            identities: IdentityTable::default(),
            members: Vec::new(),
            links: HashMap::new(),
            heard: Heard::default(),
            own_quality: ConnectionQuality::Good,
            standings: HashMap::new(),
            microphone: if media.microphone {
                OwnMicrophone::Starting
            } else {
                OwnMicrophone::Off
            },
            microphone_published: false,
            camera: OwnCamera::Off,
            screen: OwnScreen::Off,
            shares_missed: 0,
            changes_cancelled: 0,
            outgoing_kind: joining.outgoing,
            outgoing: None,
            stage: Stage::default(),
            devices: Devices::new(joining.devices),
            unheard: BTreeSet::new(),
            unheard_sent: Vec::new(),
            microphone_meter: Meter::Off,
            readings: None,
            hands: Hands::default(),
            hand_request: HandRequest::Idle,
            hand_news: None,
            reactions: Reactions::default(),
            reaction_request: ReactionRequest::Idle,
            reaction_news: None,
        };
        joined.command(MediaCommand::Microphone(media.microphone));
        if media.camera {
            joined.set_camera(true);
        }
        let mut effects = Vec::new();
        if joined.encrypted {
            let actions = joined.keys.members_changed(joined.now(), &[], fresh_key());
            effects.extend(joined.apply_keys(actions));
        }
        effects.extend(joined.members_changed(members.to_vec()));
        effects.extend(joined.replan());
        (joined, effects)
    }

    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    pub(super) const fn intent(&self) -> CallIntent {
        self.intent
    }

    pub(super) fn room_name(&self) -> &str {
        &self.room_name
    }

    pub(super) fn media_sent(&self) -> MediaChoice {
        MediaChoice {
            microphone: self.microphone.sends(),
            camera: matches!(self.camera, OwnCamera::Starting | OwnCamera::On(_)),
        }
    }

    fn has_company(&self) -> bool {
        self.members
            .iter()
            .any(|member| member.user.user_id != self.me.user)
    }

    pub(super) fn wake_at(&self, offset: Duration) -> Instant {
        self.started + offset
    }

    fn command(&self, command: MediaCommand) {
        if self.media_tx.send(command).is_err() {
            tracing::debug!(call = self.call, "the call media is already closed");
        }
    }

    pub(super) fn order_farewell(&mut self) {
        if let Some(leave_tx) = self.leave_tx.take()
            && leave_tx.send(()).is_err()
        {
            tracing::debug!(call = self.call, "the call membership lane already ended");
        }
    }

    fn holders(&self) -> Vec<KeyHolder> {
        self.members
            .iter()
            .filter(|member| MemberKey::of(member) != self.me)
            .map(KeyHolder::of)
            .collect()
    }

    fn own_spellings(&self) -> [MediaIdentity; 3] {
        spellings(&self.me.user, &self.me.device, &self.member_id)
    }

    pub(super) fn members_changed(&mut self, members: Vec<CallMember>) -> Vec<Effect> {
        if members == self.members {
            return Vec::new();
        }
        self.identities = IdentityTable::build(&members);
        self.members = members;
        self.tell_unheard();
        let mut effects = self.hands_follow_members();
        effects.extend(self.replan());
        if self.membership.is_some() && self.encrypted {
            let actions = self
                .keys
                .members_changed(self.now(), &self.holders(), fresh_key());
            effects.extend(self.apply_keys(actions));
        }
        effects.extend(self.new_standings());
        self.notice_pickup();
        effects
    }

    fn replan(&mut self) -> Vec<Effect> {
        let targets = plan(&self.transport, &self.me, &self.members);
        let wanted: HashSet<&SfuId> = targets.iter().map(|target| &target.id).collect();
        let dropped: Vec<SfuId> = self
            .links
            .keys()
            .filter(|id| !wanted.contains(id))
            .cloned()
            .collect();
        for id in dropped {
            tracing::info!(call = self.call, sfu = %id, "no member of the call uses this media server any more");
            self.links.remove(&id);
            self.heard.forget(&id);
            self.command(MediaCommand::Disconnect(id));
        }
        let mut effects = Vec::new();
        for target in targets {
            match self.links.get(&target.id) {
                Some(link) if link.gave_up() => {
                    tracing::info!(call = self.call, sfu = %target.id, "trying a failed media server again");
                    self.command(MediaCommand::Disconnect(target.id.clone()));
                }
                Some(_) => continue,
                None => {}
            }
            effects.push(self.ask_grant(target));
        }
        effects
    }

    fn ask_grant(&mut self, target: SfuTarget) -> Effect {
        tracing::info!(call = self.call, sfu = %target.id, role = ?target.role, server = %target.server_name, "asking for a call media grant");
        self.links.insert(
            target.id.clone(),
            Link {
                role: target.role,
                state: LinkState::Asking,
            },
        );
        Effect::Grant(Box::new(GrantRequest {
            target,
            device: self.me.device.clone(),
            member_id: self.member_id.clone(),
        }))
    }

    fn new_standings(&mut self) -> Option<Effect> {
        if !self.encrypted {
            return None;
        }
        let unknown: Vec<MemberKey> = self
            .members
            .iter()
            .map(MemberKey::of)
            .filter(|member| *member != self.me && !self.standings.contains_key(member))
            .collect();
        for member in &unknown {
            self.standings
                .insert(member.clone(), DeviceStanding::Unknown);
        }
        (!unknown.is_empty()).then_some(Effect::Standings(unknown))
    }

    fn notice_pickup(&mut self) {
        let someone_else = self.has_company();
        if let Some(outgoing) = self.outgoing.as_mut().filter(|outgoing| !outgoing.answered)
            && someone_else
        {
            tracing::info!(call = self.call, "someone picked up the call");
            outgoing.answered = true;
        }
    }

    pub(super) fn standings_known(&mut self, standings: Vec<(MemberKey, DeviceStanding)>) {
        self.standings.extend(standings);
    }

    pub(super) fn lane_joined(&mut self, sent: SentMembership) -> Vec<Effect> {
        let first = self.membership.is_none();
        let event_id = sent.event_id.clone();
        self.membership = Some(sent);
        if !first {
            return Vec::new();
        }
        let mut effects = Vec::new();
        if self.encrypted {
            let actions = self
                .keys
                .members_changed(self.now(), &self.holders(), fresh_key());
            effects.extend(self.apply_keys(actions));
        }
        let alone = !self.has_company();
        if let Some(kind) = self.outgoing_kind.take().filter(|_| alone) {
            self.outgoing = Some(Outgoing {
                kind,
                announcement: Announcement::Sending,
                answered: false,
            });
            effects.push(Effect::Announce {
                membership_event_id: event_id,
                kind,
            });
        }
        effects
    }

    pub(super) fn announced(&mut self, ring: Option<RingId>) -> bool {
        let Some(outgoing) = self.outgoing.as_mut() else {
            return false;
        };
        let rings = outgoing.kind == RingKind::Ring && ring.is_some();
        outgoing.announcement = ring.map_or(Announcement::Failed, Announcement::Sent);
        rings
    }

    pub(super) fn announce_again(&mut self) -> Option<Effect> {
        let membership_event_id = self.membership.as_ref()?.event_id.clone();
        let outgoing = self.outgoing.as_mut().filter(|outgoing| {
            outgoing.announcement == Announcement::Failed && !outgoing.answered
        })?;
        tracing::info!(call = self.call, "announcing the call again");
        outgoing.announcement = Announcement::Sending;
        Some(Effect::Announce {
            membership_event_id,
            kind: outgoing.kind,
        })
    }

    pub(super) fn announced_as(&self, ring: &RingId) -> bool {
        self.outgoing.as_ref().is_some_and(
            |outgoing| matches!(&outgoing.announcement, Announcement::Sent(sent) if sent == ring),
        )
    }

    pub(super) fn granted(&mut self, sfu: &SfuId, outcome: GrantResult) -> Result<(), CallFailure> {
        let Some(link) = self.links.get_mut(sfu) else {
            return Ok(());
        };
        match outcome {
            Ok(grant) => {
                link.state = LinkState::Connecting;
                self.command(MediaCommand::Connect(Box::new(grant)));
                Ok(())
            }
            Err(failure) => {
                tracing::warn!(call = self.call, %sfu, %failure, "no grant for a call media server");
                let publish = link.role == SfuRole::Publish;
                link.state = LinkState::Failed(failure.to_string());
                if publish {
                    Err(CallFailure::ServerUnreachable)
                } else {
                    Ok(())
                }
            }
        }
    }

    pub(super) fn media_event(&mut self, event: MediaEvent) -> Result<(), CallFailure> {
        match event {
            MediaEvent::Sfu { sfu, state } => return self.sfu_state(&sfu, state),
            MediaEvent::Present {
                sfu,
                identity,
                present,
            } => {
                let identities = self.heard.present.entry(sfu).or_default();
                if present {
                    identities.insert(identity);
                } else {
                    identities.remove(&identity);
                }
            }
            MediaEvent::Muted {
                sfu,
                identity,
                muted,
            } => {
                let identities = self.heard.muted.entry(sfu).or_default();
                if muted {
                    identities.insert(identity);
                } else {
                    identities.remove(&identity);
                }
            }
            MediaEvent::Speaking { sfu, speakers } => {
                self.heard.speaking.insert(sfu, speakers);
            }
            MediaEvent::Missing {
                sfu,
                identity,
                source,
                missing,
            } => self.missing_heard(sfu, identity, source, missing),
            MediaEvent::Quality {
                sfu,
                identity,
                quality,
            } => self.quality_heard(sfu, identity, quality),
            MediaEvent::OwnQuality { sfu, quality } => self.own_quality_heard(&sfu, quality),
            MediaEvent::Stalled {
                sfu,
                identity,
                stalled,
            } => {
                if self.links.contains_key(&sfu) {
                    self.heard.stalled_heard(sfu, identity, stalled);
                }
            }
            MediaEvent::Decryption { identity, state } => {
                self.heard.decryption.insert(identity, state);
            }
            MediaEvent::NotEncrypted { sfu, trouble } => {
                tracing::warn!(call = self.call, %sfu, ?trouble, "your call media could not be encrypted, so the call stops");
                return Err(CallFailure::NotEncrypted);
            }
            MediaEvent::Microphone(microphone) => self.microphone_reported(&microphone),
            MediaEvent::Connections(readings) => self.connections_read(readings),
            MediaEvent::MicrophoneLevel(percent) => {
                if self.microphone_meter != Meter::Off {
                    self.microphone_meter =
                        Meter::Level(level_percent(f32::from(percent) / PERCENT));
                }
            }
            MediaEvent::Camera(camera) => self.camera_reported(camera),
            MediaEvent::Screen(screen) => self.screen_reported(screen),
            MediaEvent::Video {
                sfu,
                identity,
                kind,
                feed,
            } => self.heard.video_heard(sfu, identity, kind, feed),
            MediaEvent::Devices(lists) => self.devices.listed(lists),
            MediaEvent::DeviceFailed { kind, device, why } => {
                tracing::warn!(call = self.call, %kind, %device, why, "a call device could not be used");
                self.devices.failed(kind, device);
            }
        }
        Ok(())
    }

    fn missing_heard(
        &mut self,
        sfu: SfuId,
        identity: MediaIdentity,
        source: MediaSource,
        missing: bool,
    ) {
        if self.links.contains_key(&sfu) {
            self.heard.source_heard(sfu, identity, source, missing);
        }
    }

    fn quality_heard(&mut self, sfu: SfuId, identity: MediaIdentity, quality: ConnectionQuality) {
        if self.links.contains_key(&sfu) {
            self.heard.rated(sfu, identity, quality);
        }
    }

    fn own_quality_heard(&mut self, sfu: &SfuId, quality: ConnectionQuality) {
        if self
            .links
            .get(sfu)
            .is_some_and(|link| link.role == SfuRole::Publish)
        {
            self.own_quality = quality;
        }
    }

    fn microphone_reported(&mut self, report: &Microphone) {
        if *report == Microphone::On {
            self.microphone_published = true;
        }
        if let Microphone::Failed(why) = report {
            if self.microphone.sends() {
                tracing::warn!(call = self.call, why, "the microphone failed");
            } else {
                tracing::debug!(
                    call = self.call,
                    why,
                    "the microphone you switched off failed"
                );
            }
        }
        self.microphone = self.microphone.reported(report);
    }

    fn camera_reported(&mut self, camera: Camera) {
        match camera {
            Camera::On(feed) if self.camera == OwnCamera::Starting => {
                self.camera = OwnCamera::On(feed);
            }
            Camera::On(_) => {}
            Camera::Failed(why) if self.camera == OwnCamera::Off => {
                tracing::debug!(call = self.call, why, "the camera you switched off failed");
            }
            Camera::Failed(why) => {
                tracing::warn!(call = self.call, why, "the camera did not start");
                self.camera = OwnCamera::Failed;
                self.command(MediaCommand::Camera(false));
                self.tell_intent();
            }
        }
    }

    fn screen_reported(&mut self, screen: ScreenShare) {
        self.screen = match screen {
            ScreenShare::On(_) if !matches!(self.screen, OwnScreen::Choosing(_)) => return,
            ScreenShare::On(feed) => {
                self.shares_missed = 0;
                OwnScreen::On(feed)
            }
            ScreenShare::Off if matches!(self.screen, OwnScreen::Unavailable(_)) => return,
            ScreenShare::Off => self.screen_ended(),
            ScreenShare::Failed(why) => {
                tracing::warn!(call = self.call, why, "the screen share did not start");
                self.shares_missed = self.shares_missed.saturating_add(1);
                OwnScreen::Failed
            }
            ScreenShare::Unavailable(refusal) => {
                tracing::info!(
                    call = self.call,
                    ?refusal,
                    "this desktop cannot share its screen"
                );
                OwnScreen::Unavailable(refusal)
            }
        };
    }

    fn screen_ended(&mut self) -> OwnScreen {
        match self.screen {
            OwnScreen::Choosing(Pick::Replacement) => {
                tracing::info!(
                    call = self.call,
                    "no other screen was chosen, so your screen share stopped"
                );
                self.changes_cancelled = self.changes_cancelled.saturating_add(1);
            }
            OwnScreen::Choosing(Pick::First) => {
                self.shares_missed = self.shares_missed.saturating_add(1);
            }
            OwnScreen::Off | OwnScreen::On(_) | OwnScreen::Failed | OwnScreen::Unavailable(_) => {}
        }
        OwnScreen::Off
    }

    fn sfu_state(&mut self, sfu: &SfuId, state: SfuState) -> Result<(), CallFailure> {
        let Some(link) = self.links.get_mut(sfu) else {
            return Ok(());
        };
        let publish = link.role == SfuRole::Publish;
        let was_up = matches!(link.state, LinkState::Connected | LinkState::Reconnecting);
        link.state = match state {
            SfuState::Connecting => LinkState::Connecting,
            SfuState::Connected => LinkState::Connected,
            SfuState::Reconnecting => LinkState::Reconnecting,
            SfuState::Disconnected(why) => {
                tracing::info!(call = self.call, %sfu, why, was_up, "a call media server connection ended");
                match (publish, was_up) {
                    (true, true) => return Err(CallFailure::CutOff),
                    (true, false) => return Err(CallFailure::ServerUnreachable),
                    (false, _) => {
                        self.heard.forget(sfu);
                        LinkState::Failed(why)
                    }
                }
            }
        };
        if publish && link.state == LinkState::Connected && self.live_since_ms.is_none() {
            self.live_since_ms = Some(unix_now_ms());
        }
        Ok(())
    }

    pub(super) fn received_key(&mut self, key: ReceivedKey) -> Vec<Effect> {
        if !self.encrypted {
            return Vec::new();
        }
        let actions = self.keys.received(key, &self.holders());
        self.apply_keys(actions)
    }

    pub(super) fn key_wake(&mut self) -> Vec<Effect> {
        let actions = self.keys.tick(self.now());
        self.apply_keys(actions)
    }

    pub(super) fn set_microphone(&mut self, on: bool) {
        let Some(switched) = self.microphone.switched(on, self.microphone_published) else {
            return;
        };
        tracing::info!(call = self.call, on, "switching the microphone");
        self.microphone = switched;
        self.command(MediaCommand::Microphone(on));
    }

    pub(super) fn set_camera(&mut self, on: bool) {
        let wanted = if on {
            OwnCamera::Starting
        } else {
            OwnCamera::Off
        };
        if matches!(
            (&self.camera, &wanted),
            (OwnCamera::Off, OwnCamera::Off)
                | (OwnCamera::Starting | OwnCamera::On(_), OwnCamera::Starting)
        ) {
            return;
        }
        tracing::info!(call = self.call, on, "switching the camera");
        if on {
            self.devices.retry(DeviceKind::Camera);
        }
        self.camera = wanted;
        self.command(MediaCommand::Camera(on));
        self.tell_intent();
    }

    fn tell_intent(&self) {
        let intent = CallIntent::of_camera(self.media_sent().camera);
        self.intent_tx.send_if_modified(|told| {
            if *told == intent {
                return false;
            }
            tracing::info!(
                call = self.call,
                ?intent,
                "the call membership's intent follows the camera"
            );
            *told = intent;
            true
        });
    }

    pub(super) fn use_device(
        &mut self,
        kind: DeviceKind,
        key: Option<&DeviceKey>,
    ) -> Option<DeviceChoice> {
        let Some(choice) = self.devices.choice_for(kind, key) else {
            tracing::debug!(call = self.call, %kind, "no such device to use in the call");
            return None;
        };
        tracing::info!(call = self.call, %kind, device = ?choice.key(), "choosing a call device");
        self.devices.choose(kind, choice.clone());
        self.command(MediaCommand::Device(kind, choice.clone()));
        Some(choice)
    }

    pub(super) fn refresh_devices(&self) {
        self.command(MediaCommand::RefreshDevices);
    }

    pub(super) fn device_lists(&self) -> DeviceLists {
        self.devices.lists().clone()
    }

    pub(super) fn camera_showing(&self, camera: &DeviceChoice) -> Option<FeedId> {
        let OwnCamera::On(feed) = &self.camera else {
            return None;
        };
        self.devices
            .uses_alike(DeviceKind::Camera, camera)
            .then(|| feed.clone())
    }

    pub(super) fn set_screen_share(&mut self, on: bool) {
        let unchanged = match self.screen {
            OwnScreen::Unavailable(_) => true,
            OwnScreen::Off | OwnScreen::Failed => !on,
            OwnScreen::Choosing(_) | OwnScreen::On(_) => on,
        };
        if unchanged {
            return;
        }
        tracing::info!(call = self.call, on, "switching the screen share");
        self.screen = if on {
            OwnScreen::Choosing(Pick::First)
        } else {
            OwnScreen::Off
        };
        self.command(MediaCommand::ScreenShare(on));
    }

    pub(super) fn change_screen_share(&mut self) {
        if !matches!(self.screen, OwnScreen::On(_)) {
            tracing::debug!(call = self.call, "no screen share to change");
            return;
        }
        tracing::info!(
            call = self.call,
            "changing the shared screen: stopping the share, then asking again"
        );
        self.screen = OwnScreen::Choosing(Pick::Replacement);
        self.command(MediaCommand::ScreenShare(false));
        self.command(MediaCommand::ScreenShare(true));
    }

    fn apply_keys(&self, actions: Vec<KeyAction>) -> Vec<Effect> {
        let mut effects = Vec::new();
        for action in actions {
            match action {
                KeyAction::Send { to, index, key } => effects.push(Effect::SendKey {
                    to,
                    key: OutgoingKey {
                        member_id: self.member_id.clone(),
                        index,
                        key,
                        sent_ms: unix_now_ms(),
                    },
                }),
                KeyAction::InstallOwn { index, key } => {
                    self.command(MediaCommand::OwnKey(index, key));
                }
                KeyAction::InstallRemote { member, index, key } => {
                    let identities = self.identities.identities_of(&member);
                    tracing::debug!(call = self.call, %member, %index, identities = identities.len(), "installing a call key");
                    self.command(MediaCommand::Key(identities, index, key));
                }
                KeyAction::UseOwnIndex(index) => self.command(MediaCommand::UseOwnKey(index)),
                KeyAction::WakeAt(at) => effects.push(Effect::WakeKeys(at)),
            }
        }
        effects
    }

    pub(super) fn is_ringing_out(&self) -> bool {
        self.outgoing.as_ref().is_some_and(|outgoing| {
            outgoing.kind == RingKind::Ring
                && !outgoing.answered
                && outgoing.announcement != Announcement::Failed
        })
    }

    fn unannounced(&self) -> Option<RingKind> {
        self.outgoing
            .as_ref()
            .filter(|outgoing| outgoing.announcement == Announcement::Failed && !outgoing.answered)
            .map(|outgoing| outgoing.kind)
    }

    fn publish_link(&self) -> Option<&Link> {
        self.links
            .values()
            .find(|link| link.role == SfuRole::Publish)
    }

    fn phase(&self) -> CallPhase {
        match self.publish_link().map(|link| &link.state) {
            Some(LinkState::Reconnecting) => CallPhase::Reconnecting,
            Some(LinkState::Connected) if self.is_ringing_out() => CallPhase::Calling,
            Some(LinkState::Connected) if !self.has_company() => CallPhase::Waiting,
            Some(LinkState::Connected) => CallPhase::Live,
            _ if self.membership.is_some() => CallPhase::Connecting,
            _ => CallPhase::Joining,
        }
    }

    fn notice(&self, tiles: &[CallTile]) -> Option<CallNotice> {
        if let Some(kind) = self.unannounced() {
            return Some(CallNotice::NotAnnounced(kind));
        }
        if self.microphone == OwnMicrophone::Failed {
            return Some(CallNotice::MicrophoneFailed);
        }
        if self.camera == OwnCamera::Failed {
            return Some(CallNotice::CameraFailed);
        }
        self.screen_notice()
            .or_else(|| self.connection_notice())
            .or_else(|| undecryptable(tiles))
            .or_else(|| not_received(tiles))
    }

    fn connection_notice(&self) -> Option<CallNotice> {
        let connected = self
            .publish_link()
            .is_some_and(|link| link.state == LinkState::Connected);
        (connected && self.own_quality != ConnectionQuality::Good)
            .then_some(CallNotice::ConnectionPoor)
    }

    fn screen_notice(&self) -> Option<CallNotice> {
        let missed_often = self.shares_missed >= SHARES_MISSED_BEFORE_HINT;
        match self.screen {
            OwnScreen::Choosing(_) => Some(CallNotice::ChoosingScreen),
            OwnScreen::Off | OwnScreen::Failed if missed_often => {
                Some(CallNotice::SharingNeverStarted)
            }
            OwnScreen::Failed => Some(CallNotice::ScreenFailed),
            OwnScreen::Off | OwnScreen::On(_) | OwnScreen::Unavailable(_) => None,
        }
    }

    pub(super) fn settle_stage(&mut self) -> Option<Duration> {
        let tiles = self.tiles();
        let loudest = self.loudest_speaker();
        let now = self.now();
        self.stage.settle(&tiles, loudest.as_ref(), now)
    }

    fn loudest_speaker(&self) -> Option<TileKey> {
        let mut servers: Vec<&SfuId> = self.heard.speaking.keys().collect();
        servers.sort_unstable();
        servers
            .into_iter()
            .filter_map(|sfu| self.heard.speaking.get(sfu))
            .flatten()
            .find_map(|identity| self.person_speaking(identity))
    }

    fn person_speaking(&self, identity: &MediaIdentity) -> Option<TileKey> {
        self.members
            .iter()
            .filter(|member| MemberKey::of(member) != self.me)
            .find(|member| member_spellings(member).contains(identity))
            .map(|member| TileKey {
                member: MemberKey::of(member),
                kind: TileKind::Person,
            })
    }

    pub(super) fn focus_tile(&mut self, key: &str) {
        let tiles = self.tiles();
        if !self.stage.focus(key, &tiles) {
            tracing::debug!(tile = key, "no call tile to focus");
        }
    }

    pub(super) fn unpin_tile(&mut self) {
        tracing::info!(
            call = self.call,
            "the call's stage goes back to its own rule"
        );
        self.stage.unpin();
    }

    pub(super) fn show_gallery(&mut self) {
        tracing::info!(call = self.call, "showing the call as a gallery");
        self.stage.gallery();
    }

    pub(super) fn view_share(&mut self) {
        let tiles = self.tiles();
        self.stage.view_share(&tiles);
    }

    pub(super) fn show_self_view(&mut self, on: bool) {
        tracing::info!(call = self.call, on, "switching the self-view");
        self.stage.show_self(on);
    }

    pub(super) fn watch_microphone(&mut self, on: bool) {
        if (self.microphone_meter != Meter::Off) == on {
            return;
        }
        tracing::debug!(call = self.call, on, "watching your microphone's activity");
        self.microphone_meter = if on { Meter::Waiting } else { Meter::Off };
        self.command(MediaCommand::WatchMicrophone(on));
    }

    pub(super) fn watch_connections(&mut self, on: bool) {
        if self.readings.is_some() == on {
            return;
        }
        tracing::info!(
            call = self.call,
            on,
            "watching the call's connection details"
        );
        self.readings = on.then(HashMap::new);
        self.command(MediaCommand::WatchConnections(on));
    }

    fn connections_read(&mut self, readings: Vec<ConnectionReading>) {
        let Some(kept) = self.readings.as_mut() else {
            return;
        };
        let now = Instant::now();
        for reading in readings {
            let rates = kept
                .get(&reading.sfu)
                .map(|(at, previous, _)| {
                    let elapsed = now.saturating_duration_since(*at);
                    rates(
                        previous,
                        &reading,
                        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                    )
                })
                .unwrap_or_default();
            kept.insert(reading.sfu.clone(), (now, reading, rates));
        }
    }

    fn connection_details(&self, readings: &Readings) -> Vec<ServerDetails> {
        let mut links: Vec<(&SfuId, &Link)> = self.links.iter().collect();
        links.sort_by_cached_key(|(sfu, link)| (link.role != SfuRole::Publish, sfu.host()));
        links
            .into_iter()
            .map(|(sfu, link)| {
                let reading = readings.get(sfu);
                let rates = reading.map(|(_, _, rates)| *rates).unwrap_or_default();
                ServerDetails {
                    host: sfu.host(),
                    publishing: link.role == SfuRole::Publish,
                    state: match link.state {
                        LinkState::Asking | LinkState::Connecting => LinkView::Connecting,
                        LinkState::Connected => LinkView::Connected,
                        LinkState::Reconnecting => LinkView::Reconnecting,
                        LinkState::Failed(_) => LinkView::Failed,
                    },
                    sent_kbps: rates.sent_kbps,
                    received_kbps: rates.received_kbps,
                    loss_permille: rates.loss_permille,
                    round_trip_ms: reading.and_then(|(_, reading, _)| reading.round_trip_ms),
                    sent_video: reading
                        .map(|(_, reading, _)| sent_videos(&reading.sent_video))
                        .unwrap_or_default(),
                    received_video: reading
                        .map(|(_, reading, _)| self.named_videos(&reading.received_video))
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    fn named_videos(&self, videos: &[ReceivedVideo]) -> Vec<(NamedUser, VideoKind, FrameSize)> {
        videos
            .iter()
            .filter_map(|video| {
                let key = self.identities.member_of(&video.identity)?;
                let member = self
                    .members
                    .iter()
                    .find(|member| MemberKey::of(member) == *key)?;
                Some((member.user.clone(), video.kind, video.size))
            })
            .collect()
    }

    pub(super) fn mute_for_me(&mut self, user_id: &str, on: bool) {
        if user_id == self.me.user {
            tracing::debug!(call = self.call, "you cannot mute yourself for you");
            return;
        }
        let changed = if on {
            self.unheard.insert(user_id.to_owned())
        } else {
            self.unheard.remove(user_id)
        };
        if changed {
            tracing::info!(
                call = self.call,
                user_id,
                on,
                "muting a call participant for you"
            );
            self.tell_unheard();
        }
    }

    fn tell_unheard(&mut self) {
        let mut identities: Vec<MediaIdentity> = self
            .members
            .iter()
            .filter(|member| self.unheard.contains(&member.user.user_id))
            .flat_map(|member| self.identities.identities_of(&MemberKey::of(member)))
            .collect();
        identities.sort();
        identities.dedup();
        if identities == self.unheard_sent {
            return;
        }
        tracing::debug!(
            call = self.call,
            people = self.unheard.len(),
            identities = identities.len(),
            "telling the media whom you do not hear"
        );
        self.unheard_sent.clone_from(&identities);
        self.command(MediaCommand::Unheard(identities));
    }

    pub(super) fn follow_speaker(&mut self, on: bool) {
        tracing::info!(call = self.call, on, "switching speaker following");
        self.stage.follow_speaker(on);
    }

    pub(super) fn show_share_preview(&mut self, on: bool) {
        tracing::info!(
            call = self.call,
            on,
            "switching the preview of your shared screen"
        );
        self.stage.show_share_preview(on);
    }

    pub(super) fn view(&self) -> ActiveCall {
        let tiles = self.tiles();
        let notice = self.notice(&tiles);
        let sharing = matches!(self.screen, OwnScreen::On(_));
        let people = people::of_tiles(
            &tiles,
            &self.members,
            sharing,
            self.stage.self_focusable(&tiles),
        );
        let arranged = self.stage.arrange(tiles);
        ActiveCall {
            call: self.call,
            room_id: self.room_id.clone(),
            room_name: self.room_name.clone(),
            room_unread: UnreadFlags::default(),
            phase: self.phase(),
            live_since_ms: self.live_since_ms,
            encrypted: self.encrypted,
            video: self.intent == CallIntent::Video,
            microphone: match self.microphone {
                OwnMicrophone::Off => MicrophoneView::Off,
                OwnMicrophone::Starting => MicrophoneView::Starting,
                OwnMicrophone::On => MicrophoneView::On,
                OwnMicrophone::Failed => MicrophoneView::Failed,
            },
            microphone_level: match self.microphone_meter {
                Meter::Level(level) if self.microphone == OwnMicrophone::On => Some(level),
                Meter::Off | Meter::Waiting | Meter::Level(_) => None,
            },
            camera: match self.camera {
                OwnCamera::Off => CameraView::Off,
                OwnCamera::Starting => CameraView::Starting,
                OwnCamera::On(_) => CameraView::On,
                OwnCamera::Failed => CameraView::Failed,
            },
            screen: match self.screen {
                OwnScreen::Off => ScreenView::Off,
                OwnScreen::Choosing(_) => ScreenView::Choosing,
                OwnScreen::On(_) => ScreenView::On,
                OwnScreen::Failed => ScreenView::Failed,
                OwnScreen::Unavailable(refusal) => ScreenView::Unavailable(refusal),
            },
            hand: self.hand_view(),
            hands_raised: self.hands_raised(),
            hand_news: self.hand_news(),
            reaction: self.reaction_view(),
            reaction_news: self.reaction_news(),
            share_preview: self.stage.share_preview(),
            share_change_cancelled: self.changes_cancelled,
            notice,
            stage: arranged.stage,
            stage_chosen: arranged.chosen,
            view_chosen: arranged.view_chosen,
            tiles: Arc::from(arranged.tiles),
            compact_self: arranged.compact_self,
            self_view: self.stage.self_view(),
            follows_speaker: self.stage.follows_speaker(),
            faces_ready: 0,
            shares: Arc::from(arranged.shares),
            offered_share: arranged.offered_share,
            started_share: self.stage.started_share().cloned(),
            ended_share: self.stage.ended_share().cloned(),
            bridges: Arc::from(if self.encrypted {
                bridges_in_order(&self.members)
            } else {
                Vec::new()
            }),
            people: Arc::from(people),
            devices: self.devices.view(),
            connection: self
                .readings
                .as_ref()
                .map(|readings| Arc::from(self.connection_details(readings))),
        }
    }

    fn tiles(&self) -> Vec<CallTile> {
        let mut tiles: Vec<CallTile> = self
            .members
            .iter()
            .filter(|member| MemberKey::of(member) != self.me)
            .filter_map(|member| self.screen_tile(member))
            .collect();
        if !self
            .members
            .iter()
            .any(|member| MemberKey::of(member) == self.me)
        {
            let me = NamedUser::unnamed(self.me.user.clone());
            tiles.extend(self.own_screen_tile(&me));
            tiles.push(self.own_tile(me, None));
        }
        for member in &self.members {
            if MemberKey::of(member) == self.me {
                tiles.extend(self.own_screen_tile(&member.user));
                tiles.push(self.own_tile(member.user.clone(), member.avatar_mxc.clone()));
            } else {
                tiles.push(self.remote_tile(member));
            }
        }
        number_devices(&mut tiles);
        for tile in tiles.iter_mut().filter(|tile| !tile.own) {
            tile.muted_for_you = self.unheard.contains(&tile.member.user);
        }
        let order = self.hand_order();
        for tile in tiles
            .iter_mut()
            .filter(|tile| tile.kind == TileKind::Person)
        {
            tile.hand = order
                .iter()
                .position(|member| *member == tile.member)
                .map(|index| index + 1);
            tile.reaction = self.reaction_on(&tile.member);
        }
        tiles
    }

    fn own_tile(&self, name: NamedUser, avatar_mxc: Option<String>) -> CallTile {
        let speaking = self
            .own_spellings()
            .iter()
            .any(|identity| self.heard.is_speaking(identity));
        let state = match self.publish_link().map(|link| &link.state) {
            Some(LinkState::Connected) if self.own_quality != ConnectionQuality::Good => {
                TileState::PoorConnection
            }
            Some(LinkState::Connected | LinkState::Reconnecting) => TileState::Live,
            Some(LinkState::Failed(_)) => TileState::Unreachable,
            _ => TileState::Joining,
        };
        let video = match &self.camera {
            OwnCamera::On(feed) => Some(feed.clone()),
            OwnCamera::Off | OwnCamera::Starting | OwnCamera::Failed => None,
        };
        CallTile {
            member: self.me.clone(),
            name,
            device: 0,
            avatar_mxc,
            own: true,
            kind: TileKind::Person,
            state,
            muted: self.microphone != OwnMicrophone::On,
            muted_for_you: false,
            hand: None,
            reaction: None,
            speaking,
            video,
        }
    }

    fn own_screen_tile(&self, name: &NamedUser) -> Option<CallTile> {
        let OwnScreen::On(feed) = &self.screen else {
            return None;
        };
        Some(CallTile {
            member: self.me.clone(),
            name: name.clone(),
            device: 0,
            avatar_mxc: None,
            own: true,
            kind: TileKind::Screen,
            state: TileState::Live,
            muted: false,
            muted_for_you: false,
            hand: None,
            reaction: None,
            speaking: false,
            video: Some(feed.clone()),
        })
    }

    fn remote_tile(&self, member: &CallMember) -> CallTile {
        let identities = member_spellings(member);
        let key = MemberKey::of(member);
        let publishes_to = self.publishes_to(member);
        let muted = publishes_to.as_ref().is_some_and(|sfu| {
            identities
                .iter()
                .any(|identity| self.heard.is_muted_on(sfu, identity))
        });
        let video =
            publishes_to.and_then(|sfu| self.heard.video_on(&sfu, &identities, VideoKind::Camera));
        CallTile {
            state: self.remote_state(member, &key, &identities),
            muted,
            muted_for_you: false,
            hand: None,
            reaction: None,
            speaking: identities
                .iter()
                .any(|identity| self.heard.is_speaking(identity)),
            member: key,
            name: member.user.clone(),
            device: 0,
            avatar_mxc: member.avatar_mxc.clone(),
            own: false,
            kind: TileKind::Person,
            video,
        }
    }

    fn screen_tile(&self, member: &CallMember) -> Option<CallTile> {
        let identities = member_spellings(member);
        let sfu = self.publishes_to(member)?;
        let feed = self.heard.video_on(&sfu, &identities, VideoKind::Screen)?;
        let key = MemberKey::of(member);
        let state = match self.remote_state(member, &key, &identities) {
            TileState::NotReceived(_) | TileState::PoorConnection | TileState::VideoStalled => {
                TileState::Live
            }
            state => state,
        };
        Some(CallTile {
            state,
            muted: false,
            muted_for_you: false,
            hand: None,
            reaction: None,
            speaking: false,
            member: key,
            name: member.user.clone(),
            device: 0,
            avatar_mxc: None,
            own: false,
            kind: TileKind::Screen,
            video: Some(feed),
        })
    }

    fn remote_state(
        &self,
        member: &CallMember,
        key: &MemberKey,
        identities: &[MediaIdentity; 3],
    ) -> TileState {
        let publishes_to = self.publishes_to(member);
        if publishes_to.as_ref().is_some_and(|sfu| {
            self.links
                .get(sfu)
                .is_some_and(|link| matches!(link.state, LinkState::Failed(_)))
        }) {
            return TileState::Unreachable;
        }
        if !identities
            .iter()
            .any(|identity| self.heard.is_present(identity))
        {
            return TileState::Joining;
        }
        let keys = self.key_state(key, identities);
        if matches!(keys, TileState::CantDecrypt(_)) {
            return keys;
        }
        publishes_to
            .and_then(|sfu| self.trouble_on(&sfu, identities))
            .unwrap_or(keys)
    }

    fn key_state(&self, key: &MemberKey, identities: &[MediaIdentity; 3]) -> TileState {
        if !self.encrypted {
            return TileState::Live;
        }
        let decryption = identities
            .iter()
            .find_map(|identity| self.heard.decryption.get(identity));
        match decryption {
            Some(Decryption::Ok) => TileState::Live,
            Some(Decryption::Failed) => TileState::CantDecrypt(self.why_undecryptable(key)),
            None if self.keys.has_key_from(key) => TileState::Live,
            Some(Decryption::MissingKey) | None => TileState::WaitingForKeys,
        }
    }

    fn trouble_on(&self, sfu: &SfuId, identities: &[MediaIdentity]) -> Option<TileState> {
        let quality = self.heard.quality_on(sfu, identities);
        if quality == ConnectionQuality::Lost {
            return Some(TileState::ConnectionLost);
        }
        if let Some(what) = missing(&self.heard.missing_on(sfu, identities)) {
            return Some(TileState::NotReceived(what));
        }
        if self.heard.is_stalled_on(sfu, identities) {
            return Some(TileState::VideoStalled);
        }
        (quality == ConnectionQuality::Poor).then_some(TileState::PoorConnection)
    }

    fn why_undecryptable(&self, key: &MemberKey) -> CantDecrypt {
        if self.standings.get(key) == Some(&DeviceStanding::NotCrossSigned) {
            CantDecrypt::NotCrossSigned
        } else {
            CantDecrypt::NoKey
        }
    }

    fn publishes_to(&self, member: &CallMember) -> Option<SfuId> {
        let transport = match member.selection {
            FocusSelection::MultiSfu => member.transport.as_ref(),
            FocusSelection::OldestMembership => self
                .members
                .first()
                .and_then(|oldest| oldest.transport.as_ref()),
        }?;
        SfuId::of(transport)
    }
}

fn missing(sources: &HashSet<MediaSource>) -> Option<MissingMedia> {
    if sources.is_empty() {
        return None;
    }
    let only = |allowed: &[MediaSource]| sources.iter().all(|source| allowed.contains(source));
    let media = if only(&[MediaSource::Microphone]) {
        MissingMedia::Audio
    } else if only(&[MediaSource::Camera]) {
        MissingMedia::Video
    } else if only(&[MediaSource::Microphone, MediaSource::Camera]) {
        MissingMedia::AudioAndVideo
    } else if sources.contains(&MediaSource::Screen)
        && only(&[MediaSource::Screen, MediaSource::ScreenAudio])
    {
        MissingMedia::Screen
    } else {
        MissingMedia::Other
    };
    Some(media)
}

fn not_received(tiles: &[CallTile]) -> Option<CallNotice> {
    let mut troubled = tiles.iter().filter_map(|tile| match tile.state {
        TileState::NotReceived(what) if tile.kind == TileKind::Person => Some((tile, what)),
        _ => None,
    });
    let (first, what) = troubled.next()?;
    let others: HashSet<&str> = troubled
        .map(|(tile, _)| tile.member.user.as_str())
        .filter(|user| *user != first.member.user)
        .collect();
    Some(CallNotice::NotReceived {
        who: first.name.clone(),
        what,
        others: others.len(),
    })
}

fn undecryptable(tiles: &[CallTile]) -> Option<CallNotice> {
    let mut troubled = tiles.iter().filter_map(|tile| match tile.state {
        TileState::CantDecrypt(why) if tile.kind == TileKind::Person => Some((tile, why)),
        _ => None,
    });
    let (first, why) = troubled.next()?;
    Some(CallNotice::CantDecrypt {
        who: first.name.clone(),
        why,
        others: troubled.count(),
    })
}

fn remote_person(tile: &CallTile) -> bool {
    !tile.own && tile.kind == TileKind::Person
}

fn number_devices(tiles: &mut [CallTile]) {
    let mut devices: HashMap<String, u8> = HashMap::new();
    for tile in tiles.iter().filter(|tile| remote_person(tile)) {
        let count = devices.entry(tile.member.user.clone()).or_default();
        *count = count.saturating_add(1);
    }
    let mut numbered: HashMap<String, u8> = HashMap::new();
    for tile in tiles.iter_mut().filter(|tile| remote_person(tile)) {
        if devices
            .get(&tile.member.user)
            .is_some_and(|count| *count > 1)
        {
            let number = numbered.entry(tile.member.user.clone()).or_default();
            *number = number.saturating_add(1);
            tile.device = *number;
        }
    }
}
