use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use super::data;
use crate::domain::call::identity::{MediaIdentity, spellings};
use crate::domain::call::keys::{KeyIndex, MediaKey, ReceivedKey};
use crate::domain::call::media::MediaEvent;
use crate::domain::call::ring::{CallSignal, Ring, RingId, RingKind};
use crate::domain::call::sfu::SfuId;
use crate::domain::call::{
    CallIntent, CallMember, DeviceId, FocusSelection, MemberKey, RoomCall, Transport,
};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::room::{Room, RoomId};
use crate::domain::user_info::UserId;
use crate::util::unix_now_ms;

const ENV_VAR: &str = "U2DM_DEMO_CALLS";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "starts MatrixRTC calls in demo rooms and rings you",
    combinable: true,
    flags: &[
        Flag {
            value: "active",
            effect: "a video call runs in Design Team (Nadia, Jordan) and a voice call in Rust Programming (Marcus, Kai)",
            note: "",
        },
        Flag {
            value: "ring",
            effect: "Sarah joins a voice call in her chat 3s after sync and rings you",
            note: "her membership lands with the ring, so the card shows at once",
        },
        Flag {
            value: "ring-video",
            effect: "with ring: the call and the ring are video",
            note: "",
        },
        Flag {
            value: "ring-unconfirmed",
            effect: "with ring: Sarah's membership never shows, so nothing rings",
            note: "the ring is dropped after its 3s window",
        },
        Flag {
            value: "ring-ends",
            effect: "with ring: Sarah leaves the call 8s after ringing",
            note: "",
        },
        Flag {
            value: "declined-elsewhere",
            effect: "with ring: your other device declines 5s after the ring",
            note: "",
        },
        Flag {
            value: "answered-elsewhere",
            effect: "with ring: your other device joins the call 5s after the ring",
            note: "",
        },
        Flag {
            value: "group-call",
            effect: "Kai starts a call in Matrix HQ 3s after sync and notifies the room",
            note: "a notification, not a ring: nothing rings",
        },
        Flag {
            value: "expiring",
            effect: "with active: Kai's membership in Rust Programming expires 20s after sync, as a crashed client's does",
            note: "",
        },
        Flag {
            value: "no-permission",
            effect: "joining calls in Rust Programming needs a power level you lack",
            note: "",
        },
        Flag {
            value: "decline-fails",
            effect: "declining a call fails",
            note: "",
        },
        Flag {
            value: "slow",
            effect: "declining, joining and token requests take ~1.5s",
            note: "",
        },
        Flag {
            value: "no-sfu",
            effect: "the homeserver names no call server, so calls cannot be joined",
            note: "",
        },
        Flag {
            value: "delayed-unsupported",
            effect: "the homeserver has no delayed events: the join goes on without a delayed leave",
            note: "",
        },
        Flag {
            value: "grant-fails",
            effect: "the call server refuses the token request with 403",
            note: "",
        },
        Flag {
            value: "connect-fails",
            effect: "the media server refuses the connection",
            note: "",
        },
        Flag {
            value: "reconnecting",
            effect: "5s after connecting the media server reconnects for 3s",
            note: "",
        },
        Flag {
            value: "keys-late",
            effect: "the other members' keys arrive 4s after you join",
            note: "their tiles wait for keys until then",
        },
        Flag {
            value: "undecryptable",
            effect: "Jordan's device is not cross-signed and never sends a key that works",
            note: "",
        },
        Flag {
            value: "answers",
            effect: "Sarah joins a call you start in her chat 3s after it rings",
            note: "",
        },
        Flag {
            value: "declines",
            effect: "Sarah declines a call you start in her chat 3s after it rings",
            note: "",
        },
        Flag {
            value: "speaking",
            effect: "the people in your call take turns speaking every 2s",
            note: "",
        },
        Flag {
            value: "crowd",
            effect: "with active: eight people are in the Design Team call",
            note: "",
        },
        Flag {
            value: "remote-sfu",
            effect: "with active: Jordan publishes to another homeserver's media server",
            note: "",
        },
    ],
    notes: &[
        "each decline logs `demo: the call is declined`",
        "call facets show in /state at .view.directory.rooms[].call",
        "joining logs `demo: you joined the call`, leaving `demo: you left the call`",
        "the media is fake: it never opens a microphone or speaker",
    ],
};

