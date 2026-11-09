mod connections;
mod controls;
mod hands;
mod heard;
mod media_events;
mod reactions;
mod tiles;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::Span;

use self::connections::{Meter, Readings};
use self::hands::HandRequest;
use self::heard::Heard;
use self::reactions::ReactionRequest;
use super::devices::Devices;
use super::media::MediaCommand;
use super::stage::Stage;
use crate::commands::view::{CallFailure, HandNews, ReactionNews};
use crate::domain::call::devices::DeviceChoices;
use crate::domain::call::hands::Hands;
use crate::domain::call::identity::{IdentityTable, MediaIdentity, spellings};
use crate::domain::call::keys::{KeyAction, KeyExchange, KeyHolder, MediaKey, OWN_KEY_LEN};
use crate::domain::call::media::{ConnectionQuality, MediaChoice, Microphone};
use crate::domain::call::reactions::{ReactionOption, Reactions};
use crate::domain::call::ring::{RingId, RingKind};
use crate::domain::call::sfu::{GrantRequest, GrantResult, SfuId, SfuRole, SfuTarget, plan};
use crate::domain::call::stats::SentVideo;
use crate::domain::call::video::{FeedId, FrameSize, ScreenRefusal, VideoKind};
use crate::domain::call::{
    CallIntent, CallMember, DeviceStanding, MemberKey, OutgoingKey, SentMembership, Transport,
};
use crate::domain::room::RoomId;
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
    pub(super) follow_speaker: bool,
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
            stage: Stage::following_speaker(joining.follow_speaker),
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
}
