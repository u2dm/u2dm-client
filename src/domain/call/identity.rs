use std::collections::HashMap;
use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use sha2::{Digest, Sha256};

use super::{CallMember, MemberKey};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MediaIdentity(String);

impl MediaIdentity {
    #[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
    pub fn new(identity: impl Into<String>) -> Self {
        Self(identity.into())
    }
}

impl AsRef<str> for MediaIdentity {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MediaIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn spellings(user: &str, device: &str, member_id: &str) -> [MediaIdentity; 3] {
    [
        MediaIdentity(format!("{user}:{device}")),
        MediaIdentity(hashed(&[user, member_id])),
        MediaIdentity(hashed(&[user, device, member_id])),
    ]
}

pub fn member_spellings(member: &CallMember) -> [MediaIdentity; 3] {
    spellings(&member.user.user_id, &member.device, &member.member_id)
}

fn hashed(parts: &[&str]) -> String {
    let json = serde_json::to_string(parts).unwrap_or_default();
    STANDARD_NO_PAD.encode(Sha256::digest(json.as_bytes()))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityTable {
    members: HashMap<MediaIdentity, MemberKey>,
}

impl IdentityTable {
    pub fn build(members: &[CallMember]) -> Self {
        let members = members
            .iter()
            .flat_map(|member| {
                let key = MemberKey::of(member);
                member_spellings(member).map(move |identity| (identity, key.clone()))
            })
            .collect();
        Self { members }
    }

    pub fn member_of(&self, identity: &MediaIdentity) -> Option<&MemberKey> {
        self.members.get(identity)
    }

    pub fn identities_of(&self, member: &MemberKey) -> Vec<MediaIdentity> {
        let mut identities: Vec<MediaIdentity> = self
            .members
            .iter()
            .filter(|(_, key)| *key == member)
            .map(|(identity, _)| identity.clone())
            .collect();
        identities.sort();
        identities
    }
}
