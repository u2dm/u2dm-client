use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::commands::effects::Effect;

const CAPACITY: usize = 2048;

#[derive(Serialize, Clone)]
pub struct Record {
    pub seq: u64,
    pub at_ms: u64,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Serialize)]
pub struct Page {
    pub next: u64,
    pub dropped: u64,
    pub records: Vec<Record>,
}

struct Ring {
    next: u64,
    dropped: u64,
    records: VecDeque<Record>,
}

pub struct Journal {
    ring: Mutex<Ring>,
}

impl Journal {
    pub fn new() -> Self {
        Self {
            ring: Mutex::new(Ring {
                next: 0,
                dropped: 0,
                records: VecDeque::new(),
            }),
        }
    }

    pub fn record_effect(&self, effect: &Effect) {
        let (kind, room_id, generation, detail) = summarize(effect);
        self.push(kind, room_id, generation, detail);
    }

    pub fn record_command(&self, label: String) {
        self.push("command", None, None, Some(label));
    }

    fn push(
        &self,
        kind: &'static str,
        room_id: Option<String>,
        generation: Option<i32>,
        detail: Option<String>,
    ) {
        let Ok(mut ring) = self.ring.lock() else {
            return;
        };
        let seq = ring.next;
        ring.next += 1;
        ring.records.push_back(Record {
            seq,
            at_ms: now_ms(),
            kind,
            room_id,
            generation,
            detail,
        });
        while ring.records.len() > CAPACITY {
            ring.records.pop_front();
            ring.dropped += 1;
        }
    }

    pub fn page(&self, since: u64, limit: usize) -> Page {
        let Ok(ring) = self.ring.lock() else {
            return Page {
                next: since,
                dropped: 0,
                records: Vec::new(),
            };
        };
        let records: Vec<Record> = ring
            .records
            .iter()
            .filter(|record| record.seq >= since)
            .take(limit)
            .cloned()
            .collect();
        let next = records.last().map_or(ring.next, |record| record.seq + 1);
        Page {
            next,
            dropped: ring.dropped,
            records,
        }
    }
}

type Summary = (&'static str, Option<String>, Option<i32>, Option<String>);

fn summarize(effect: &Effect) -> Summary {
    match effect {
        Effect::Snapshot(_) => ("snapshot", None, None, None),
        Effect::SelectedRoom {
            id,
            name,
            generation,
            ..
        } => (
            "selected-room",
            Some(id.to_string()),
            Some(*generation),
            Some(name.clone()),
        ),
        Effect::Timeline {
            room_id,
            generation,
            patch,
        } => (
            "timeline",
            Some(room_id.to_string()),
            Some(*generation),
            Some(patch.label().to_owned()),
        ),
        Effect::TimelineStatus {
            room_id,
            generation,
            status,
        } => (
            "timeline-status",
            Some(room_id.to_string()),
            Some(*generation),
            Some(format!("{status:?}")),
        ),
        Effect::TimelineFocus {
            room_id,
            generation,
            event_id,
            row,
        } => (
            "timeline-focus",
            Some(room_id.to_string()),
            Some(*generation),
            Some(format!("{event_id}@{row}")),
        ),
        Effect::Verification(_) => ("verification", None, None, None),
        Effect::SessionReset(_) => ("session-reset", None, None, None),
    }
}

pub struct Selected {
    inner: Mutex<Option<(String, i32)>>,
}

impl Selected {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub fn observe(&self, effect: &Effect) {
        let Effect::SelectedRoom { id, generation, .. } = effect else {
            return;
        };
        if let Ok(mut current) = self.inner.lock() {
            *current = Some((id.to_string(), *generation));
        }
    }

    pub fn get(&self) -> Option<(String, i32)> {
        self.inner.lock().ok().and_then(|current| current.clone())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}
