use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tracing::Span;

use super::media::MediaCommand;
use crate::commands::view::{
    ActiveCall, CallPhase, CallTile, CantDecrypt, MicrophoneView, TileState,
};
use crate::domain::call::identity::{IdentityTable, MediaIdentity, member_spellings, spellings};
use crate::domain::call::keys::{
    KEY_LEN, KeyAction, KeyExchange, KeyHolder, MediaKey, ReceivedKey,
};
use crate::domain::call::media::{Decryption, MediaEvent, Microphone, SfuState};
use crate::domain::call::ring::{RingId, RingKind};
use crate::domain::call::sfu::{GrantRequest, GrantResult, SfuId, SfuRole, SfuTarget, plan};
use crate::domain::call::{
    CallIntent, CallMember, DeviceStanding, FocusSelection, MemberKey, OutgoingKey, SentMembership,
    Transport,
};
use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;
use crate::util::unix_now_ms;

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
    speaking: HashMap<SfuId, HashSet<MediaIdentity>>,
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

    fn forget(&mut self, sfu: &SfuId) {
        self.present.remove(sfu);
        self.muted.remove(sfu);
        self.speaking.remove(sfu);
    }
}

pub(super) struct Outgoing {
    pub(super) kind: RingKind,
    pub(super) ring: Option<RingId>,
    pub(super) answered: bool,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CallLoss {
    ServerUnreachable,
    MediaFailed,
    CutOff,
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
    pub(super) media_tx: mpsc::UnboundedSender<MediaCommand>,
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
    media_tx: mpsc::UnboundedSender<MediaCommand>,
    keys: KeyExchange,
    identities: IdentityTable,
    members: Vec<CallMember>,
    links: HashMap<SfuId, Link>,
    heard: Heard,
    standings: HashMap<MemberKey, DeviceStanding>,
    microphone: Microphone,
    outgoing_kind: Option<RingKind>,
    pub(super) outgoing: Option<Outgoing>,
}

pub(super) fn fresh_key() -> MediaKey {
    let mut bytes = [0_u8; KEY_LEN];
    rand::fill(&mut bytes);
    MediaKey::new(bytes)
}

impl Joined {
    pub(super) fn start(joining: Joining, members: &[CallMember]) -> (Self, Vec<Effect>) {
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
            media_tx: joining.media_tx,
            keys: KeyExchange::default(),
            identities: IdentityTable::default(),
            members: Vec::new(),
            links: HashMap::new(),
            heard: Heard::default(),
            standings: HashMap::new(),
            microphone: Microphone::Off,
            outgoing_kind: joining.outgoing,
            outgoing: None,
        };
        joined.command(MediaCommand::Microphone(true));
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
        let mut effects = self.replan();
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
        tracing::info!(call = self.call, sfu = %target.id, role = ?target.role, "asking for a call media grant");
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
        let someone_else = self
            .members
            .iter()
            .any(|member| member.user.user_id != self.me.user);
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
        let alone = !self
            .members
            .iter()
            .any(|member| member.user.user_id != self.me.user);
        if let Some(kind) = self.outgoing_kind.take().filter(|_| alone) {
            self.outgoing = Some(Outgoing {
                kind,
                ring: None,
                answered: false,
            });
            effects.push(Effect::Announce {
                membership_event_id: event_id,
                kind,
            });
        }
        effects
    }

    pub(super) fn granted(&mut self, sfu: &SfuId, outcome: GrantResult) -> Result<(), CallLoss> {
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
                    Err(CallLoss::ServerUnreachable)
                } else {
                    Ok(())
                }
            }
        }
    }

    pub(super) fn media_event(&mut self, event: MediaEvent) -> Result<(), CallLoss> {
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
                self.heard
                    .speaking
                    .insert(sfu, speakers.into_iter().collect());
            }
            MediaEvent::Decryption { identity, state } => {
                self.heard.decryption.insert(identity, state);
            }
            MediaEvent::Microphone(microphone) => self.microphone = microphone,
        }
        Ok(())
    }

    fn sfu_state(&mut self, sfu: &SfuId, state: SfuState) -> Result<(), CallLoss> {
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
                    (true, true) => return Err(CallLoss::CutOff),
                    (true, false) => return Err(CallLoss::ServerUnreachable),
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

    pub(super) fn set_microphone(&self, on: bool) {
        self.command(MediaCommand::Microphone(on));
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
        self.outgoing
            .as_ref()
            .is_some_and(|outgoing| outgoing.kind == RingKind::Ring && !outgoing.answered)
    }

    fn publish_link(&self) -> Option<&Link> {
        self.links
            .values()
            .find(|link| link.role == SfuRole::Publish)
    }

    fn phase(&self) -> CallPhase {
        match self.publish_link().map(|link| &link.state) {
            Some(LinkState::Reconnecting) => CallPhase::Reconnecting,
            Some(LinkState::Connected) if self.is_ringing_out() => CallPhase::Ringing,
            Some(LinkState::Connected) => CallPhase::Live,
            _ if self.membership.is_some() => CallPhase::Connecting,
            _ => CallPhase::Joining,
        }
    }

    pub(super) fn view(&self) -> ActiveCall {
        ActiveCall {
            call: self.call,
            room_id: self.room_id.clone(),
            room_name: self.room_name.clone(),
            phase: self.phase(),
            live_since_ms: self.live_since_ms,
            encrypted: self.encrypted,
            video: self.intent == CallIntent::Video,
            microphone: match self.microphone {
                Microphone::On => MicrophoneView::On,
                Microphone::Off => MicrophoneView::Off,
                Microphone::Failed(_) => MicrophoneView::Failed,
            },
            tiles: Arc::from(self.tiles()),
        }
    }

    fn tiles(&self) -> Vec<CallTile> {
        let mut tiles = Vec::new();
        if !self
            .members
            .iter()
            .any(|member| MemberKey::of(member) == self.me)
        {
            tiles.push(self.own_tile(NamedUser::unnamed(self.me.user.clone())));
        }
        for member in &self.members {
            if MemberKey::of(member) == self.me {
                tiles.push(self.own_tile(member.user.clone()));
            } else {
                tiles.push(self.remote_tile(member));
            }
        }
        tiles
    }

    fn own_tile(&self, name: NamedUser) -> CallTile {
        let speaking = self
            .own_spellings()
            .iter()
            .any(|identity| self.heard.is_speaking(identity));
        let state = match self.publish_link().map(|link| &link.state) {
            Some(LinkState::Connected | LinkState::Reconnecting) => TileState::Live,
            Some(LinkState::Failed(_)) => TileState::Unreachable,
            _ => TileState::Joining,
        };
        CallTile {
            member: self.me.clone(),
            name,
            own: true,
            state,
            muted: self.microphone != Microphone::On,
            speaking,
        }
    }

    fn remote_tile(&self, member: &CallMember) -> CallTile {
        let identities = member_spellings(member);
        let key = MemberKey::of(member);
        let muted = self.publishes_to(member).is_some_and(|sfu| {
            identities
                .iter()
                .any(|identity| self.heard.is_muted_on(&sfu, identity))
        });
        CallTile {
            state: self.remote_state(member, &key, &identities),
            muted,
            speaking: identities
                .iter()
                .any(|identity| self.heard.is_speaking(identity)),
            member: key,
            name: member.user.clone(),
            own: false,
        }
    }

    fn remote_state(
        &self,
        member: &CallMember,
        key: &MemberKey,
        identities: &[MediaIdentity; 3],
    ) -> TileState {
        if self.publishes_to(member).is_some_and(|sfu| {
            self.links
                .get(&sfu)
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
