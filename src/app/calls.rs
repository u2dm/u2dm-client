mod joined;
pub(super) mod lane;
mod media;
mod offer;
mod sheet;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, sleep_until, timeout};
use tracing::{Instrument, Span};

use self::joined::{Effect, Joined, Joining};
use self::lane::{LEAVE_WAIT, Lane, LaneEvent, MEMBERSHIP_LIFETIME, keep_membership};
use self::media::MediaDriver;
use self::offer::{Media, OfferFacts, Transports, offer};
use self::sheet::Sheet;
use super::event::{AppEvent, CallEvent, RingDeadline};
use super::input::EventSender;
use super::room_directory::RoomDirectory;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::WindowFocus;
use crate::commands::view::{
    CallFailure, CallSheetView, CallView, FailedCall, JoinOffer, JoinRefusal, LeavingCall,
    OfferView, RingView, RoomCallSummary, Toast,
};
use crate::domain::call::keys::ReceivedKey;
use crate::domain::call::media::{MediaChoice, MediaEvent, MediaSetup};
use crate::domain::call::ring::{
    CallSignal, OUTGOING_RING_LIFETIME_MS, OutgoingRing, Ring, RingId, RingKind,
};
use crate::domain::call::sfu::{GrantResult, SfuId};
use crate::domain::call::{
    CallIntent, DeviceId, KeySendReport, MemberKey, OwnMembership, Transport,
};
use crate::domain::room::{Room, RoomId};
use crate::domain::user_info::UserId;
use crate::ports::call::CallMediaPort;
use crate::ports::matrix::{CallSignalPort, RtcMembershipPort};
use crate::ports::output::AppOutputPort;
use crate::util::unix_now_ms;

const SIGNAL_CHANNEL_CAP: usize = 16;
const RING_CONFIRM_WAIT: Duration = Duration::from_secs(3);
const RING_OUT_WAIT: Duration = Duration::from_millis(OUTGOING_RING_LIFETIME_MS);

struct Own {
    port: Arc<dyn CallSignalPort>,
    rtc: Arc<dyn RtcMembershipPort>,
    session: u64,
    user: UserId,
    device: DeviceId,
}

impl Own {
    fn member(&self) -> MemberKey {
        MemberKey::new(self.user.to_string(), self.device.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answering {
    Pending,
    Showing,
    Silenced,
}

struct Incoming {
    ring: Ring,
    answering: Answering,
    serial: u64,
}

#[derive(Debug, Clone, Copy)]
enum RingEnd {
    Declined,
    Answered,
    DeclinedElsewhere,
    AnsweredElsewhere,
    CallerLeft,
    NeverJoined,
    Expired,
    RoomGone,
    Superseded,
}

impl RingEnd {
    const fn reason(self) -> &'static str {
        match self {
            Self::Declined => "you declined it",
            Self::Answered => "you answered it",
            Self::DeclinedElsewhere => "you declined it on another device",
            Self::AnsweredElsewhere => "you answered it on another device",
            Self::CallerLeft => "the caller left the call",
            Self::NeverJoined => "the caller never joined the call",
            Self::Expired => "nobody answered in time",
            Self::RoomGone => "the room is no longer listed",
            Self::Superseded => "a newer call rings",
        }
    }
}

struct Ended {
    kind: UserMessageKind,
    detail: String,
}

struct JoinRequest {
    room_id: RoomId,
    intent: CallIntent,
    media: MediaChoice,
    leaving: Option<RoomId>,
}

struct Failed {
    room_id: RoomId,
    intent: CallIntent,
    media: MediaChoice,
    failure: CallFailure,
}

enum Blocked {
    NotListed,
    Refused(JoinRefusal),
    AlreadyIn,
    InAnotherCall(RoomId),
}

impl fmt::Display for Blocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotListed => f.write_str("the room is not listed"),
            Self::Refused(refusal) => write!(f, "the room refuses it: {refusal:?}"),
            Self::AlreadyIn => f.write_str("you are already in this call"),
            Self::InAnotherCall(current) => write!(
                f,
                "it would leave your call in {current}, and leaving it was not agreed"
            ),
        }
    }
}

pub(super) struct Calls {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    media: Option<Arc<dyn CallMediaPort>>,
    rings: TaskGroup,
    actions: TaskGroup,
    lane: TaskGroup,
    media_tasks: TaskGroup,
    key_tasks: TaskGroup,
    own: Option<Own>,
    focus: WindowFocus,
    incoming: Option<Incoming>,
    serial: u64,
    selected: Option<RoomId>,
    verified: bool,
    transports: Transports,
    discovering: bool,
    pending_join: Option<JoinRequest>,
    joined: Option<Joined>,
    failed: Option<Failed>,
    sheet: Option<Sheet>,
    sheet_serial: i32,
    generation: u64,
    stale_checked: bool,
    published: CallView,
}

impl Calls {
    pub(super) fn new(
        output: Arc<dyn AppOutputPort>,
        events: EventSender,
        media: Option<Arc<dyn CallMediaPort>>,
    ) -> Self {
        Self {
            output,
            events,
            media,
            rings: TaskGroup::new("call-rings"),
            actions: TaskGroup::new("call-actions"),
            lane: TaskGroup::new("call-lane"),
            media_tasks: TaskGroup::new("call-media"),
            key_tasks: TaskGroup::new("call-keys"),
            own: None,
            focus: WindowFocus::default(),
            incoming: None,
            serial: 0,
            selected: None,
            verified: false,
            transports: Transports::Unknown,
            discovering: false,
            pending_join: None,
            joined: None,
            failed: None,
            sheet: None,
            sheet_serial: 0,
            generation: 0,
            stale_checked: false,
            published: CallView::default(),
        }
    }