pub const RING_AFTER: Duration = Duration::from_secs(3);
const ELSEWHERE_AFTER: Duration = Duration::from_secs(5);
const RING_ENDS_AFTER: Duration = Duration::from_secs(8);
const EXPIRES_AFTER: Duration = Duration::from_secs(20);
const EXPIRY_LAG: Duration = Duration::from_secs(1);
const SLOW_ANSWER: Duration = Duration::from_millis(1500);
const RING_LIFETIME_MS: u64 = 90_000;
const MEMBERSHIP_LIFETIME_MS: u64 = 14_400_000;
const DESIGN: &str = "!design:demo.local";
const RUST: &str = "!rust:demo.local";
const SARAH_CHAT: &str = "!sarah:demo.local";
const MATRIX_HQ: &str = "!matrixhq:demo.local";
const SARAH: &str = "@sarah:matrix.org";
const NADIA: &str = "@nadia:matrix.org";
const JORDAN: &str = "@jordan:matrix.org";
const MARCUS: &str = "@marcus:matrix.org";
const KAI: &str = "@kai:matrix.org";
const LEO: &str = "@leo:matrix.org";
const NINA: &str = "@nina:matrix.org";
const PRIYA: &str = "@priya:matrix.org";
const VERA: &str = "@vera:matrix.org";
const MEMBER_DEVICE: &str = "DEMOCALLDEVICE";
const OTHER_OWN_DEVICE: &str = "DEMOOTHERDEVICE";
const DEMO_SFU: &str = "https://sfu.demo.local/livekit/jwt";
const REMOTE_SFU: &str = "https://sfu.remote.demo/livekit/jwt";
pub const DEMO_LIVEKIT: &str = "wss://livekit.demo.local";
pub const ANSWER_AFTER: Duration = Duration::from_secs(3);
pub const KEYS_LATE_BY: Duration = Duration::from_secs(4);
pub const RECONNECT_AFTER: Duration = Duration::from_secs(5);
pub const RECONNECT_FOR: Duration = Duration::from_secs(3);
pub const SPEAKING_TURN: Duration = Duration::from_secs(2);
pub const UNTRUSTED: &str = JORDAN;
const DEMO_KEY: [u8; 16] = [7; 16];

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    active: bool,
    ring: bool,
    ring_video: bool,
    ring_unconfirmed: bool,
    ring_ends: bool,
    declined_elsewhere: bool,
    answered_elsewhere: bool,
    group_call: bool,
    expiring: bool,
    no_permission: bool,
    pub decline_fails: bool,
    pub slow: bool,
    pub no_sfu: bool,
    pub delayed_unsupported: bool,
    pub grant_fails: bool,
    pub connect_fails: bool,
    pub reconnecting: bool,
    pub keys_late: bool,
    pub undecryptable: bool,
    pub answers: bool,
    pub declines: bool,
    pub speaking: bool,
    crowd: bool,
    remote_sfu: bool,
}

pub fn scenario() -> Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    *SCENARIO.get_or_init(from_env)
}

