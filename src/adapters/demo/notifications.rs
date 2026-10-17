use std::collections::HashSet;
use std::env;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use super::catalog::{Flag, Scenarios};
use super::data;
use crate::adapters::notifier;
use crate::domain::message::{DecryptionFailure, MessageBody, MessagePreviewKind, RichText};
use crate::domain::push::PushTweaks;
use crate::domain::room::RoomId;
use crate::ports::notifier::{
    ActivationSink, DesktopNotice, NoticeAction, NoticeActivation, NotifierPort, RingNotice,
};

const ENV_VAR: &str = "U2DM_DEMO_NOTIFY";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "makes messages arrive in rooms that are not open, as push rules would notify them, and logs the desktop notifications instead of showing them",
    combinable: true,
    flags: &[
        Flag {
            value: "group",
            effect: "a message arrives in !rust:demo.local three seconds after the session starts",
            note: "",
        },
        Flag {
            value: "dm",
            effect: "Priya writes in her direct chat, with the sound tweak a DM rule sets",
            note: "",
        },
        Flag {
            value: "mention",
            effect: "a message in !matrixhq:demo.local names you, with the highlight and sound tweaks",
            note: "the room was already unread, so its notification shows at once",
        },
        Flag {
            value: "spoiler",
            effect: "a message in !design:demo.local hides its ending under a spoiler",
            note: "the notification shows the cover label, never the hidden words",
        },
        Flag {
            value: "undecryptable",
            effect: "Nina sends a message this session cannot decrypt",
            note: "the notification says it is encrypted",
        },
        Flag {
            value: "muted",
            effect: "a message arrives in the muted !linux:demo.local, lighting its plain dot",
            note: "push rules silence a muted room, so nothing notifies",
        },
        Flag {
            value: "burst",
            effect: "three messages arrive in !rust:demo.local a second apart",
            note: "each replaces the room's notification",
        },
        Flag {
            value: "read-elsewhere",
            effect: "the first arrival's room is read on another device three seconds later",
            note: "its notification is withdrawn",
        },
        Flag {
            value: "invite",
            effect: "Leo's chat invite is held back, then arrives with a notification",
            note: "",
        },
        Flag {
            value: "desktop",
            effect: "also shows the notifications on the real desktop over D-Bus",
            note: "for a person watching; agents leave it off",
        },
        Flag {
            value: "all",
            effect: "every arrival except burst, then read-elsewhere and invite",
            note: "",
        },
    ],
    notes: &[
        "nothing pops up while the window has focus: send window_focus false first",
        "each notification logs `demo: a desktop notification shows`, each withdrawal `demo: a desktop notification is withdrawn`",
        "the click_notification probe op clicks a shown notification",
    ],
};

