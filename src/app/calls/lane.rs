use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, watch};
use tokio::time::{Instant, sleep, sleep_until};

use crate::app::event::{AppEvent, CallEvent};
use crate::app::input::EventSender;
use crate::domain::call::{
    CallIntent, DelayedLeave, OwnMembership, RestartFailure, ScheduleFailure, SentMembership,
};
use crate::domain::room::RoomId;
use crate::ports::matrix::RtcMembershipPort;

pub(super) const LEAVE_DELAY: Duration = Duration::from_secs(18);
pub(super) const LEAVE_WAIT: Duration = Duration::from_secs(2);
pub(super) const MEMBERSHIP_LIFETIME: Duration = Duration::from_hours(4);
const RESTART_EVERY: Duration = Duration::from_secs(4);
const RETRY_AFTER: Duration = Duration::from_secs(1);
const JOIN_ATTEMPTS: u32 = 5;
const REFRESH_LEAD: Duration = Duration::from_secs(5);
const LONGEST_REFRESH: Duration = Duration::from_hours(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum LaneEvent {
    Joined(SentMembership),
    Failed(String),
}

pub(super) struct Lane {
    pub(super) port: Arc<dyn RtcMembershipPort>,
    pub(super) room_id: RoomId,
    pub(super) membership: OwnMembership,
    pub(super) intent: watch::Receiver<CallIntent>,
    pub(super) leave_delay: Duration,
    pub(super) events: EventSender,
    pub(super) call: u64,
}

enum Ended {
    Ordered,
    Failed,
}

pub(super) async fn keep_membership(mut lane: Lane, mut leave_rx: oneshot::Receiver<()>) {
    let mut leave = lane.schedule_leave().await;
    match lane.join(&mut leave_rx).await {
        Ok(sent) => lane.report(LaneEvent::Joined(sent)),
        Err(Ended::Ordered) => {
            lane.farewell(leave.as_ref()).await;
            return;
        }
        Err(Ended::Failed) => {
            lane.report(LaneEvent::Failed(
                "the homeserver did not take the call membership".into(),
            ));
            lane.farewell(leave.as_ref()).await;
            return;
        }
    }
    let mut last_good = Instant::now();
    let mut refresh_at = Instant::now() + refresh_after(&lane.membership);
    let mut intent_told = true;
    loop {
        tokio::select! {
            _ = &mut leave_rx => {
                lane.farewell(leave.as_ref()).await;
                return;
            }
            () = sleep(RESTART_EVERY), if leave.is_some() => {
                if let Some(current) = leave.take() {
                    leave = lane.restart(current, &mut last_good).await;
                }
            }
            () = sleep_until(refresh_at) => {
                lane.refresh().await;
                refresh_at = Instant::now() + refresh_after(&lane.membership);
            }
            changed = lane.intent.changed(), if intent_told => {
                if changed.is_ok() {
                    lane.retell_intent().await;
                } else {
                    intent_told = false;
                }
            }
        }
    }
}

fn refresh_after(membership: &OwnMembership) -> Duration {
    Duration::from_millis(membership.lifetime_ms)
        .min(LONGEST_REFRESH)
        .saturating_sub(REFRESH_LEAD)
}

impl Lane {
    fn report(&self, event: LaneEvent) {
        let event = AppEvent::Call(CallEvent::Lane {
            call: self.call,
            event,
        });
        if self.events.send(event).is_err() {
            tracing::debug!("the app stopped before hearing from the call membership lane");
        }
    }

    async fn schedule_leave(&mut self) -> Option<DelayedLeave> {
        let mut delay = LEAVE_DELAY;
        loop {
            match self.port.schedule_leave(&self.room_id, delay).await {
                Ok(leave) => {
                    tracing::debug!(room_id = %self.room_id, delay_ms = delay.as_millis(), "scheduled the call's delayed leave");
                    self.leave_delay = delay;
                    return Some(leave);
                }
                Err(ScheduleFailure::TooLong {
                    max_ms: Some(max_ms),
                }) if Duration::from_millis(max_ms) < delay && max_ms > 0 => {
                    tracing::info!(room_id = %self.room_id, max_ms, "the homeserver caps delayed events, scheduling a shorter leave");
                    delay = Duration::from_millis(max_ms);
                }
                Err(failure) => {
                    tracing::info!(room_id = %self.room_id, ?failure, "no delayed leave for this call, a crash leaves the membership until it expires");
                    return None;
                }
            }
        }
    }

    async fn join(
        &mut self,
        leave_rx: &mut oneshot::Receiver<()>,
    ) -> Result<SentMembership, Ended> {
        for attempt in 1..=JOIN_ATTEMPTS {
            self.take_intent();
            let sent = tokio::select! {
                _ = &mut *leave_rx => return Err(Ended::Ordered),
                sent = self.port.send_membership(&self.room_id, &self.membership) => sent,
            };
            match sent {
                Ok(sent) => {
                    tracing::info!(room_id = %self.room_id, attempt, "sent the call membership");
                    self.membership.created_ms = Some(sent.created_ms);
                    return Ok(sent);
                }
                Err(e) => {
                    tracing::warn!(room_id = %self.room_id, attempt, "the call membership was not sent: {e}");
                    if attempt < JOIN_ATTEMPTS {
                        sleep(RETRY_AFTER).await;
                    }
                }
            }
        }
        Err(Ended::Failed)
    }

    async fn restart(
        &mut self,
        leave: DelayedLeave,
        last_good: &mut Instant,
    ) -> Option<DelayedLeave> {
        match self.port.restart_leave(&leave).await {
            Ok(()) => {
                *last_good = Instant::now();
                Some(leave)
            }
            Err(failure) => {
                let lapsed = last_good.elapsed() >= self.leave_delay;
                tracing::info!(room_id = %self.room_id, ?failure, lapsed, "the call's delayed leave did not restart");
                match (failure, lapsed) {
                    (_, true) => self.rejoin(last_good).await,
                    (RestartFailure::Gone, false) => {
                        let leave = self.schedule_leave().await;
                        *last_good = Instant::now();
                        leave
                    }
                    (RestartFailure::Failed(_), false) => Some(leave),
                }
            }
        }
    }

    async fn rejoin(&mut self, last_good: &mut Instant) -> Option<DelayedLeave> {
        let leave = self.schedule_leave().await;
        self.take_intent();
        match self
            .port
            .send_membership(&self.room_id, &self.membership)
            .await
        {
            Ok(sent) => {
                tracing::info!(room_id = %self.room_id, "rejoined the call after its delayed leave lapsed");
                self.report(LaneEvent::Joined(sent));
            }
            Err(e) => tracing::warn!(room_id = %self.room_id, "could not rejoin the call: {e}"),
        }
        *last_good = Instant::now();
        leave
    }

    async fn refresh(&mut self) {
        self.take_intent();
        match self
            .port
            .send_membership(&self.room_id, &self.membership)
            .await
        {
            Ok(_) => tracing::debug!(room_id = %self.room_id, "refreshed the call membership"),
            Err(e) => {
                tracing::warn!(room_id = %self.room_id, "could not refresh the call membership: {e}");
            }
        }
    }

    fn take_intent(&mut self) {
        self.membership.intent = *self.intent.borrow_and_update();
    }

    async fn retell_intent(&mut self) {
        let told = self.membership.intent;
        self.take_intent();
        let intent = self.membership.intent;
        if intent == told {
            return;
        }
        match self
            .port
            .send_membership(&self.room_id, &self.membership)
            .await
        {
            Ok(_) => {
                tracing::info!(room_id = %self.room_id, ?intent, "the call membership says the new intent");
            }
            Err(e) => {
                tracing::warn!(room_id = %self.room_id, ?intent, "could not update the call membership's intent: {e}");
            }
        }
    }

    async fn farewell(&self, leave: Option<&DelayedLeave>) {
        if let Some(leave) = leave
            && self.send_leave(leave).await
        {
            return;
        }
        self.clear_membership().await;
    }

    async fn send_leave(&self, leave: &DelayedLeave) -> bool {
        let sent = self.port.send_leave(leave).await;
        match &sent {
            Ok(()) => {
                tracing::info!(room_id = %self.room_id, "left the call through its delayed leave");
            }
            Err(e) => {
                tracing::info!(room_id = %self.room_id, "the delayed leave was not sent: {e}");
            }
        }
        sent.is_ok()
    }

    async fn clear_membership(&self) {
        let cleared = self.port.clear_membership(&self.room_id).await;
        match cleared {
            Ok(()) => tracing::info!(room_id = %self.room_id, "left the call"),
            Err(e) => tracing::warn!(room_id = %self.room_id, "could not leave the call: {e}"),
        }
    }
}