fn from_env() -> Scenario {
    let Ok(raw) = env::var(ENV_VAR) else {
        return Scenario::default();
    };
    let mut scenario = Scenario::default();
    for flag in raw.split(',').map(str::trim) {
        apply(&mut scenario, flag);
    }
    tracing::info!(flags = raw, "demo mode: shaping calls");
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "active" => scenario.active = true,
        "ring" => scenario.ring = true,
        "ring-video" => scenario.ring_video = true,
        "ring-unconfirmed" => scenario.ring_unconfirmed = true,
        "ring-ends" => scenario.ring_ends = true,
        "declined-elsewhere" => scenario.declined_elsewhere = true,
        "answered-elsewhere" => scenario.answered_elsewhere = true,
        "group-call" => scenario.group_call = true,
        "expiring" => scenario.expiring = true,
        "no-permission" => scenario.no_permission = true,
        "decline-fails" => scenario.decline_fails = true,
        "slow" => scenario.slow = true,
        "no-sfu" => scenario.no_sfu = true,
        "delayed-unsupported" => scenario.delayed_unsupported = true,
        "grant-fails" => scenario.grant_fails = true,
        "connect-fails" => scenario.connect_fails = true,
        "reconnecting" => scenario.reconnecting = true,
        "keys-late" => scenario.keys_late = true,
        "undecryptable" => scenario.undecryptable = true,
        "answers" => scenario.answers = true,
        "declines" => scenario.declines = true,
        "speaking" => scenario.speaking = true,
        "crowd" => scenario.crowd = true,
        "remote-sfu" => scenario.remote_sfu = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

#[derive(Clone, Copy)]
struct Joiner {
    user: &'static str,
    device: &'static str,
    intent: CallIntent,
    expires_after: Option<Duration>,
    sfu: &'static str,
}

impl Joiner {
    const fn member(user: &'static str, intent: CallIntent) -> Self {
        Self {
            user,
            device: MEMBER_DEVICE,
            intent,
            expires_after: None,
            sfu: DEMO_SFU,
        }
    }

    fn identity(self) -> MediaIdentity {
        let member_id = format!("{}:{}", self.user, self.device);
        let [legacy, _, _] = spellings(self.user, self.device, &member_id);
        legacy
    }

    fn key(self) -> MemberKey {
        MemberKey::new(self.user, DeviceId::new(self.device))
    }

    fn to_member(self, started_ms: u64, offset: u64) -> CallMember {
        let created_ms = started_ms.saturating_add(offset);
        let expires_ms = self
            .expires_after
            .map_or(created_ms.saturating_add(MEMBERSHIP_LIFETIME_MS), |after| {
                started_ms.saturating_add(duration_ms(after))
            });
        CallMember {
            user: NamedUser::new(
                self.user,
                MemberName::new(data::person_name(self.user).as_deref(), false),
            ),
            device: DeviceId::new(self.device),
            member_id: format!("{}:{}", self.user, self.device),
            created_ms,
            expires_ms,
            intent: Some(self.intent),
            transport: Some(Transport {
                service_url: Some(self.sfu.to_owned()),
                sfu_url: None,
            }),
            selection: FocusSelection::MultiSfu,
        }
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Default)]
struct Calls {
    started_ms: u64,
    rooms: HashMap<String, Vec<Joiner>>,
}

struct Watcher {
    room_id: String,
    sfu: String,
    events_tx: mpsc::UnboundedSender<MediaEvent>,
}

#[derive(Default)]
pub struct CallBook {
    calls: Mutex<Calls>,
    signals: Mutex<Option<mpsc::Sender<CallSignal>>>,
    watchers: Mutex<Vec<Watcher>>,
}

pub fn own_transport() -> Transport {
    Transport {
        service_url: Some(DEMO_SFU.to_owned()),
        sfu_url: None,
    }
}

pub fn book() -> &'static CallBook {
    static BOOK: OnceLock<CallBook> = OnceLock::new();
    BOOK.get_or_init(CallBook::default)
}

pub fn shaped(mut room: Room) -> Room {
    book().shape(&mut room);
    room
}

impl CallBook {
    pub fn begin(&self) {
        let shaped = scenario();
        let Ok(mut calls) = self.calls.lock() else {
            return;
        };
        calls.started_ms = unix_now_ms();
        calls.rooms.clear();
        if shaped.active {
            let kai = Joiner {
                expires_after: shaped.expiring.then_some(EXPIRES_AFTER),
                ..Joiner::member(KAI, CallIntent::Voice)
            };
            let jordan = Joiner {
                sfu: if shaped.remote_sfu {
                    REMOTE_SFU
                } else {
                    DEMO_SFU
                },
                ..Joiner::member(JORDAN, CallIntent::Video)
            };
            let mut design = vec![Joiner::member(NADIA, CallIntent::Video), jordan];
            if shaped.crowd {
                design.extend(
                    [LEO, NINA, PRIYA, VERA, MARCUS, KAI]
                        .map(|user| Joiner::member(user, CallIntent::Video)),
                );
            }
            calls.rooms.insert(DESIGN.to_owned(), design);
            calls.rooms.insert(
                RUST.to_owned(),
                vec![Joiner::member(MARCUS, CallIntent::Voice), kai],
            );
        }
    }