const FIRST_ARRIVAL: Duration = Duration::from_secs(3);
const ARRIVAL_GAP: Duration = Duration::from_millis(1500);
const BURST_GAP: Duration = Duration::from_secs(1);
pub const ROOM_ECHO_LAG: Duration = Duration::from_millis(250);
const READ_ELSEWHERE_AFTER: Duration = Duration::from_secs(3);
pub const LATE_INVITE: &str = "!invite-leo:demo.local";
const GROUP_ROOM: &str = "!rust:demo.local";
const DIRECT_ROOM: &str = "!priya:demo.local";
const MENTION_ROOM: &str = "!matrixhq:demo.local";
const SPOILER_ROOM: &str = "!design:demo.local";
const UNDECRYPTABLE_ROOM: &str = "!nina:demo.local";
const MUTED_ROOM: &str = "!linux:demo.local";
const BURST: [&str; 3] = [
    "The build is green again",
    "Merging the parser fix now",
    "Done, thanks everyone",
];

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub group: bool,
    pub dm: bool,
    pub mention: bool,
    pub spoiler: bool,
    pub undecryptable: bool,
    pub muted: bool,
    pub burst: bool,
    pub read_elsewhere: bool,
    pub invite: bool,
    pub desktop: bool,
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
    tracing::info!(flags = raw, "demo mode: shaping which messages notify");
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "group" => scenario.group = true,
        "dm" => scenario.dm = true,
        "mention" => scenario.mention = true,
        "spoiler" => scenario.spoiler = true,
        "undecryptable" => scenario.undecryptable = true,
        "muted" => scenario.muted = true,
        "burst" => scenario.burst = true,
        "read-elsewhere" => scenario.read_elsewhere = true,
        "invite" => scenario.invite = true,
        "desktop" => scenario.desktop = true,
        "all" => {
            scenario.group = true;
            scenario.dm = true;
            scenario.mention = true;
            scenario.spoiler = true;
            scenario.undecryptable = true;
            scenario.muted = true;
            scenario.read_elsewhere = true;
            scenario.invite = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub struct Arrival {
    pub room_id: RoomId,
    pub body: MessageBody,
    pub kind: MessagePreviewKind,
    pub tweaks: PushTweaks,
    pub notifies: bool,
}

impl Arrival {
    fn text(room_id: &str, text: &str, tweaks: PushTweaks) -> Self {
        Self::of(
            room_id,
            MessageBody::Text(RichText::plain(text.to_owned())),
            tweaks,
        )
    }

    fn of(room_id: &str, body: MessageBody, tweaks: PushTweaks) -> Self {
        Self {
            room_id: RoomId::new(room_id),
            kind: body.preview_kind(),
            body,
            tweaks,
            notifies: true,
        }
    }

    pub fn preview(&self) -> RichText {
        data::body_preview(&self.body)
    }
}

pub enum Step {
    Arrive(Box<Arrival>),
    ReadElsewhere(RoomId),
    Invite,
}

pub struct Timed {
    pub after: Duration,
    pub step: Step,
}

pub fn script() -> Vec<Timed> {
    let demo = scenario();
    let mut arrivals = Vec::new();
    if demo.group {
        arrivals.push(Arrival::text(
            GROUP_ROOM,
            "Has anyone tried the new trait solver yet?",
            PushTweaks::default(),
        ));
    }
    if demo.dm {
        arrivals.push(Arrival::text(
            DIRECT_ROOM,
            "Are we still on for lunch tomorrow?",
            PushTweaks {
                highlight: false,
                sound: true,
            },
        ));
    }
    if demo.mention {
        arrivals.push(mention());
    }
    if demo.spoiler {
        arrivals.push(spoiler());
    }
    if demo.undecryptable {
        arrivals.push(Arrival::of(
            UNDECRYPTABLE_ROOM,
            MessageBody::UnableToDecrypt(DecryptionFailure::default()),
            PushTweaks {
                highlight: false,
                sound: true,
            },
        ));
    }
    if demo.muted {
        arrivals.push(Arrival {
            notifies: false,
            ..Arrival::text(MUTED_ROOM, "Kernel 7.3 is out", PushTweaks::default())
        });
    }
    let first_room = arrivals.first().map(|arrival| arrival.room_id.clone());
    let mut steps: Vec<Timed> = arrivals
        .into_iter()
        .enumerate()
        .map(|(index, arrival)| Timed {
            after: if index == 0 {
                FIRST_ARRIVAL
            } else {
                ARRIVAL_GAP
            },
            step: Step::Arrive(Box::new(arrival)),
        })
        .collect();
    if demo.burst {
        let opens_script = steps.is_empty();
        steps.extend(BURST.iter().enumerate().map(|(index, text)| Timed {
            after: if index == 0 && opens_script {
                FIRST_ARRIVAL
            } else {
                BURST_GAP
            },
            step: Step::Arrive(Box::new(Arrival::text(
                GROUP_ROOM,
                text,
                PushTweaks::default(),
            ))),
        }));
    }
    if demo.read_elsewhere
        && let Some(room_id) = first_room
    {
        steps.push(Timed {
            after: READ_ELSEWHERE_AFTER,
            step: Step::ReadElsewhere(room_id),
        });
    }
    if demo.invite {
        steps.push(Timed {
            after: if steps.is_empty() {
                FIRST_ARRIVAL
            } else {
                ARRIVAL_GAP
            },
            step: Step::Invite,
        });
    }
    steps
}

fn mention() -> Arrival {
    let own = data::own_member_name();
    let name = own.set().unwrap_or(data::own_user());
    Arrival::text(
        MENTION_ROOM,
        &format!("{name}, could you look at the release notes before Friday?"),
        PushTweaks {
            highlight: true,
            sound: true,
        },
    )
}

fn spoiler() -> Arrival {
    let plain = "No spoilers, but the ending: [spoiler]".to_owned();
    let html =
        "No spoilers, but the ending: <span data-mx-spoiler>the robot was the narrator</span>"
            .to_owned();
    Arrival::of(
        SPOILER_ROOM,
        MessageBody::Text(RichText::formatted(plain, html)),
        PushTweaks::default(),
    )
}

pub struct DemoNotifier {
    sink: Mutex<Option<ActivationSink>>,
    shown: Mutex<HashSet<RoomId>>,
    rings: Mutex<HashSet<RoomId>>,
    desktop: Option<Arc<dyn NotifierPort>>,
}

static NOTIFIER: OnceLock<Arc<DemoNotifier>> = OnceLock::new();

pub fn notifier() -> Arc<dyn NotifierPort> {
    let notifier = NOTIFIER.get_or_init(|| {
        Arc::new(DemoNotifier {
            sink: Mutex::new(None),
            shown: Mutex::new(HashSet::new()),
            rings: Mutex::new(HashSet::new()),
            desktop: scenario().desktop.then(notifier::desktop),
        })
    });
    Arc::clone(notifier) as Arc<dyn NotifierPort>
}

pub fn click(room_id: &RoomId, token: Option<String>, action: NoticeAction) -> bool {
    let Some(notifier) = NOTIFIER.get() else {
        return false;
    };
    let rings = notifier
        .rings
        .lock()
        .is_ok_and(|rings| rings.contains(room_id));
    let shown = rings
        || (action == NoticeAction::Open
            && notifier
                .shown
                .lock()
                .is_ok_and(|shown| shown.contains(room_id)));
    let sink = notifier.sink.lock().ok().and_then(|sink| sink.clone());
    let Some(sink) = sink.filter(|_| shown) else {
        return false;
    };
    tracing::debug!(%room_id, ?action, "demo: a desktop notification is clicked");
    sink(NoticeActivation {
        key: room_id.clone(),
        token,
        action,
    });
    true
}

impl NotifierPort for DemoNotifier {
    fn show(&self, notice: DesktopNotice) {
        let replaces = self
            .shown
            .lock()
            .is_ok_and(|mut shown| !shown.insert(notice.key.clone()));
        tracing::info!(
            room_id = %notice.key,
            summary = notice.summary,
            body = notice.body,
            sound = notice.sound,
            replaces,
            "demo: a desktop notification shows"
        );
        if let Some(desktop) = &self.desktop {
            desktop.show(notice);
        }
    }

    fn withdraw(&self, key: &RoomId) {
        if self.shown.lock().is_ok_and(|mut shown| shown.remove(key)) {
            tracing::info!(room_id = %key, "demo: a desktop notification is withdrawn");
        }
        if let Some(desktop) = &self.desktop {
            desktop.withdraw(key);
        }
    }

    fn withdraw_all(&self) {
        let withdrawn = self
            .shown
            .lock()
            .map(|mut shown| shown.drain().count())
            .unwrap_or_default()
            + self
                .rings
                .lock()
                .map(|mut rings| rings.drain().count())
                .unwrap_or_default();
        if withdrawn > 0 {
            tracing::info!(withdrawn, "demo: every desktop notification is withdrawn");
        }
        if let Some(desktop) = &self.desktop {
            desktop.withdraw_all();
        }
    }

    fn ring(&self, notice: RingNotice) {
        let replaces = self
            .rings
            .lock()
            .is_ok_and(|mut rings| !rings.insert(notice.key.clone()));
        tracing::info!(
            room_id = %notice.key,
            summary = notice.summary,
            body = notice.body,
            sound = notice.sound,
            answers = notice.answer_label.is_some(),
            replaces,
            "demo: a call rings"
        );
        if let Some(desktop) = &self.desktop {
            desktop.ring(notice);
        }
    }

    fn stop_ringing(&self, key: &RoomId) {
        if self.rings.lock().is_ok_and(|mut rings| rings.remove(key)) {
            tracing::info!(room_id = %key, "demo: the ringing stops");
        }
        if let Some(desktop) = &self.desktop {
            desktop.stop_ringing(key);
        }
    }

    fn on_activation(&self, sink: ActivationSink) {
        if let Some(desktop) = &self.desktop {
            desktop.on_activation(Arc::clone(&sink));
        }
        if let Ok(mut slot) = self.sink.lock() {
            *slot = Some(sink);
        }
    }
}