    pub(super) fn begin_session(
        &mut self,
        background: &mut TaskGroup,
        port: Arc<dyn CallSignalPort>,
        rtc: Arc<dyn RtcMembershipPort>,
        session: u64,
        user: UserId,
        device: DeviceId,
    ) {
        let events = self.events.clone();
        let token = background.token();
        let follower = Arc::clone(&port);
        background.spawn(async move {
            let (signals_tx, signals_rx) = mpsc::channel(SIGNAL_CHANNEL_CAP);
            let follow = async {
                tokio::join!(
                    follow_signals(follower.as_ref(), signals_tx),
                    forward_signals(&events, session, signals_rx),
                )
            };
            token.run_until_cancelled(follow).await;
        });
        self.own = Some(Own {
            port,
            rtc,
            session,
            user,
            device,
        });
        self.discover();
    }

    fn discover(&mut self) {
        let Some(own) = &self.own else {
            return;
        };
        if self.discovering {
            return;
        }
        self.discovering = true;
        let rtc = Arc::clone(&own.rtc);
        let session = own.session;
        let events = self.events.clone();
        self.actions.spawn(async move {
            let found = match rtc.own_transports().await {
                Ok(found) => Some(found),
                Err(e) => {
                    tracing::warn!("could not learn this homeserver's call server: {e}");
                    None
                }
            };
            send(&events, CallEvent::Transports { session, found });
        });
    }

    pub(super) fn transports_found(
        &mut self,
        found: Option<Vec<Transport>>,
        directory: &RoomDirectory,
    ) {
        self.discovering = false;
        match found {
            Some(found) if !found.is_empty() => {
                tracing::info!(count = found.len(), "found this homeserver's call server");
                self.transports = Transports::Found(found);
            }
            Some(_) => {
                tracing::info!("this homeserver offers no call server, so calls cannot be joined");
                self.transports = Transports::Missing;
            }
            None => {}
        }
        self.settle_pending_join(directory);
        self.publish(directory);
    }

    fn settle_pending_join(&mut self, directory: &RoomDirectory) {
        let Some(request) = self.pending_join.take() else {
            return;
        };
        if self.transports.first().is_some() {
            self.join(request, directory);
        } else if self.joined.is_some() {
            self.toast_about(
                UserMessageKind::CallServerUnreachable,
                &request.room_id,
                directory,
            );
        } else {
            tracing::info!(room_id = %request.room_id, "the call could not start without a call server");
            self.failed = Some(Failed {
                room_id: request.room_id,
                intent: request.intent,
                media: request.media,
                failure: CallFailure::ServerUnreachable,
            });
        }
    }

    pub(super) fn signal(&mut self, signal: CallSignal, directory: &RoomDirectory) {
        match signal {
            CallSignal::Rang(ring) => self.rang(*ring, directory),
            CallSignal::Declined { room_id, ring, by } => {
                if !self.declined_out(&ring, &by, directory) {
                    self.declined_by(&room_id, &ring, &by, directory);
                }
            }
            CallSignal::Key { room_id, key } => self.key_arrived(&room_id, *key),
        }
    }

    fn key_arrived(&mut self, room_id: &RoomId, key: ReceivedKey) {
        let Some(joined) = self
            .joined
            .as_mut()
            .filter(|joined| joined.room_id == *room_id)
        else {
            tracing::debug!(%room_id, "a call key for a call you are not in");
            return;
        };
        let span = joined.span.clone();
        let _in_call = span.enter();
        let effects = joined.received_key(key);
        self.run_effects(effects);
    }

    fn rang(&mut self, ring: Ring, directory: &RoomDirectory) {
        if let Some(reason) = self.ignored(&ring, directory) {
            tracing::debug!(room_id = %ring.room_id, ring = %ring.id, reason, "a call notification rings nothing");
            return;
        }
        if self.incoming.is_some() {
            self.stop(RingEnd::Superseded);
        }
        tracing::info!(room_id = %ring.room_id, ring = %ring.id, video = ring.is_video(), "a call rings");
        self.serial = self.serial.wrapping_add(1);
        self.arm_windows(&ring);
        self.incoming = Some(Incoming {
            ring,
            answering: Answering::Pending,
            serial: self.serial,
        });
        self.directory_changed(directory);
    }

