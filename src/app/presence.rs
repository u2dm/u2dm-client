use std::collections::HashMap;
use std::future::pending;
use std::mem;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{Instant, sleep, timeout};

use super::event::{AppEvent, PresenceEvent};
use super::input::EventSender;
use super::task_group::TaskGroup;
use crate::commands::ui::{UserActivity, WindowAttention, WindowFocus};
use crate::commands::view::{PartnerPresence, PersonPresence, PresenceView};
use crate::domain::presence::{
    Availability, BeaconFailure, LookupFailure, LookupResult, PresenceLine, PresenceReport,
};
use crate::domain::room::RoomId;
use crate::domain::user_info::UserId;
use crate::ports::matrix::PresencePort;
use crate::ports::output::AppOutputPort;

const HEARTBEAT: Duration = Duration::from_secs(20);
const PRESENCE_REFRESH: Duration = Duration::from_mins(1);
const FAREWELL_WAIT: Duration = Duration::from_secs(2);
const UNSTATED_THROTTLE: Duration = Duration::from_secs(10);
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(5);
const MAX_DOUBLINGS: u32 = 8;

#[derive(Clone, PartialEq, Eq)]
struct Shown {
    line: PresenceLine,
    status: Option<String>,
}

pub(super) struct Presence {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    beacon: TaskGroup,
    lookups: TaskGroup,
    lane: Option<watch::Sender<Availability>>,
    farewell: bool,
    attention: WindowAttention,
    disabled: bool,
    watch: u64,
    partner: Option<(RoomId, UserId)>,
    person: Option<UserId>,
    shown: HashMap<UserId, Shown>,
    looked_at: Option<Instant>,
    published: PresenceView,
}

