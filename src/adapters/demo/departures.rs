use std::collections::BTreeMap;
use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use crate::domain::departure::RemovalKind;
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::message::{MessageBody, SendState, ServiceEvent, TimelineMessage};
use crate::domain::room::RoomId;

const ENV_VAR: &str = "U2DM_DEMO_DEPARTURES";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "removes or bans you from rooms, and shapes forgetting and leaving spaces",
    combinable: true,
    flags: &[
        Flag {
            value: "removed",
            effect: "the first sync lists job hunt crashout (Kai removed you, with a reason) and cat tax (Nadia banned you) as removals",
            note: "as a restart finds removals the store kept",
        },
        Flag {
            value: "kicked",
            effect: "Jordan removes you from Design Team ~5s after sync starts, with a reason",
            note: "open !design first to see its removal sheet replace the timeline",
        },
        Flag {
            value: "banned",
            effect: "Jordan bans you from Design Team ~5s after sync starts",
            note: "with kicked too, the ban wins",
        },
        Flag {
            value: "forget-fails",
            effect: "every Forget fails, so the sheet's error and the toast are reachable",
            note: "",
        },
        Flag {
            value: "slow",
            effect: "Forget, the space dialog's admin check and every leave wait ~1.5s",
            note: "",
        },
        Flag {
            value: "sole-admin",
            effect: "you are the only admin of every space, so the leave dialog warns",
            note: "",
        },
        Flag {
            value: "room-stays",
            effect: "leaving linux ricing fails, so leaving Rust with its rooms ends in the some-rooms toast",
            note: "only that room; the space itself still leaves",
        },
        Flag {
            value: "reasons",
            effect: "appends to Town Square Kai removing Leo and Sarah banning him, each with a reason",
            note: "the service rows a reason extends",
        },
    ],
    notes: &[
        "a forget or a leave reaches the room list ~0.6s after it returns, as a real sync echo does",
        "Design Team is in both the Design space and Rust's Web subspace, so leaving either with its rooms keeps it",
    ],
};

pub const KICKED_ROOM: &str = "!design:demo.local";
pub const KICKED_BY: &str = "@jordan:matrix.org";
pub const KICKED_REASON: &str = "Making room for the new team";
pub const KICK_AFTER: Duration = Duration::from_secs(5);
pub const STAYING_ROOM: &str = "!linux:demo.local";
const REASONS_ROOM: &str = "!square:demo.local";
const SLOW_DELAY: Duration = Duration::from_millis(1500);
const MODERATION_STEP_MS: u64 = 60_000;

struct ModerationRow {
    id: &'static str,
    by: &'static str,
    by_name: &'static str,
    banned: bool,
    reason: &'static str,
}

const MODERATION_ROWS: &[ModerationRow] = &[
    ModerationRow {
        id: "demo-square-removed-leo",
        by: "@kai:matrix.org",
        by_name: "Kai",
        banned: false,
        reason: "Spamming invite links",
    },
    ModerationRow {
        id: "demo-square-banned-leo",
        by: "@sarah:matrix.org",
        by_name: "Sarah Chen",
        banned: true,
        reason: "Came back with the same links",
    },
];
const MODERATED_USER: &str = "@leo:matrix.org";
const MODERATED_NAME: &str = "Leo";

pub struct StoredRemoval {
    pub room_id: &'static str,
    pub kind: RemovalKind,
    pub by: &'static str,
    pub reason: Option<&'static str>,
    pub minutes_ago: u64,
}

pub const STORED_REMOVALS: &[StoredRemoval] = &[
    StoredRemoval {
        room_id: "!jobs:demo.local",
        kind: RemovalKind::Kicked,
        by: "@kai:matrix.org",
        reason: Some("Off-topic links, please read the pinned rules"),
        minutes_ago: 45,
    },
    StoredRemoval {
        room_id: "!cats:demo.local",
        kind: RemovalKind::Banned,
        by: "@nadia:matrix.org",
        reason: None,
        minutes_ago: 180,
    },
];

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub stored: bool,
    pub kicked: bool,
    pub banned: bool,
    pub forget_fails: bool,
    pub is_slow: bool,
    pub sole_admin: bool,
    pub room_stays: bool,
    pub reasons: bool,
}

impl Scenario {
    pub fn live_removal(self) -> Option<RemovalKind> {
        if self.banned {
            Some(RemovalKind::Banned)
        } else if self.kicked {
            Some(RemovalKind::Kicked)
        } else {
            None
        }
    }
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
        stored = scenario.stored,
        kicked = scenario.kicked,
        banned = scenario.banned,
        forget_fails = scenario.forget_fails,
        is_slow = scenario.is_slow,
        sole_admin = scenario.sole_admin,
        room_stays = scenario.room_stays,
        reasons = scenario.reasons,
        "demo mode: shaping departures"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "removed" => scenario.stored = true,
        "kicked" => scenario.kicked = true,
        "banned" => scenario.banned = true,
        "forget-fails" => scenario.forget_fails = true,
        "slow" => scenario.is_slow = true,
        "sole-admin" => scenario.sole_admin = true,
        "room-stays" => scenario.room_stays = true,
        "reasons" => scenario.reasons = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub fn apply_scenario(room_id: &RoomId, messages: &mut Vec<TimelineMessage>) {
    if !scenario().reasons || room_id.as_ref() != REASONS_ROOM {
        return;
    }
    let Some(template) = messages
        .iter()
        .rev()
        .find(|message| !message.is_own)
        .cloned()
    else {
        return;
    };
    let mut timestamp = template.timestamp;
    for row in MODERATION_ROWS {
        timestamp = timestamp.saturating_add(MODERATION_STEP_MS);
        messages.push(moderation_row(&template, row, timestamp));
    }
}

fn moderation_row(
    template: &TimelineMessage,
    row: &ModerationRow,
    timestamp: u64,
) -> TimelineMessage {
    let target = NamedUser::new(MODERATED_USER, MemberName::new(Some(MODERATED_NAME), false));
    let reason = Some(row.reason.to_owned());
    let event = if row.banned {
        ServiceEvent::Banned { target, reason }
    } else {
        ServiceEvent::Kicked { target, reason }
    };
    TimelineMessage {
        unique_id: row.id.to_owned(),
        event_id: Some(row.id.to_owned()),
        local_id: None,
        sender: row.by.to_owned(),
        sender_name: MemberName::new(Some(row.by_name), false),
        sender_avatar_url: Some(row.by.to_owned()),
        sender_pronouns: Vec::new(),
        body: MessageBody::Service(event),
        body_emoji: BTreeMap::new(),
        mentions_room: false,
        timestamp,
        is_own: false,
        reply: None,
        edited: false,
        editable: false,
        is_first_unread: false,
        send_state: SendState::default(),
        reactions: Vec::new(),
        ..template.clone()
    }
}

pub async fn pause() {
    if scenario().is_slow {
        sleep(SLOW_DELAY).await;
    }
}