    fn ignored(&self, ring: &Ring, directory: &RoomDirectory) -> Option<&'static str> {
        if ring.kind == RingKind::Notify {
            return Some("it does not ask to ring");
        }
        if self
            .incoming
            .as_ref()
            .is_some_and(|incoming| incoming.ring.id == ring.id)
        {
            return Some("it already rings");
        }
        let Some(room) = directory.room(&ring.room_id) else {
            return Some("its room is not listed");
        };
        let joined = self.own.as_ref().is_some_and(|own| {
            room.call
                .as_ref()
                .is_some_and(|call| call.joined_by(&own.user))
        });
        joined.then_some("you are already in the call")
    }

    fn declined_out(&mut self, ring: &RingId, by: &UserId, directory: &RoomDirectory) -> bool {
        let ours = self
            .joined
            .as_ref()
            .is_some_and(|joined| joined.announced_as(ring));
        let by_us = self.own.as_ref().is_some_and(|own| own.user == *by);
        if !ours || by_us {
            return false;
        }
        let name = self
            .joined
            .as_ref()
            .and_then(|joined| directory.room(&joined.room_id))
            .filter(|room| room.is_direct)
            .map_or_else(|| by.to_string(), |room| room.display_name.clone());
        tracing::info!(%by, "the call you started was declined");
        self.end_call(
            Some(Ended {
                kind: UserMessageKind::CallDeclinedThere,
                detail: name,
            }),
            directory,
        );
        true
    }

    fn declined_by(
        &mut self,
        room_id: &RoomId,
        ring: &RingId,
        by: &UserId,
        directory: &RoomDirectory,
    ) {
        let Some(incoming) = &self.incoming else {
            return;
        };
        if incoming.ring.id != *ring {
            return;
        }
        let ours = self.own.as_ref().is_some_and(|own| own.user == *by);
        if !ours {
            tracing::debug!(%room_id, %by, "someone else declined the call ringing here");
            return;
        }
        self.stop(RingEnd::DeclinedElsewhere);
        self.publish(directory);
    }

    pub(super) fn directory_changed(&mut self, directory: &RoomDirectory) {
        if let Some(end) = self.judge_ring(directory) {
            self.stop(end);
        }
        self.follow_joined(directory);
        self.follow_sheet(directory);
        self.follow_failed(directory);
        self.clear_stale_memberships(directory);
        self.publish(directory);
    }

    fn follow_joined(&mut self, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let Some(room) = directory.room(&joined.room_id) else {
            tracing::info!(call = joined.call, room_id = %joined.room_id, "the call's room left the list, hanging up");
            self.end_call(None, directory);
            return;
        };
        let members = room
            .call
            .as_ref()
            .map(|call| call.members().to_vec())
            .unwrap_or_default();
        let effects = joined.members_changed(members);
        self.run_effects(effects);
    }

    fn follow_sheet(&mut self, directory: &RoomDirectory) {
        let Some(sheet) = &self.sheet else {
            return;
        };
        if directory.room(&sheet.room_id).is_none() {
            tracing::debug!(room_id = %sheet.room_id, "the room the call sheet asks about left the list");
            self.sheet = None;
        }
    }

    fn follow_failed(&mut self, directory: &RoomDirectory) {
        let Some(failed) = &self.failed else {
            return;
        };
        if directory.room(&failed.room_id).is_none() {
            tracing::debug!(room_id = %failed.room_id, "the room of the failed call left the list");
            self.failed = None;
        }
    }

    fn clear_stale_memberships(&mut self, directory: &RoomDirectory) {
        if self.stale_checked || !directory.rooms_landed() {
            return;
        }
        let Some(own) = &self.own else {
            return;
        };
        self.stale_checked = true;
        let me = own.member();
        let joined_room = self.joined.as_ref().map(|joined| joined.room_id.clone());
        for room in directory.rooms() {
            let stale = room
                .call
                .as_ref()
                .is_some_and(|call| call.member(&me).is_some());
            if !stale || joined_room.as_ref() == Some(&room.id) {
                continue;
            }
            tracing::info!(room_id = %room.id, "clearing a call membership this device left behind");
            let rtc = Arc::clone(&own.rtc);
            let room_id = room.id.clone();
            self.actions.spawn(async move {
                if let Err(e) = rtc.clear_membership(&room_id).await {
                    tracing::warn!(%room_id, "could not clear a stale call membership: {e}");
                }
            });
        }
    }

    fn judge_ring(&mut self, directory: &RoomDirectory) -> Option<RingEnd> {
        let incoming = self.incoming.as_mut()?;
        let room_id = &incoming.ring.room_id;
        let Some(room) = directory.room(room_id) else {
            return Some(RingEnd::RoomGone);
        };
        let call = room.call.as_deref();
        if let Some(own) = &self.own
            && call.is_some_and(|call| call.joined_elsewhere(&own.user, &own.device))
        {
            return Some(RingEnd::AnsweredElsewhere);
        }
        let caller_in = call.is_some_and(|call| call.joined_by(&incoming.ring.caller.user_id));
        match (incoming.answering, caller_in) {
            (Answering::Pending, true) => {
                tracing::debug!(%room_id, ring = %incoming.ring.id, "the caller is in the call, so it rings");
                incoming.answering = Answering::Showing;
                None
            }
            (Answering::Showing | Answering::Silenced, false) => Some(RingEnd::CallerLeft),
            _ => None,
        }
    }

    pub(super) fn ring_window(
        &mut self,
        ring: &RingId,
        deadline: RingDeadline,
        directory: &RoomDirectory,
    ) {
        let Some(incoming) = &self.incoming else {
            return;
        };
        if incoming.ring.id != *ring {
            return;
        }
        match deadline {
            RingDeadline::Unconfirmed if incoming.answering == Answering::Pending => {
                self.stop(RingEnd::NeverJoined);
            }
            RingDeadline::Unconfirmed => return,
            RingDeadline::Expired => self.stop(RingEnd::Expired),
        }
        self.publish(directory);
    }

    pub(super) fn decline(&mut self, room_id: &RoomId, directory: &RoomDirectory) {
        let Some(incoming) = self
            .incoming
            .as_ref()
            .filter(|incoming| incoming.ring.room_id == *room_id)
        else {
            tracing::debug!(%room_id, "nothing rings here to decline");
            return;
        };
        let ring = incoming.ring.id.clone();
        if let Some(own) = &self.own {
            let port = Arc::clone(&own.port);
            let events = self.events.clone();
            let room_id = room_id.clone();
            self.actions.spawn(async move {
                let sent = match port.decline(&room_id, &ring).await {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!(%room_id, "the call could not be declined: {e}");
                        false
                    }
                };
                send(&events, CallEvent::Declined { room_id, sent });
            });
        }
        self.stop(RingEnd::Declined);
        self.publish(directory);
    }

    pub(super) fn decline_settled(&self, room_id: &RoomId, sent: bool, directory: &RoomDirectory) {
        if !sent {
            self.toast_about(UserMessageKind::CallDeclineFailed, room_id, directory);
        }
    }

    fn toast_about(&self, kind: UserMessageKind, room_id: &RoomId, directory: &RoomDirectory) {
        let name = directory
            .room(room_id)
            .map_or_else(|| room_id.to_string(), |room| room.display_name.clone());
        show_toast(
            self.output.as_ref(),
            Toast::Error(UserMessage::about(kind, &name)),
        );
    }

    pub(super) fn silence(&mut self, directory: &RoomDirectory) {
        let Some(incoming) = self
            .incoming
            .as_mut()
            .filter(|incoming| incoming.answering == Answering::Showing)
        else {
            return;
        };
        tracing::debug!(room_id = %incoming.ring.room_id, "the ring was silenced");
        incoming.answering = Answering::Silenced;
        self.publish(directory);
    }

    pub(super) fn open_sheet(&mut self, room_id: &RoomId, video: bool, directory: &RoomDirectory) {
        let Some(room) = directory.room(room_id) else {
            tracing::debug!(%room_id, "no listed room to offer a call in");
            return;
        };
        if self.offer_for(room) == JoinOffer::InCall {
            tracing::debug!(%room_id, "you are already in this call");
            return;
        }
        tracing::debug!(%room_id, video, "asking how to join the call");
        self.sheet_serial = self.sheet_serial.wrapping_add(1);
        self.sheet = Some(Sheet::open(room, video, self.sheet_serial));
        self.publish(directory);
    }

    pub(super) fn close_sheet(&mut self, directory: &RoomDirectory) {
        if self.sheet.take().is_some() {
            self.publish(directory);
        }
    }

    pub(super) fn start_call(
        &mut self,
        room_id: RoomId,
        video: bool,
        media: MediaChoice,
        leaving: Option<RoomId>,
        directory: &RoomDirectory,
    ) {
        let intent = if video {
            CallIntent::Video
        } else {
            CallIntent::Voice
        };
        self.join(
            JoinRequest {
                room_id,
                intent,
                media,
                leaving,
            },
            directory,
        );
    }

    pub(super) fn join_call(
        &mut self,
        room_id: RoomId,
        media: MediaChoice,
        leaving: Option<RoomId>,
        directory: &RoomDirectory,
    ) {
        let Some(intent) = running_intent(directory, &room_id) else {
            tracing::info!(%room_id, "the call ended before you joined it");
            if let Some(sheet) = self.sheet.as_mut().filter(|sheet| sheet.room_id == room_id) {
                sheet.call_ended();
                self.publish(directory);
            }
            return;
        };
        self.join(
            JoinRequest {
                room_id,
                intent,
                media,
                leaving,
            },
            directory,
        );
    }

    pub(super) fn answer(
        &mut self,
        room_id: RoomId,
        camera: bool,
        leaving: Option<RoomId>,
        directory: &RoomDirectory,
    ) {
        let ringing = self
            .incoming
            .as_ref()
            .filter(|incoming| incoming.ring.room_id == room_id)
            .map(|incoming| incoming.ring.intent.unwrap_or(CallIntent::Voice));
        let Some(intent) = ringing else {
            tracing::debug!(%room_id, "nothing rings here to answer");
            return;
        };
        self.join(
            JoinRequest {
                room_id,
                intent,
                media: MediaChoice {
                    microphone: true,
                    camera,
                },
                leaving,
            },
            directory,
        );
    }

    fn admitted<'a>(
        &self,
        request: &JoinRequest,
        directory: &'a RoomDirectory,
    ) -> Result<&'a Room, Blocked> {
        let room = directory.room(&request.room_id).ok_or(Blocked::NotListed)?;
        match self.offer_for(room) {
            JoinOffer::Refused(refusal) => return Err(Blocked::Refused(refusal)),
            JoinOffer::InCall => return Err(Blocked::AlreadyIn),
            JoinOffer::Start | JoinOffer::Join => {}
        }
        match &self.joined {
            Some(joined) if request.leaving.as_ref() != Some(&joined.room_id) => {
                Err(Blocked::InAnotherCall(joined.room_id.clone()))
            }
            _ => Ok(room),
        }
    }

    fn explain_refusal(&self, blocked: &Blocked, room_id: &RoomId, directory: &RoomDirectory) {
        tracing::info!(%room_id, reason = %blocked, "this call cannot be joined");
        if matches!(blocked, Blocked::Refused(_)) {
            self.toast_about(UserMessageKind::CallUnjoinable, room_id, directory);
        }
    }

    fn wait_for_transports(&mut self, request: JoinRequest, directory: &RoomDirectory) {
        tracing::debug!(room_id = %request.room_id, "learning the call server before joining");
        self.pending_join = Some(request);
        self.discover();
        self.publish(directory);
    }

    fn join(&mut self, request: JoinRequest, directory: &RoomDirectory) {
        let room = match self.admitted(&request, directory) {
            Ok(room) => room,
            Err(blocked) => {
                self.explain_refusal(&blocked, &request.room_id, directory);
                return;
            }
        };
        self.sheet = None;
        self.failed = None;
        if self.rings_in(&request.room_id) {
            self.stop(RingEnd::Answered);
        }
        let Some(transport) = self.transports.first().cloned() else {
            self.wait_for_transports(request, directory);
            return;
        };
        if self.joined.is_some() {
            self.end_call(None, directory);
        }
        let Some(joining) = self.joining(room, &request, transport) else {
            return;
        };
        let members = room
            .call
            .as_ref()
            .map(|call| call.members().to_vec())
            .unwrap_or_default();
        tracing::info!(
            call = joining.call,
            room_id = %request.room_id,
            intent = ?request.intent,
            microphone = request.media.microphone,
            camera = request.media.camera,
            encrypted = room.is_encrypted,
            "joining the call"
        );
        let (joined, effects) = Joined::start(joining, &members);
        self.joined = Some(joined);
        self.run_effects(effects);
        self.publish(directory);
    }

    fn joining(
        &mut self,
        room: &Room,
        request: &JoinRequest,
        transport: Transport,
    ) -> Option<Joining> {
        let own = self.own.as_ref()?;
        let media = Arc::clone(self.media.as_ref()?);
        self.generation = self.generation.wrapping_add(1);
        let call = self.generation;
        let span = tracing::debug_span!("call", room_id = %room.id, call);
        let me = own.member();
        let member_id = format!("{}:{}", own.user, own.device);
        let (media_tx, media_rx) = mpsc::unbounded_channel();
        let driver = MediaDriver {
            port: media,
            setup: MediaSetup {
                room_id: room.id.clone(),
                encrypted: room.is_encrypted,
            },
            events: self.events.clone(),
            call,
        };
        self.media_tasks
            .spawn(driver.run(media_rx).instrument(span.clone()));
        let (leave_tx, leave_rx) = oneshot::channel();
        let lane = Lane {
            port: Arc::clone(&own.rtc),
            room_id: room.id.clone(),
            membership: OwnMembership {
                device: own.device.clone(),
                member_id: member_id.clone(),
                intent: request.intent,
                transport: transport.clone(),
                created_ms: None,
                lifetime_ms: u64::try_from(MEMBERSHIP_LIFETIME.as_millis()).unwrap_or(u64::MAX),
            },
            events: self.events.clone(),
            call,
        };
        let token = self.lane.token();
        self.lane.spawn(
            async move {
                token
                    .run_until_cancelled(keep_membership(lane, leave_rx))
                    .await;
            }
            .instrument(span.clone()),
        );
        let alone = room
            .call
            .as_ref()
            .is_none_or(|existing| existing.others(&me).next().is_none());
        let outgoing = alone.then_some(if room.is_direct {
            RingKind::Ring
        } else {
            RingKind::Notify
        });
        Some(Joining {
            call,
            span,
            room_id: room.id.clone(),
            room_name: room.display_name.clone(),
            intent: request.intent,
            encrypted: room.is_encrypted,
            me,
            member_id,
            transport,
            leave_tx,
            media_tx,
            media: request.media,
            outgoing,
        })
    }

    pub(super) fn leave_call(&mut self, directory: &RoomDirectory) {
        self.end_call(None, directory);
    }

    pub(super) fn retry_failed(&mut self, directory: &RoomDirectory) {
        let Some(failed) = self.failed.take() else {
            tracing::debug!("no failed call to try again");
            return;
        };
        tracing::info!(room_id = %failed.room_id, "trying the failed call again");
        let intent = running_intent(directory, &failed.room_id).unwrap_or(failed.intent);
        self.join(
            JoinRequest {
                room_id: failed.room_id,
                intent,
                media: failed.media,
                leaving: None,
            },
            directory,
        );
        self.publish(directory);
    }

    pub(super) fn dismiss_failed(&mut self, directory: &RoomDirectory) {
        if self.failed.take().is_some() {
            self.publish(directory);
        }
    }

    pub(super) fn announce_again(&mut self, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let span = joined.span.clone();
        let _in_call = span.enter();
        if let Some(effect) = joined.announce_again() {
            self.run_effect(effect);
            self.publish(directory);
        }
    }

    pub(super) fn left_room(&mut self, room_id: &RoomId, directory: &RoomDirectory) {
        if self
            .joined
            .as_ref()
            .is_some_and(|joined| joined.room_id == *room_id)
        {
            self.end_call(None, directory);
        }
    }

    pub(super) fn set_microphone(&mut self, on: bool, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let span = joined.span.clone();
        let _in_call = span.enter();
        joined.set_microphone(on);
        self.publish(directory);
    }

    pub(super) fn set_camera(&mut self, on: bool, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let span = joined.span.clone();
        let _in_call = span.enter();
        joined.set_camera(on);
        self.publish(directory);
    }

    pub(super) fn set_screen_share(&mut self, on: bool, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let span = joined.span.clone();
        let _in_call = span.enter();
        joined.set_screen_share(on);
        self.publish(directory);
    }

    pub(super) fn hang_up(&mut self) {
        if let Some(mut joined) = self.joined.take() {
            tracing::info!(call = joined.call, room_id = %joined.room_id, "hanging up");
            joined.order_farewell();
            self.key_tasks.cancel_and_detach();
        }
    }

    fn end_call(&mut self, ended: Option<Ended>, directory: &RoomDirectory) {
        let Some(mut joined) = self.joined.take() else {
            return;
        };
        tracing::info!(call = joined.call, room_id = %joined.room_id, "leaving the call");
        joined.order_farewell();
        self.key_tasks.cancel_and_detach();
        if let Some(ended) = ended {
            show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::about(ended.kind, &ended.detail)),
            );
        }
        drop(joined);
        self.publish(directory);
    }

    fn fail(&mut self, failure: CallFailure, directory: &RoomDirectory) {
        let Some(joined) = &self.joined else {
            return;
        };
        tracing::info!(call = joined.call, room_id = %joined.room_id, ?failure, "the call failed");
        self.failed = Some(Failed {
            room_id: joined.room_id.clone(),
            intent: joined.intent(),
            media: joined.media_sent(),
            failure,
        });
        self.end_call(None, directory);
    }

    fn call_span(&self) -> Span {
        self.joined
            .as_ref()
            .map_or_else(Span::none, |joined| joined.span.clone())
    }

    pub(super) fn is_current(&self, call: u64) -> bool {
        self.joined
            .as_ref()
            .is_some_and(|joined| joined.call == call)
    }

    pub(super) fn call_event(&mut self, event: CallEvent, directory: &RoomDirectory) {
        let span = self.call_span();
        let _in_call = span.enter();
        match event {
            CallEvent::Lane { event, .. } => self.lane_event(event, directory),
            CallEvent::Media { event, .. } => self.media_event(event, directory),
            CallEvent::MediaFailed { why, .. } => {
                tracing::warn!("the call's media did not start: {why}");
                self.fail(CallFailure::MediaFailed, directory);
            }
            CallEvent::Granted { sfu, outcome, .. } => self.granted(&sfu, *outcome, directory),
            CallEvent::Announced { ring, .. } => self.announced(ring, directory),
            CallEvent::RingOutTimeout { .. } => self.ring_out_timeout(directory),
            CallEvent::KeyWake { .. } => {
                if let Some(joined) = self.joined.as_mut() {
                    let effects = joined.key_wake();
                    self.run_effects(effects);
                }
            }
            CallEvent::KeysSent { report, .. } => keys_sent(&report),
            CallEvent::Standings { standings, .. } => {
                if let Some(joined) = self.joined.as_mut() {
                    joined.standings_known(standings);
                }
                self.publish(directory);
            }
            CallEvent::Signal { .. }
            | CallEvent::RingWindow { .. }
            | CallEvent::Declined { .. }
            | CallEvent::Transports { .. } => {}
        }
    }

    fn lane_event(&mut self, event: LaneEvent, directory: &RoomDirectory) {
        match event {
            LaneEvent::Joined(sent) => {
                if let Some(joined) = self.joined.as_mut() {
                    let effects = joined.lane_joined(sent);
                    self.run_effects(effects);
                }
                self.publish(directory);
            }
            LaneEvent::Failed(why) => {
                tracing::warn!("the call membership failed: {why}");
                self.fail(CallFailure::JoinFailed, directory);
            }
        }
    }

    fn media_event(&mut self, event: MediaEvent, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        if let Err(failure) = joined.media_event(event) {
            self.fail(failure, directory);
            return;
        }
        self.publish(directory);
    }

    fn granted(&mut self, sfu: &SfuId, outcome: GrantResult, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        if let Err(failure) = joined.granted(sfu, outcome) {
            self.fail(failure, directory);
            return;
        }
        self.publish(directory);
    }

    fn announced(&mut self, ring: Option<RingId>, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let call = joined.call;
        if joined.announced(ring) {
            let events = self.events.clone();
            let token = self.key_tasks.token();
            self.key_tasks.spawn(async move {
                if token
                    .run_until_cancelled(sleep(RING_OUT_WAIT))
                    .await
                    .is_some()
                {
                    send(&events, CallEvent::RingOutTimeout { call });
                }
            });
        }
        self.publish(directory);
    }

    fn ring_out_timeout(&mut self, directory: &RoomDirectory) {
        let Some(joined) = self
            .joined
            .as_ref()
            .filter(|joined| joined.is_ringing_out())
        else {
            return;
        };
        tracing::info!(call = joined.call, "nobody answered the call you started");
        let detail = directory.room(&joined.room_id).map_or_else(
            || joined.room_id.to_string(),
            |room| room.display_name.clone(),
        );
        self.end_call(
            Some(Ended {
                kind: UserMessageKind::CallNoAnswer,
                detail,
            }),
            directory,
        );
    }

    fn run_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            self.run_effect(effect);
        }
    }

    fn run_effect(&mut self, effect: Effect) {
        let (Some(joined), Some(own)) = (&self.joined, &self.own) else {
            return;
        };
        let call = joined.call;
        let room_id = joined.room_id.clone();
        let events = self.events.clone();
        let span = joined.span.clone();
        match effect {
            Effect::SendKey { to, key } => {
                let port = Arc::clone(&own.port);
                self.key_tasks.spawn(
                    async move {
                        let report = port.send_key(&room_id, &key, &to).await;
                        send(&events, CallEvent::KeysSent { call, report });
                    }
                    .instrument(span),
                );
            }
            Effect::WakeKeys(at) => {
                let wake = joined.wake_at(at);
                let token = self.key_tasks.token();
                self.key_tasks.spawn(async move {
                    if token.run_until_cancelled(sleep_until(wake)).await.is_some() {
                        send(&events, CallEvent::KeyWake { call });
                    }
                });
            }
            Effect::Grant(request) => {
                let rtc = Arc::clone(&own.rtc);
                self.actions.spawn(
                    async move {
                        let outcome = rtc.sfu_grant(&room_id, &request).await;
                        let sfu = request.target.id.clone();
                        send(
                            &events,
                            CallEvent::Granted {
                                call,
                                sfu,
                                outcome: Box::new(outcome),
                            },
                        );
                    }
                    .instrument(span),
                );
            }
            Effect::Announce {
                membership_event_id,
                kind,
            } => {
                let port = Arc::clone(&own.port);
                let ring = OutgoingRing {
                    membership_event_id,
                    kind,
                    intent: joined.intent(),
                    sent_ms: unix_now_ms(),
                };
                self.actions.spawn(
                    async move {
                        let ring = match port.announce_call(&room_id, &ring).await {
                            Ok(ring) => {
                                tracing::info!(%room_id, ?kind, "announced the call");
                                Some(ring)
                            }
                            Err(e) => {
                                tracing::warn!(%room_id, "the call was not announced: {e}");
                                None
                            }
                        };
                        send(&events, CallEvent::Announced { call, ring });
                    }
                    .instrument(span),
                );
            }
            Effect::Standings(members) => {
                let port = Arc::clone(&own.port);
                self.actions.spawn(
                    async move {
                        let standings = port.device_standing(&members).await;
                        send(&events, CallEvent::Standings { call, standings });
                    }
                    .instrument(span),
                );
            }
        }
    }

    pub(super) fn trust_changed(&mut self, verified: bool, directory: &RoomDirectory) {
        if self.verified == verified {
            return;
        }
        self.verified = verified;
        self.publish(directory);
    }

    pub(super) fn focus_changed(&mut self, focus: WindowFocus, directory: &RoomDirectory) {
        if self.focus == focus {
            return;
        }
        self.focus = focus;
        self.publish(directory);
    }

    pub(super) fn selected(&mut self, room_id: Option<&RoomId>, directory: &RoomDirectory) {
        self.selected = room_id.cloned();
        self.publish(directory);
    }

    pub(super) fn rings_in(&self, room_id: &RoomId) -> bool {
        self.incoming
            .as_ref()
            .is_some_and(|incoming| incoming.ring.room_id == *room_id)
    }

    pub(super) async fn restart(&mut self) {
        let abandoned = self.joined.take();
        if let Some(joined) = &abandoned {
            tracing::info!(call = joined.call, room_id = %joined.room_id, "the session stopped, leaving the call to its delayed leave");
            self.lane.cancel_and_detach();
        }
        self.lane.drain().await;
        drop(abandoned);
        self.media_tasks.drain().await;
        tokio::join!(
            self.rings.restart(),
            self.actions.restart(),
            self.lane.restart(),
            self.media_tasks.restart(),
            self.key_tasks.restart(),
        );
        self.reset();
    }

    pub(super) async fn shutdown(&mut self) {
        self.hang_up();
        let lane = &mut self.lane;
        let media = &mut self.media_tasks;
        let farewell = async move {
            tokio::join!(lane.drain(), media.drain());
        };
        if timeout(LEAVE_WAIT, farewell).await.is_err() {
            tracing::info!("the call's farewell did not finish before quitting");
        }
        tokio::join!(
            self.rings.shutdown(),
            self.actions.shutdown(),
            self.lane.shutdown(),
            self.media_tasks.shutdown(),
            self.key_tasks.shutdown(),
        );
        self.reset();
    }

    fn reset(&mut self) {
        self.own = None;
        self.incoming = None;
        self.selected = None;
        self.verified = false;
        self.transports = Transports::Unknown;
        self.discovering = false;
        self.pending_join = None;
        self.joined = None;
        self.failed = None;
        self.sheet = None;
        self.stale_checked = false;
        self.published = CallView::default();
    }

    fn stop(&mut self, end: RingEnd) {
        let Some(incoming) = self.incoming.take() else {
            return;
        };
        tracing::info!(
            room_id = %incoming.ring.room_id,
            ring = %incoming.ring.id,
            reason = end.reason(),
            "the call stopped ringing"
        );
        self.rings.cancel_and_detach();
    }

    fn arm_windows(&mut self, ring: &Ring) {
        let expires_in = Duration::from_millis(ring.expires_ms.saturating_sub(unix_now_ms()));
        for (wait, deadline) in [
            (RING_CONFIRM_WAIT, RingDeadline::Unconfirmed),
            (expires_in, RingDeadline::Expired),
        ] {
            let events = self.events.clone();
            let ring = ring.id.clone();
            let token = self.rings.token();
            self.rings.spawn(async move {
                if token.run_until_cancelled(sleep(wait)).await.is_none() {
                    return;
                }
                send(&events, CallEvent::RingWindow { ring, deadline });
            });
        }
    }

    fn offer_for(&self, room: &Room) -> JoinOffer {
        offer(&OfferFacts {
            media: match self.media.as_ref().map(|media| media.broken()) {
                None => Media::NotBuilt,
                Some(Some(_)) => Media::Broken,
                Some(None) => Media::Ready,
            },
            transports: &self.transports,
            room,
            verified: self.verified,
            in_this_call: self
                .joined
                .as_ref()
                .is_some_and(|joined| joined.room_id == room.id),
        })
    }

    fn publish(&mut self, directory: &RoomDirectory) {
        let view = CallView {
            ring: self.ring_view(directory),
            here: self.here(directory),
            offer: self.offer_view(directory),
            sheet: self.sheet_view(directory),
            active: self.joined.as_ref().map(Joined::view),
            failed: self.failed_view(directory),
        };
        if view == self.published {
            return;
        }
        self.published = view.clone();
        self.output
            .publish(Box::new(move |state| state.call = view));
    }

    fn offer_view(&self, directory: &RoomDirectory) -> Option<OfferView> {
        self.own.as_ref()?;
        let room = directory.room(self.selected.as_ref()?)?;
        Some(OfferView {
            room_id: room.id.clone(),
            offer: self.offer_for(room),
        })
    }

    fn sheet_view(&self, directory: &RoomDirectory) -> Option<CallSheetView> {
        let sheet = self.sheet.as_ref()?;
        let room = directory.room(&sheet.room_id)?;
        let leaving = self
            .joined
            .as_ref()
            .filter(|joined| joined.room_id != room.id)
            .map(|joined| LeavingCall {
                room_id: joined.room_id.clone(),
                room_name: joined.room_name().to_owned(),
            });
        Some(sheet.view(room, self.offer_for(room), leaving))
    }

    fn failed_view(&self, directory: &RoomDirectory) -> Option<FailedCall> {
        let failed = self.failed.as_ref()?;
        let room = directory.room(&failed.room_id)?;
        Some(FailedCall {
            room_id: room.id.clone(),
            room_name: room.display_name.clone(),
            failure: failed.failure,
            retry: matches!(self.offer_for(room), JoinOffer::Join | JoinOffer::Start),
        })
    }

    fn ring_view(&self, directory: &RoomDirectory) -> Option<RingView> {
        let incoming = self
            .incoming
            .as_ref()
            .filter(|incoming| incoming.answering == Answering::Showing)?;
        let room = directory.room(&incoming.ring.room_id)?;
        let offer = self.offer_for(room);
        Some(RingView {
            serial: incoming.serial,
            room_id: room.id.clone(),
            room_name: room.display_name.clone(),
            room_avatar_mxc: room.avatar_mxc.clone(),
            direct: room.is_direct,
            caller: incoming.ring.caller.clone(),
            video: incoming.ring.is_video(),
            desktop: self.focus == WindowFocus::Unfocused,
            answerable: matches!(offer, JoinOffer::Join | JoinOffer::Start),
            refusal: match offer {
                JoinOffer::Refused(refusal) => Some(refusal),
                JoinOffer::Start | JoinOffer::Join | JoinOffer::InCall => None,
            },
        })
    }

    fn here(&self, directory: &RoomDirectory) -> Option<RoomCallSummary> {
        let room = directory.room(self.selected.as_ref()?)?;
        let call = room.call.as_deref()?;
        let joined_by_you = self
            .own
            .as_ref()
            .is_some_and(|own| call.joined_by(&own.user));
        Some(RoomCallSummary {
            room_id: room.id.clone(),
            people: call.people(),
            video: call.is_video(),
            joined_by_you,
        })
    }
}

