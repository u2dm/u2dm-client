use std::sync::Arc;

use tokio::sync::mpsc;

use super::event::{AppEvent, IdentityEvent};
use super::input::EventSender;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{IdentityAlertView, Toast};
use crate::domain::identity::{IdentityAlert, OwnIdentity, RoomAlert, SessionAlert};
use crate::domain::room::RoomId;
use crate::domain::user_info::UserId;
use crate::error::Result;
use crate::ports::matrix::IdentityPort;
use crate::ports::output::AppOutputPort;

const ALERT_CHANNEL_CAP: usize = 8;

pub(super) enum Resolution {
    Accept(UserId),
    Withdraw(UserId),
    AcknowledgeOwnReset,
}

impl Resolution {
    fn resolves(&self, alert: &IdentityAlert) -> bool {
        match (self, alert) {
            (Self::Accept(user_id), IdentityAlert::Room(RoomAlert::IdentityReset(user)))
            | (Self::Withdraw(user_id), IdentityAlert::Room(RoomAlert::NoLongerVerified(user))) => {
                user.user_id == user_id.as_ref()
            }
            (Self::AcknowledgeOwnReset, IdentityAlert::OwnIdentityReset) => true,
            _ => false,
        }
    }

    fn failure(&self, alert: &IdentityAlert) -> UserMessage {
        let person = match alert {
            IdentityAlert::Room(alert) => alert.user().identified(),
            IdentityAlert::OwnIdentityReset | IdentityAlert::Session(_) => String::new(),
        };
        let kind = match self {
            Self::Accept(_) => UserMessageKind::IdentityAcceptFailed,
            Self::Withdraw(_) => UserMessageKind::VerificationWithdrawFailed,
            Self::AcknowledgeOwnReset => UserMessageKind::IdentityResetAcknowledgeFailed,
        };
        UserMessage::about(kind, &person)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Requested(u64),
    AwaitingEcho,
}

struct Pending {
    alert: IdentityAlert,
    phase: Phase,
}

pub(super) struct IdentityAlerts {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    follow: TaskGroup,
    actions: TaskGroup,
    watch: u64,
    requests: u64,
    first_request_of_session: u64,
    room_id: Option<RoomId>,
    room: Vec<RoomAlert>,
    own: OwnIdentity,
    session: Option<SessionAlert>,
    pending: Option<Pending>,
}

impl IdentityAlerts {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            follow: TaskGroup::new("identity-follow"),
            actions: TaskGroup::new("identity-actions"),
            watch: 0,
            requests: 0,
            first_request_of_session: 1,
            room_id: None,
            room: Vec::new(),
            own: OwnIdentity::Unchanged,
            session: None,
            pending: None,
        }
    }

    pub(super) fn follow(&mut self, port: Arc<dyn IdentityPort>, room_id: RoomId) {
        self.follow.cancel_and_detach();
        self.watch = self.watch.wrapping_add(1);
        if self.room_id.as_ref() != Some(&room_id) {
            self.room_id = Some(room_id.clone());
            self.room.clear();
            self.drop_pending_room_alert();
            self.publish();
        }
        tracing::debug!(%room_id, watch = self.watch, "following the room's identity alerts");

        let watch = self.watch;
        let events = self.events.clone();
        let token = self.follow.token();
        self.follow.spawn(async move {
            let (alerts_tx, alerts_rx) = mpsc::channel(ALERT_CHANNEL_CAP);
            let follow = async {
                tokio::join!(
                    follow_room(port.as_ref(), &room_id, alerts_tx),
                    forward_room_alerts(&events, watch, alerts_rx),
                )
            };
            token.run_until_cancelled(follow).await;
        });
    }

    pub(super) fn clear(&mut self) {
        self.follow.cancel_and_detach();
        self.watch = self.watch.wrapping_add(1);
        self.room_id = None;
        self.room.clear();
        self.drop_pending_room_alert();
        self.publish();
    }

    pub(super) fn spawn_own_listener(
        &mut self,
        group: &mut TaskGroup,
        port: Arc<dyn IdentityPort>,
        session: u64,
    ) {
        self.own = OwnIdentity::Unchanged;
        let events = self.events.clone();
        let token = group.token();
        group.spawn(async move {
            let (own_tx, own_rx) = mpsc::channel(ALERT_CHANNEL_CAP);
            let follow = async {
                tokio::join!(
                    follow_own(port.as_ref(), own_tx),
                    forward_own_identity(&events, session, own_rx),
                )
            };
            token.run_until_cancelled(follow).await;
        });
    }

    pub(super) fn room_alerts_changed(&mut self, watch: u64, alerts: Vec<RoomAlert>) {
        if watch != self.watch {
            tracing::debug!(watch, "dropping identity alerts from a superseded follow");
            return;
        }
        tracing::debug!(alerts = alerts.len(), "the room's identity alerts changed");
        self.room = alerts;
        self.settle_echoed();
        self.publish();
    }

    pub(super) fn own_identity_changed(&mut self, own: OwnIdentity) {
        if own == OwnIdentity::Reset {
            tracing::warn!("the account's identity was reset; alerting");
        }
        self.own = own;
        self.settle_echoed();
        self.publish();
    }

    pub(super) fn session_alert_changed(&mut self, session: Option<SessionAlert>) {
        if self.session == session {
            return;
        }
        tracing::debug!(?session, "this session's own alert changed");
        self.session = session;
        self.settle_echoed();
        self.publish();
    }

    pub(super) fn resolve(&mut self, port: Arc<dyn IdentityPort>, resolution: Resolution) {
        let Some(alert) = self.raised(&resolution) else {
            tracing::debug!("ignoring a resolution for an identity alert that is not raised");
            return;
        };
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.alert.concerns(&alert))
        {
            tracing::debug!("the identity alert is already being resolved");
            return;
        }
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        let failure = resolution.failure(&alert);
        self.pending = Some(Pending {
            alert,
            phase: Phase::Requested(request),
        });
        self.publish();

        let events = self.events.clone();
        self.actions.spawn(async move {
            let failure = match resolve(port.as_ref(), &resolution).await {
                Ok(()) => None,
                Err(e) => {
                    tracing::warn!("failed to resolve an identity alert: {e}");
                    Some(failure)
                }
            };
            drop(events.send(AppEvent::Identity(IdentityEvent::Settled {
                request,
                failure,
            })));
        });
    }

    pub(super) fn settled(&mut self, request: u64, failure: Option<UserMessage>) {
        if request < self.first_request_of_session {
            tracing::debug!(
                request,
                "dropping an identity resolution of a session that ended"
            );
            return;
        }
        let current = self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.phase == Phase::Requested(request));
        if let Some(failure) = failure {
            show_toast(self.output.as_ref(), Toast::Error(failure));
            if current {
                self.pending = None;
            }
        } else {
            if current && let Some(pending) = self.pending.as_mut() {
                pending.phase = Phase::AwaitingEcho;
            }
            self.settle_echoed();
        }
        self.publish();
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.follow.restart(), self.actions.restart());
        self.first_request_of_session = self.requests.wrapping_add(1);
        self.watch = self.watch.wrapping_add(1);
        self.room_id = None;
        self.room.clear();
        self.own = OwnIdentity::Unchanged;
        self.session = None;
        self.pending = None;
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.follow.shutdown(), self.actions.shutdown());
    }

    fn raised(&self, resolution: &Resolution) -> Option<IdentityAlert> {
        let own = (self.own == OwnIdentity::Reset).then_some(IdentityAlert::OwnIdentityReset);
        own.into_iter()
            .chain(self.session.map(IdentityAlert::Session))
            .chain(self.room.iter().cloned().map(IdentityAlert::Room))
            .find(|alert| resolution.resolves(alert))
    }

    fn settle_echoed(&mut self) {
        let echoed = self.pending.as_ref().is_some_and(|pending| {
            pending.phase == Phase::AwaitingEcho
                && !pending
                    .alert
                    .still_raised(self.own, self.session, &self.room)
        });
        if echoed {
            self.pending = None;
        }
    }

    fn drop_pending_room_alert(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| matches!(pending.alert, IdentityAlert::Room(_)))
        {
            self.pending = None;
        }
    }

    fn publish(&self) {
        let alert = IdentityAlert::most_urgent(self.own, self.session, &self.room);
        let busy = alert.as_ref().is_some_and(|alert| {
            self.pending
                .as_ref()
                .is_some_and(|pending| pending.alert.concerns(alert))
        });
        let view = IdentityAlertView { alert, busy };
        self.output
            .publish(Box::new(move |state| state.identity_alert = view));
    }
}

