use super::catalog::{Flag, Scenarios};
use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use super::data;
use crate::domain::message::{ReadBy, ReadScan, TimelineMessage};

const ENV_VAR: &str = "U2DM_DEMO_RECEIPTS";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "reproduces how read receipts reach your own messages",
    combinable: true,
    flags: &[
        Flag {
            value: "late",
            effect: "every read mark arrives as a Set rather than inside the opening Reset",
            note: "include in ANY read-mark test; marks in the opening Reset never exercise the restamp path production uses",
        },
        Flag {
            value: "crowd",
            effect: "forty members have read up to your newest message, so \"and N more\" renders",
            note: "",
        },
        Flag {
            value: "seen",
            effect: "a member reads each text you send shortly after it lands, so one check turns into two live",
            note: "",
        },
        Flag {
            value: "pending",
            effect: "every text you send stays a local echo, so the clock never resolves",
            note: "excluded from `all`; U2DM_DEMO_TIMELINE=send-fails still wins",
        },
        Flag {
            value: "all",
            effect: "late, crowd and seen",
            note: "excludes pending",
        },
    ],
    notes: &[
        "without any flag, a message counts as read by everyone who posted after it, which is the implicit receipt a homeserver reports",
    ],
};

pub const LATE_INTERVAL: Duration = Duration::from_millis(350);
pub const SEEN_DELAY: Duration = Duration::from_millis(1500);

const CROWD_SIZE: usize = 40;
const FALLBACK_READER: &str = "@member:matrix.org";

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub marks_arrive_late: bool,
    pub crowd_reads: bool,
    pub member_sees_sends: bool,
    pub sends_stay_pending: bool,
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
    tracing::info!(
        marks_arrive_late = scenario.marks_arrive_late,
        crowd_reads = scenario.crowd_reads,
        member_sees_sends = scenario.member_sees_sends,
        sends_stay_pending = scenario.sends_stay_pending,
        "demo mode: reproducing real-account read receipts"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "late" => scenario.marks_arrive_late = true,
        "crowd" => scenario.crowd_reads = true,
        "seen" => scenario.member_sees_sends = true,
        "pending" => scenario.sends_stay_pending = true,
        "all" => {
            scenario.marks_arrive_late = true;
            scenario.crowd_reads = true;
            scenario.member_sees_sends = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub struct Receipt {
    pub unique_id: String,
    pub reader: String,
}

pub fn seed_receipts(messages: &[TimelineMessage]) -> Vec<Receipt> {
    if !scenario().crowd_reads {
        return Vec::new();
    }
    let Some(newest) = messages
        .iter()
        .rev()
        .find(|message| message.is_own && message.event_id.is_some())
    else {
        return Vec::new();
    };
    (0..CROWD_SIZE)
        .map(|n| Receipt {
            unique_id: newest.unique_id.clone(),
            reader: format!("@reader{n}:matrix.org"),
        })
        .collect()
}

pub fn apply_scenario(messages: &mut [TimelineMessage]) {
    let receipts = seed_receipts(messages);
    stamp(messages, &receipts);
}

pub fn stamp(messages: &mut [TimelineMessage], receipts: &[Receipt]) -> Vec<usize> {
    let mut scan = ReadScan::excluding(Some(data::own_user()));
    let mut changed = Vec::new();
    for (index, message) in messages.iter_mut().enumerate().rev() {
        scan.observe(
            receipts
                .iter()
                .filter(|receipt| receipt.unique_id == message.unique_id)
                .map(|receipt| receipt.reader.as_str()),
        );
        if message.tracks_readers() && !scan.describes(&message.read_by, &message.sender) {
            message.read_by = scan.read_by(&message.sender);
            changed.push(index);
        }
        scan.observe([message.sender.as_str()]);
    }
    changed
}

pub fn likely_reader(messages: &[TimelineMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| !message.is_own && !message.sender.is_empty())
        .map_or_else(
            || FALLBACK_READER.to_owned(),
            |message| message.sender.clone(),
        )
}

pub fn strip_read_marks(messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    messages
        .iter()
        .map(|message| TimelineMessage {
            read_by: ReadBy::default(),
            ..message.clone()
        })
        .collect()
}

pub fn read_indices(messages: &[TimelineMessage]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.read_by.is_read())
        .map(|(index, _)| index)
        .collect()
}