impl Presence {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            beacon: TaskGroup::new("presence-beacon"),
            lookups: TaskGroup::new("presence-lookups"),
            lane: None,
            farewell: false,
            attention: WindowAttention::default(),
            disabled: false,
            watch: 0,
            partner: None,
            person: None,
            shown: HashMap::new(),
            looked_at: None,
            published: PresenceView::default(),
        }
    }

    pub(super) fn begin_session(&mut self, port: Arc<dyn PresencePort>, session: u64, own: UserId) {
        self.disabled = false;
        self.farewell = false;
        let availability = wanted(self.attention.activity);
        tracing::debug!(?availability, "keeping your presence");
        let (lane, wanted_rx) = watch::channel(availability);
        let beacon = Beacon {
            port,
            events: self.events.clone(),
            session,
            own,
        };
        let token = self.beacon.token();
        self.beacon.spawn(async move {
            token
                .run_until_cancelled(keep_announcing(&beacon, wanted_rx))
                .await;
        });
        self.lane = Some(lane);
    }

    pub(super) fn attention_changed(
        &mut self,
        port: Option<Arc<dyn PresencePort>>,
        attention: WindowAttention,
    ) {
        let before = mem::replace(&mut self.attention, attention);
        if before.activity != attention.activity {
            tracing::debug!(activity = ?attention.activity, "your activity changed");
            if let Some(lane) = self.lane.as_ref().filter(|_| !self.farewell) {
                lane.send_replace(wanted(attention.activity));
            }
        }
        if before.focus == attention.focus {
            return;
        }
        match attention.focus {
            WindowFocus::Unfocused => self.lookups.cancel_and_detach(),
            WindowFocus::Focused => {
                let waited = self.looked_at.map_or(PRESENCE_REFRESH, |at| at.elapsed());
                self.spawn_lookups(port, PRESENCE_REFRESH.saturating_sub(waited));
            }
        }
    }

    pub(super) fn follow(
        &mut self,
        port: Option<Arc<dyn PresencePort>>,
        partner: Option<(RoomId, UserId)>,
        person: Option<UserId>,
    ) {
        if self.partner == partner && self.person == person {
            return;
        }
        self.partner = partner;
        self.person = person;
        self.watch = self.watch.wrapping_add(1);
        self.looked_at = None;
        self.lookups.cancel_and_detach();
        let subjects = self.subjects();
        self.shown.retain(|user_id, _| subjects.contains(user_id));
        self.publish();
        if self.attention.focus == WindowFocus::Focused {
            self.spawn_lookups(port, Duration::ZERO);
        }
    }

    pub(super) fn looked(&mut self, watch: u64, user_id: UserId, result: LookupResult) {
        if watch != self.watch || self.disabled {
            tracing::debug!(%user_id, watch, "dropping presence from a superseded lookup");
            return;
        }
        self.looked_at = Some(Instant::now());
        let shown = match result {
            Ok(report) => shown(&report),
            Err(LookupFailure::Refused) => Shown {
                line: PresenceLine::Hidden,
                status: None,
            },
            Err(LookupFailure::Failed) => return,
        };
        self.shown.insert(user_id, shown);
        self.publish();
    }

    pub(super) fn disabled(&mut self) {
        if self.disabled {
            return;
        }
        tracing::debug!("the homeserver keeps no presence; not asking again this session");
        self.disabled = true;
        self.lane = None;
        self.lookups.cancel_and_detach();
        self.shown.clear();
        self.publish();
    }

    pub(super) fn sign_off(&mut self) {
        if self.farewell {
            return;
        }
        if let Some(lane) = &self.lane {
            tracing::debug!("saying offline");
            lane.send_replace(Availability::Offline);
            self.farewell = true;
        }
    }

    pub(super) async fn restart(&mut self) {
        let farewell = mem::take(&mut self.farewell);
        tokio::join!(
            restart_beacon(&mut self.beacon, farewell),
            self.lookups.restart()
        );
        self.lane = None;
        self.disabled = false;
        self.watch = self.watch.wrapping_add(1);
        self.partner = None;
        self.person = None;
        self.shown.clear();
        self.looked_at = None;
        self.published = PresenceView::default();
    }

    pub(super) async fn shutdown(&mut self) {
        let farewell = mem::take(&mut self.farewell);
        tokio::join!(
            shut_beacon(&mut self.beacon, farewell),
            self.lookups.shutdown()
        );
    }

    fn subjects(&self) -> Vec<UserId> {
        let mut subjects: Vec<UserId> = self
            .partner
            .iter()
            .map(|(_, user_id)| user_id.clone())
            .collect();
        if let Some(person) = self
            .person
            .as_ref()
            .filter(|person| !subjects.contains(person))
        {
            subjects.push(person.clone());
        }
        subjects
    }

    fn spawn_lookups(&mut self, port: Option<Arc<dyn PresencePort>>, first_wait: Duration) {
        let Some(port) = port.filter(|_| !self.disabled) else {
            return;
        };
        let subjects = self.subjects();
        if subjects.is_empty() {
            return;
        }
        let watch = self.watch;
        let events = self.events.clone();
        let token = self.lookups.token();
        self.lookups.spawn(async move {
            let looking = keep_looking(port.as_ref(), &events, watch, &subjects, first_wait);
            token.run_until_cancelled(looking).await;
        });
    }

    fn publish(&mut self) {
        let view = self.view();
        if view == self.published {
            return;
        }
        self.published = view.clone();
        self.output
            .publish(Box::new(move |state| state.presence = view));
    }

    fn view(&self) -> PresenceView {
        if self.disabled {
            return PresenceView::default();
        }
        let partner = self.partner.as_ref().and_then(|(room_id, user_id)| {
            let shown = self.shown.get(user_id)?;
            Some(PartnerPresence {
                room_id: room_id.clone(),
                user_id: user_id.clone(),
                line: shown.line,
            })
        });
        let person = self.person.as_ref().and_then(|user_id| {
            let shown = self.shown.get(user_id)?;
            Some(PersonPresence {
                user_id: user_id.clone(),
                line: shown.line,
                status: shown.status.clone(),
            })
        });
        PresenceView { partner, person }
    }
}

const fn wanted(activity: UserActivity) -> Availability {
    match activity {
        UserActivity::Active => Availability::Online,
        UserActivity::Idle => Availability::Unavailable,
    }
}

fn shown(report: &PresenceReport) -> Shown {
    Shown {
        line: report.line(),
        status: report.status_line(),
    }
}

async fn restart_beacon(beacon: &mut TaskGroup, farewell: bool) {
    let_the_farewell_out(beacon, farewell).await;
    beacon.restart().await;
}

