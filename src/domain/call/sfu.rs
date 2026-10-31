use std::fmt;

use url::Url;

use super::identity::MediaIdentity;
use super::{CallMember, DeviceId, FocusSelection, MemberKey, Secret, Transport};
use crate::domain::user_info::server_part;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SfuId(String);

impl SfuId {
    pub fn of(transport: &Transport) -> Option<Self> {
        transport
            .service_url
            .as_ref()
            .or(transport.sfu_url.as_ref())
            .map(|url| Self(url.clone()))
    }

    pub fn host(&self) -> String {
        Url::parse(&self.0)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| self.0.clone())
    }
}

impl AsRef<str> for SfuId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SfuId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SfuRole {
    Publish,
    Subscribe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfuTarget {
    pub id: SfuId,
    pub transport: Transport,
    pub role: SfuRole,
    pub server_name: String,
}

pub fn plan(own: &Transport, me: &MemberKey, members: &[CallMember]) -> Vec<SfuTarget> {
    let mut targets = Vec::new();
    if let Some(id) = SfuId::of(own) {
        targets.push(SfuTarget {
            id,
            transport: own.clone(),
            role: SfuRole::Publish,
            server_name: server_part(&me.user).to_owned(),
        });
    }
    let Some(oldest) = members.first() else {
        return targets;
    };
    for member in members {
        if member.user.user_id == me.user && member.device == me.device {
            continue;
        }
        let named_by = match member.selection {
            FocusSelection::MultiSfu => member,
            FocusSelection::OldestMembership => oldest,
        };
        let Some(transport) = &named_by.transport else {
            continue;
        };
        let Some(id) = SfuId::of(transport) else {
            continue;
        };
        if targets.iter().any(|target| target.id == id) {
            continue;
        }
        targets.push(SfuTarget {
            id,
            transport: transport.clone(),
            role: SfuRole::Subscribe,
            server_name: server_part(&named_by.user.user_id).to_owned(),
        });
    }
    targets
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRequest {
    pub target: SfuTarget,
    pub device: DeviceId,
    pub member_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfuGrant {
    pub sfu: SfuId,
    pub role: SfuRole,
    pub url: String,
    pub token: Secret,
    pub identity: MediaIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantFailure {
    NoRoute,
    Refused(u16),
    Unreachable(String),
    Malformed(String),
}

pub type GrantResult = Result<SfuGrant, GrantFailure>;

impl fmt::Display for GrantFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRoute => f.write_str("no token route answered"),
            Self::Refused(status) => write!(f, "the token service refused with HTTP {status}"),
            Self::Unreachable(why) => write!(f, "the token service is unreachable: {why}"),
            Self::Malformed(why) => write!(f, "the token service answered oddly: {why}"),
        }
    }
}