fn running_intent(directory: &RoomDirectory, room_id: &RoomId) -> Option<CallIntent> {
    directory
        .room(room_id)
        .and_then(|room| room.call.as_deref())
        .map(|call| call.intent().unwrap_or(CallIntent::Voice))
}

fn keys_sent(report: &KeySendReport) {
    if report.withheld.is_empty() {
        tracing::debug!(sent = report.sent.len(), "sent the call key");
        return;
    }
    let withheld: Vec<String> = report.withheld.iter().map(ToString::to_string).collect();
    tracing::info!(
        sent = report.sent.len(),
        ?withheld,
        "the call key did not reach every device in the call"
    );
}

fn send(events: &EventSender, event: CallEvent) {
    if events.send(AppEvent::Call(event)).is_err() {
        tracing::debug!("the app stopped before hearing from a call task");
    }
}

async fn follow_signals(port: &dyn CallSignalPort, signals_tx: mpsc::Sender<CallSignal>) {
    if let Err(e) = port.follow_signals(signals_tx).await {
        tracing::warn!("failed to follow call signals: {e}");
    }
}

async fn forward_signals(
    events: &EventSender,
    session: u64,
    mut signals_rx: mpsc::Receiver<CallSignal>,
) {
    while let Some(signal) = signals_rx.recv().await {
        if events
            .send(AppEvent::Call(CallEvent::Signal { session, signal }))
            .is_err()
        {
            return;
        }
    }
}
