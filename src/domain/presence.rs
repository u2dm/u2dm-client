use std::time::Duration;

pub const STATUS_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Online,
    Unavailable,
    Offline,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresenceReport {
    pub availability: Option<Availability>,
    pub currently_active: bool,
    pub last_active_ago: Option<Duration>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PresenceLine {
    #[default]
    Hidden,
    Online,
    Away,
    LastSeen {
        minutes: u32,
    },
}

impl PresenceReport {
    pub fn line(&self) -> PresenceLine {
        match self.availability {
            Some(Availability::Online) => PresenceLine::Online,
            Some(Availability::Unavailable) => PresenceLine::Away,
            Some(Availability::Offline) if !self.currently_active => {
                self.last_active_ago.map_or(PresenceLine::Hidden, last_seen)
            }
            Some(Availability::Offline) | None => PresenceLine::Hidden,
        }
    }

    pub fn says_nothing(&self) -> bool {
        self.availability == Some(Availability::Offline)
            && self.last_active_ago.is_none()
            && !self.currently_active
    }

    pub fn status_line(&self) -> Option<String> {
        let words: Vec<String> = self
            .status
            .as_deref()?
            .split_whitespace()
            .map(|word| word.chars().filter(|c| !c.is_control()).collect::<String>())
            .filter(|word| !word.is_empty())
            .collect();
        let line: String = words.join(" ").chars().take(STATUS_CHARS).collect();
        (!line.is_empty()).then_some(line)
    }
}

fn last_seen(ago: Duration) -> PresenceLine {
    let minutes = u32::try_from(ago.as_secs() / 60).unwrap_or(u32::MAX);
    PresenceLine::LastSeen { minutes }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeaconFailure {
    Refused,
    Throttled(Option<Duration>),
    Failed,
}

pub type BeaconResult = Result<(), BeaconFailure>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupFailure {
    Refused,
    Failed,
}

pub type LookupResult = Result<PresenceReport, LookupFailure>;