async fn resolve(port: &dyn IdentityPort, resolution: &Resolution) -> Result<()> {
    match resolution {
        Resolution::Accept(user_id) => port.pin_identity(user_id).await,
        Resolution::Withdraw(user_id) => port.withdraw_verification(user_id).await,
        Resolution::AcknowledgeOwnReset => port.acknowledge_own_reset().await,
    }
}

async fn follow_room(
    port: &dyn IdentityPort,
    room_id: &RoomId,
    alerts_tx: mpsc::Sender<Vec<RoomAlert>>,
) {
    if let Err(e) = port.follow_room(room_id, alerts_tx).await {
        tracing::warn!(%room_id, "failed to follow the room's identity alerts: {e}");
    }
}

async fn forward_room_alerts(
    events: &EventSender,
    watch: u64,
    mut alerts_rx: mpsc::Receiver<Vec<RoomAlert>>,
) {
    while let Some(alerts) = alerts_rx.recv().await {
        let event = AppEvent::Identity(IdentityEvent::RoomAlerts { watch, alerts });
        if events.send(event).is_err() {
            return;
        }
    }
}

async fn follow_own(port: &dyn IdentityPort, own_tx: mpsc::Sender<OwnIdentity>) {
    if let Err(e) = port.follow_own(own_tx).await {
        tracing::warn!("failed to follow the account's own identity: {e}");
    }
}

async fn forward_own_identity(
    events: &EventSender,
    session: u64,
    mut own_rx: mpsc::Receiver<OwnIdentity>,
) {
    while let Some(own) = own_rx.recv().await {
        let event = AppEvent::Identity(IdentityEvent::OwnIdentity { session, own });
        if events.send(event).is_err() {
            return;
        }
    }
}