async fn shut_beacon(beacon: &mut TaskGroup, farewell: bool) {
    let_the_farewell_out(beacon, farewell).await;
    beacon.shutdown().await;
}

async fn let_the_farewell_out(beacon: &mut TaskGroup, farewell: bool) {
    if farewell && timeout(FAREWELL_WAIT, beacon.drain()).await.is_err() {
        tracing::debug!("the homeserver did not take your offline presence in time");
    }
}

struct Beacon {
    port: Arc<dyn PresencePort>,
    events: EventSender,
    session: u64,
    own: UserId,
}

enum Rest {
    Beat,
    Stop,
}

async fn keep_announcing(beacon: &Beacon, mut wanted: watch::Receiver<Availability>) {
    let mut status: Option<String> = None;
    let mut accepted = false;
    let mut failures: u32 = 0;
    loop {
        let farewell = *wanted.borrow() == Availability::Offline;
        if !farewell && let Some(own) = beacon.read_own(&mut wanted).await {
            if accepted && own.says_nothing() {
                beacon.disable("your own presence reads offline right after an accepted update");
                return;
            }
            status = own.status;
        }
        let availability = *wanted.borrow_and_update();
        let (pause, only_farewell) =
            match beacon.port.announce(availability, status.as_deref()).await {
                Ok(()) => {
                    accepted = true;
                    failures = 0;
                    (HEARTBEAT, false)
                }
                Err(BeaconFailure::Refused) => {
                    beacon.disable("the homeserver refuses presence");
                    return;
                }
                Err(BeaconFailure::Throttled(wait)) => {
                    let wait = wait.unwrap_or(UNSTATED_THROTTLE).min(HEARTBEAT);
                    tracing::debug!(
                        ?wait,
                        "the homeserver asks for a pause between presence updates"
                    );
                    (wait, true)
                }
                Err(BeaconFailure::Failed) => {
                    failures = failures.saturating_add(1);
                    (backoff(failures), false)
                }
            };
        if availability == Availability::Offline {
            return;
        }
        if matches!(rest(&mut wanted, pause, only_farewell).await, Rest::Stop) {
            return;
        }
    }
}

impl Beacon {
    async fn read_own(&self, wanted: &mut watch::Receiver<Availability>) -> Option<PresenceReport> {
        tokio::select! {
            biased;
            () = farewell_wanted(wanted) => None,
            result = self.port.presence_of(&self.own) => result.ok(),
        }
    }

    fn disable(&self, why: &str) {
        tracing::debug!("{why}; your presence stops for this session");
        let disabled = AppEvent::Presence(PresenceEvent::Disabled {
            session: self.session,
        });
        if self.events.send(disabled).is_err() {
            tracing::debug!("the app stopped before hearing that presence is off");
        }
    }
}

async fn farewell_wanted(wanted: &mut watch::Receiver<Availability>) {
    let offline = wanted
        .wait_for(|availability| *availability == Availability::Offline)
        .await
        .is_ok();
    if !offline {
        pending::<()>().await;
    }
}

async fn rest(
    wanted: &mut watch::Receiver<Availability>,
    pause: Duration,
    only_farewell: bool,
) -> Rest {
    let until = sleep(pause);
    tokio::pin!(until);
    loop {
        tokio::select! {
            () = &mut until => return Rest::Beat,
            changed = wanted.changed() => {
                if changed.is_err() {
                    return Rest::Stop;
                }
                let farewell = *wanted.borrow() == Availability::Offline;
                if farewell || !only_farewell {
                    return Rest::Beat;
                }
            }
        }
    }
}

fn backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(MAX_DOUBLINGS);
    RETRY_AFTER_FAILURE
        .saturating_mul(2_u32.saturating_pow(doublings))
        .min(HEARTBEAT)
}

async fn keep_looking(
    port: &dyn PresencePort,
    events: &EventSender,
    watch: u64,
    subjects: &[UserId],
    first_wait: Duration,
) {
    sleep(first_wait).await;
    loop {
        for user_id in subjects {
            let result = port.presence_of(user_id).await;
            let looked = AppEvent::Presence(PresenceEvent::Looked {
                watch,
                user_id: user_id.clone(),
                result,
            });
            if events.send(looked).is_err() {
                return;
            }
        }
        sleep(PRESENCE_REFRESH).await;
    }
}
