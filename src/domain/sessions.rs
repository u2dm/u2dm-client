use std::cmp::Reverse;

use crate::domain::user_info::DeviceTrust;

const KEY_GROUP_CHARS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnSession {
    pub device_id: String,
    pub name: Option<String>,
    pub last_seen_ms: Option<u64>,
    pub last_seen_ip: Option<String>,
    pub trust: DeviceTrust,
}

impl OwnSession {
    pub fn shown_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.device_id)
    }

    fn sort_key(&self) -> (Reverse<Option<u64>>, String, &str) {
        (
            Reverse(self.last_seen_ms),
            self.shown_name().to_lowercase(),
            self.device_id.as_str(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThisSession {
    pub device_id: String,
    pub name: Option<String>,
    pub key: Option<String>,
}

impl ThisSession {
    pub fn fingerprint(&self) -> Option<String> {
        let key = self.key.as_deref()?;
        let chars: Vec<char> = key.chars().collect();
        let groups: Vec<String> = chars
            .chunks(KEY_GROUP_CHARS)
            .map(|group| group.iter().collect())
            .collect();
        Some(groups.join(" "))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnSessions {
    pub this: ThisSession,
    pub others: Vec<OwnSession>,
}

impl OwnSessions {
    pub fn new(this: ThisSession, mut others: Vec<OwnSession>) -> Self {
        others.retain(|session| session.device_id != this.device_id);
        others.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        Self { this, others }
    }

    pub fn other(&self, device_id: &str) -> Option<&OwnSession> {
        self.others
            .iter()
            .find(|session| session.device_id == device_id)
    }

    pub fn names(&self, device_id: &str) -> bool {
        self.this.device_id == device_id || self.other(device_id).is_some()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogoutRisks {
    pub keys_at_risk: bool,
    pub unsent: usize,
}

impl LogoutRisks {
    pub fn any(self) -> bool {
        self.keys_at_risk || self.unsent > 0
    }
}
