pub mod identity;
pub mod keys;
pub mod media;
pub mod ring;
pub mod sfu;

use std::collections::HashSet;
use std::sync::Arc;
use std::{fmt, ops};

use self::keys::{KeyIndex, MediaKey};
use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl ops::Deref for DeviceId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for DeviceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemberKey {
    pub user: String,
    pub device: DeviceId,
}

impl MemberKey {
    pub fn new(user: impl Into<String>, device: DeviceId) -> Self {
        Self {
            user: user.into(),
            device,
        }
    }

    pub fn of(member: &CallMember) -> Self {
        Self::new(member.user.user_id.clone(), member.device.clone())
    }
}

impl fmt::Display for MemberKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.user, self.device)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallIntent {
    Voice,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transport {
    pub service_url: Option<String>,
    pub sfu_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusSelection {
    MultiSfu,
    OldestMembership,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallMember {
    pub user: NamedUser,
    pub device: DeviceId,
    pub member_id: String,
    pub created_ms: u64,
    pub expires_ms: u64,
    pub intent: Option<CallIntent>,
    pub transport: Option<Transport>,
    pub selection: FocusSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomCall {
    members: Arc<[CallMember]>,
}

impl RoomCall {
    pub fn gather(mut members: Vec<CallMember>) -> Option<Self> {
        if members.is_empty() {
            return None;
        }
        members.sort_by(|a, b| {
            a.created_ms
                .cmp(&b.created_ms)
                .then_with(|| a.user.user_id.cmp(&b.user.user_id))
                .then_with(|| a.device.cmp(&b.device))
        });
        Some(Self {
            members: members.into(),
        })
    }

    pub fn members(&self) -> &[CallMember] {
        &self.members
    }

    pub fn intent(&self) -> Option<CallIntent> {
        self.members().first().and_then(|caller| caller.intent)
    }

    pub fn is_video(&self) -> bool {
        self.intent() == Some(CallIntent::Video)
    }

    pub fn people(&self) -> usize {
        self.members
            .iter()
            .map(|member| member.user.user_id.as_str())
            .collect::<HashSet<&str>>()
            .len()
    }

    pub fn joined_by(&self, user_id: &str) -> bool {
        self.members
            .iter()
            .any(|member| member.user.user_id == user_id)
    }

    pub fn joined_elsewhere(&self, user_id: &str, device: &str) -> bool {
        self.members
            .iter()
            .any(|member| member.user.user_id == user_id && *member.device != *device)
    }

    pub fn next_expiry_ms(&self) -> Option<u64> {
        self.members.iter().map(|member| member.expires_ms).min()
    }

    pub fn member(&self, key: &MemberKey) -> Option<&CallMember> {
        self.members
            .iter()
            .find(|member| member.user.user_id == key.user && member.device == key.device)
    }

    pub fn others(&self, me: &MemberKey) -> impl Iterator<Item = &CallMember> {
        self.members
            .iter()
            .filter(move |member| !(member.user.user_id == me.user && member.device == me.device))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnMembership {
    pub device: DeviceId,
    pub member_id: String,
    pub intent: CallIntent,
    pub transport: Transport,
    pub created_ms: Option<u64>,
    pub lifetime_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMembership {
    pub event_id: String,
    pub created_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayedLeave {
    pub room_id: RoomId,
    pub delay_id: Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStanding {
    CrossSigned,
    NotCrossSigned,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleFailure {
    Unsupported,
    TooLong { max_ms: Option<u64> },
    Failed(String),
}

pub type ScheduleResult = Result<DelayedLeave, ScheduleFailure>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartFailure {
    Gone,
    Failed(String),
}

pub type RestartResult = Result<(), RestartFailure>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingKey {
    pub member_id: String,
    pub index: KeyIndex,
    pub key: MediaKey,
    pub sent_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeySendReport {
    pub sent: Vec<MemberKey>,
    pub withheld: Vec<MemberKey>,
}