    fn shape(&self, room: &mut Room) {
        if scenario().no_permission && room.id.as_ref() == RUST {
            room.may_join_calls = false;
        }
        let Ok(calls) = self.calls.lock() else {
            return;
        };
        let now = unix_now_ms();
        let members: Vec<CallMember> = calls
            .rooms
            .get(room.id.as_ref())
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(offset, joiner)| {
                joiner.to_member(calls.started_ms, u64::try_from(offset).unwrap_or(0))
            })
            .filter(|member| member.expires_ms > now)
            .collect();
        room.call = RoomCall::gather(members).map(Arc::new);
    }

    pub fn follow(&self, signals_tx: mpsc::Sender<CallSignal>) {
        if let Ok(mut signals) = self.signals.lock() {
            *signals = Some(signals_tx);
        }
    }

    fn join(&self, room_id: &str, joiner: Joiner) {
        if let Ok(mut calls) = self.calls.lock() {
            calls
                .rooms
                .entry(room_id.to_owned())
                .or_default()
                .push(joiner);
        }
        self.tell_watchers(room_id, joiner, true);
    }

    fn leave(&self, room_id: &str, user: &str) {
        let mut left = Vec::new();
        if let Ok(mut calls) = self.calls.lock()
            && let Some(joiners) = calls.rooms.get_mut(room_id)
        {
            left = joiners
                .iter()
                .filter(|joiner| joiner.user == user)
                .copied()
                .collect();
            joiners.retain(|joiner| joiner.user != user);
        }
        for joiner in left {
            self.tell_watchers(room_id, joiner, false);
        }
    }

    pub fn own_join(&self, room_id: &str, intent: CallIntent) {
        self.leave_own(room_id);
        let own = Joiner {
            device: own_device(),
            ..Joiner::member(data::own_user(), intent)
        };
        self.join(room_id, own);
    }

    pub fn leave_own(&self, room_id: &str) {
        if let Ok(mut calls) = self.calls.lock()
            && let Some(joiners) = calls.rooms.get_mut(room_id)
        {
            joiners.retain(|joiner| {
                !(joiner.user == data::own_user() && joiner.device == own_device())
            });
        }
    }

    pub fn watch(&self, room_id: &str, sfu: &str, events_tx: mpsc::UnboundedSender<MediaEvent>) {
        let present: Vec<Joiner> = self
            .calls
            .lock()
            .map(|calls| {
                calls
                    .rooms
                    .get(room_id)
                    .into_iter()
                    .flatten()
                    .filter(|joiner| joiner.sfu == sfu && joiner.user != data::own_user())
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        if let Some(id) = sfu_id(sfu) {
            for joiner in present {
                send_media(
                    &events_tx,
                    MediaEvent::Present {
                        sfu: id.clone(),
                        identity: joiner.identity(),
                        present: true,
                    },
                );
            }
        }
        if let Ok(mut watchers) = self.watchers.lock() {
            watchers.retain(|watcher| !watcher.events_tx.is_closed());
            watchers.push(Watcher {
                room_id: room_id.to_owned(),
                sfu: sfu.to_owned(),
                events_tx,
            });
        }
    }

    fn tell_watchers(&self, room_id: &str, joiner: Joiner, present: bool) {
        if joiner.user == data::own_user() {
            return;
        }
        let Some(id) = sfu_id(joiner.sfu) else {
            return;
        };
        let Ok(watchers) = self.watchers.lock() else {
            return;
        };
        for watcher in watchers
            .iter()
            .filter(|watcher| watcher.room_id == room_id && watcher.sfu == joiner.sfu)
        {
            send_media(
                &watcher.events_tx,
                MediaEvent::Present {
                    sfu: id.clone(),
                    identity: joiner.identity(),
                    present,
                },
            );
        }
    }

    pub fn others_in(&self, room_id: &str) -> Vec<(MemberKey, MediaIdentity)> {
        self.calls
            .lock()
            .map(|calls| {
                calls
                    .rooms
                    .get(room_id)
                    .into_iter()
                    .flatten()
                    .filter(|joiner| joiner.user != data::own_user())
                    .map(|joiner| (joiner.key(), joiner.identity()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn keys_from_others(&self, room_id: &str) -> Vec<CallSignal> {
        let shaped = scenario();
        self.others_in(room_id)
            .into_iter()
            .filter(|(member, _)| !(shaped.undecryptable && member.user == UNTRUSTED))
            .map(|(member, _)| CallSignal::Key {
                room_id: RoomId::new(room_id),
                key: Box::new(ReceivedKey {
                    member,
                    index: KeyIndex::FIRST,
                    key: MediaKey::new(DEMO_KEY),
                    sent_ms: unix_now_ms(),
                }),
            })
            .collect()
    }

    pub async fn signal(&self, signal: CallSignal) {
        let signals_tx = self.signals.lock().ok().and_then(|signals| signals.clone());
        let Some(signals_tx) = signals_tx else {
            tracing::debug!("demo: nobody follows call signals yet");
            return;
        };
        if signals_tx.send(signal).await.is_err() {
            tracing::debug!("demo: the call signals follower is gone");
        }
    }
}

pub enum Step {
    Rooms,
    Signal(CallSignal),
}

pub type Change = Box<dyn FnOnce(&CallBook) -> Vec<Step> + Send>;

pub struct ScriptedChange {
    pub after: Duration,
    pub change: Change,
}

pub fn script() -> Vec<ScriptedChange> {
    let shaped = scenario();
    let mut changes = Vec::new();
    if shaped.ring {
        let intent = if shaped.ring_video {
            CallIntent::Video
        } else {
            CallIntent::Voice
        };
        changes.push(ScriptedChange {
            after: RING_AFTER,
            change: Box::new(move |book: &CallBook| {
                if !shaped.ring_unconfirmed {
                    book.join(SARAH_CHAT, Joiner::member(SARAH, intent));
                }
                vec![
                    Step::Rooms,
                    Step::Signal(ring(SARAH_CHAT, SARAH, intent, RingKind::Ring)),
                ]
            }),
        });
        script_ring_endings(shaped, &mut changes);
    }
    if shaped.active && shaped.expiring {
        changes.push(ScriptedChange {
            after: EXPIRES_AFTER + EXPIRY_LAG,
            change: Box::new(|_: &CallBook| vec![Step::Rooms]),
        });
    }
    if shaped.group_call {
        changes.push(ScriptedChange {
            after: RING_AFTER,
            change: Box::new(|book: &CallBook| {
                book.join(MATRIX_HQ, Joiner::member(KAI, CallIntent::Voice));
                vec![
                    Step::Rooms,
                    Step::Signal(ring(MATRIX_HQ, KAI, CallIntent::Voice, RingKind::Notify)),
                ]
            }),
        });
    }
    changes
}

fn script_ring_endings(shaped: Scenario, changes: &mut Vec<ScriptedChange>) {
    if shaped.declined_elsewhere {
        changes.push(ScriptedChange {
            after: RING_AFTER + ELSEWHERE_AFTER,
            change: Box::new(|_: &CallBook| {
                vec![Step::Signal(CallSignal::Declined {
                    room_id: RoomId::new(SARAH_CHAT),
                    ring: ring_id(SARAH_CHAT),
                    by: UserId::new(data::own_user()),
                })]
            }),
        });
    }
    if shaped.answered_elsewhere {
        changes.push(ScriptedChange {
            after: RING_AFTER + ELSEWHERE_AFTER,
            change: Box::new(|book: &CallBook| {
                book.join(
                    SARAH_CHAT,
                    Joiner {
                        device: OTHER_OWN_DEVICE,
                        ..Joiner::member(data::own_user(), CallIntent::Voice)
                    },
                );
                vec![Step::Rooms]
            }),
        });
    }
    if shaped.ring_ends {
        changes.push(ScriptedChange {
            after: RING_AFTER + RING_ENDS_AFTER,
            change: Box::new(|book: &CallBook| {
                book.leave(SARAH_CHAT, SARAH);
                vec![Step::Rooms]
            }),
        });
    }
}

fn ring_id(room_id: &str) -> RingId {
    RingId::new(format!("$demo-ring-{room_id}"))
}

fn ring(room_id: &str, caller: &str, intent: CallIntent, kind: RingKind) -> CallSignal {
    CallSignal::Rang(Box::new(Ring {
        id: ring_id(room_id),
        room_id: RoomId::new(room_id),
        caller: NamedUser::new(
            caller,
            MemberName::new(data::person_name(caller).as_deref(), false),
        ),
        intent: Some(intent),
        kind,
        expires_ms: unix_now_ms().saturating_add(RING_LIFETIME_MS),
    }))
}

pub fn partner_of(room_id: &str) -> Option<&'static str> {
    (room_id == SARAH_CHAT).then_some(SARAH)
}

pub fn answer_in(room_id: &str) {
    if let Some(partner) = partner_of(room_id) {
        book().join(room_id, Joiner::member(partner, CallIntent::Voice));
    }
}

pub fn own_device() -> &'static str {
    static DEVICE: OnceLock<String> = OnceLock::new();
    DEVICE.get_or_init(|| data::session().device_id)
}

pub fn sfu_id(sfu: &str) -> Option<SfuId> {
    SfuId::of(&Transport {
        service_url: Some(sfu.to_owned()),
        sfu_url: None,
    })
}

fn send_media(events_tx: &mpsc::UnboundedSender<MediaEvent>, event: MediaEvent) {
    if events_tx.send(event).is_err() {
        tracing::debug!("demo: the call media session is gone");
    }
}

pub async fn pause() {
    if scenario().slow {
        sleep(SLOW_ANSWER).await;
    }
}
