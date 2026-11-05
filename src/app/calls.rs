pub(super) mod device_test;
mod devices;
mod in_call;
mod joined;
mod joining;
pub(super) mod lane;
mod media;
mod offer;
mod people;
mod preparing;
pub(super) mod preview;
mod ringing;
mod sheet;
mod stage;

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::Span;

use self::device_test::Testing;
use self::joined::Joined;
use self::lane::LEAVE_WAIT;
use self::media::MEDIA_TURN_SHARES;
use self::offer::Transports;
use self::preview::Preview;
use self::sheet::Sheet;
use super::event::{AppEvent, CallEvent};
use super::input::EventSender;
use super::room_directory::RoomDirectory;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::WindowFocus;
use crate::commands::view::{CallFailure, CallView, JoinRefusal, Toast};
use crate::domain::call::devices::{DeviceChoices, DeviceLists};
use crate::domain::call::media::MediaChoice;
use crate::domain::call::ring::{CallSignal, OUTGOING_RING_LIFETIME_MS, Ring};
use crate::domain::call::{CallIntent, DeviceId, MemberKey};
use crate::domain::room::{Room, RoomId, UnreadFlags};
use crate::domain::user_info::UserId;
use crate::ports::call::CallMediaPort;
use crate::ports::matrix::{CallSignalPort, RtcMembershipPort};
use crate::ports::output::AppOutputPort;

const SIGNAL_CHANNEL_CAP: usize = 16;

const RING_CONFIRM_WAIT: Duration = Duration::from_secs(3);

const RINGS_REMEMBERED: usize = 16;

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
    devices: Option<DeviceChoices>,
    choosing: bool,
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
    media_turn: Arc<Semaphore>,
    key_tasks: TaskGroup,
    own: Option<Own>,
    focus: WindowFocus,
    incoming: Option<Incoming>,
    rung: VecDeque<Ring>,
    serial: u64,
    selected: Option<RoomId>,
    verified: bool,
    transports: Transports,
    discovering: bool,
    pending_join: Option<JoinRequest>,
    joined: Option<Joined>,
    stage_wake: Option<Duration>,
    reaction_wake: Option<Duration>,
    faces_asked: HashSet<String>,
    faces_ready: u64,
    failed: Option<Failed>,
    sheet: Option<Sheet>,
    sheet_serial: i32,
    preview: Option<Preview>,
    previews: u64,
    preview_tasks: TaskGroup,
    testing: Option<Testing>,
    tests: u64,
    test_tasks: TaskGroup,
    devices: DeviceChoices,
    listed: Option<DeviceLists>,
    listings: u64,
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
            media_turn: Arc::new(Semaphore::new(MEDIA_TURN_SHARES as usize)),
            key_tasks: TaskGroup::new("call-keys"),
            own: None,
            focus: WindowFocus::default(),
            incoming: None,
            rung: VecDeque::new(),
            serial: 0,
            selected: None,
            verified: false,
            transports: Transports::Unknown,
            discovering: false,
            pending_join: None,
            joined: None,
            stage_wake: None,
            reaction_wake: None,
            faces_asked: HashSet::new(),
            faces_ready: 0,
            failed: None,
            sheet: None,
            sheet_serial: 0,
            preview: None,
            previews: 0,
            preview_tasks: TaskGroup::new("call-preview"),
            testing: None,
            tests: 0,
            test_tasks: TaskGroup::new("call-device-test"),
            devices: DeviceChoices::default(),
            listed: None,
            listings: 0,
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
        let follower = Arc::clone(&port);
        background.spawn_follow(
            &self.events,
            SIGNAL_CHANNEL_CAP,
            move |signals_tx| async move { follow_signals(follower.as_ref(), signals_tx).await },
            move |signal| AppEvent::Call(CallEvent::Signal { session, signal }),
        );
        self.own = Some(Own {
            port,
            rtc,
            session,
            user,
            device,
        });
        self.discover();
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

    fn toast_about(&self, kind: UserMessageKind, room_id: &RoomId, directory: &RoomDirectory) {
        let name = directory
            .room(room_id)
            .map_or_else(|| room_id.to_string(), |room| room.display_name.clone());
        show_toast(
            self.output.as_ref(),
            Toast::Error(UserMessage::about(kind, &name)),
        );
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
        if self.selected.as_ref() != room_id {
            self.close_sheet_elsewhere(room_id);
        }
        self.selected = room_id.cloned();
        self.publish(directory);
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
            self.preview_tasks.restart(),
            self.test_tasks.restart(),
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
            self.preview_tasks.shutdown(),
            self.test_tasks.shutdown(),
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
        self.stage_wake = None;
        self.reaction_wake = None;
        self.faces_asked.clear();
        self.failed = None;
        self.sheet = None;
        self.preview = None;
        self.testing = None;
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
        if self.rung.len() == RINGS_REMEMBERED {
            self.rung.pop_front();
        }
        self.rung.push_back(incoming.ring);
    }

    fn publish(&mut self, directory: &RoomDirectory) {
        if let Some(wake) = self.joined.as_mut().and_then(Joined::settle_stage) {
            self.wake_stage(wake);
        }
        if let Some(wake) = self.joined.as_ref().and_then(Joined::settle_reactions) {
            self.wake_reactions(wake);
        }
        self.follow_preview(directory);
        self.follow_test(directory);
        let mut active = self.joined.as_ref().map(Joined::view);
        if let Some(active) = active.as_mut() {
            active.faces_ready = self.faces_ready;
            active.room_unread = unread_of(directory.room(&active.room_id));
        }
        let here = self.here(directory);
        self.fetch_faces(active.as_ref(), here.as_ref());
        let view = CallView {
            ring: self.ring_view(directory),
            here,
            offer: self.offer_view(directory),
            sheet: self.sheet_view(directory),
            active,
            failed: self.failed_view(directory),
        };
        if view == self.published {
            return;
        }
        self.published = view.clone();
        self.output
            .publish(Box::new(move |state| state.call = view));
    }
}

fn unread_of(room: Option<&Room>) -> UnreadFlags {
    let mut flags = UnreadFlags::default();
    if let Some(room) = room {
        flags.absorb(room);
    }
    flags
}

async fn take_turn(
    turn: &Arc<Semaphore>,
    stopped: &mut oneshot::Receiver<()>,
    token: &CancellationToken,
    what: &str,
) -> Option<OwnedSemaphorePermit> {
    tokio::select! {
        biased;
        _ = stopped => None,
        () = token.cancelled() => None,
        turn = Arc::clone(turn).acquire_owned() => turn
            .inspect_err(|e| tracing::debug!("the {what} cannot take its turn: {e}"))
            .ok(),
    }
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
