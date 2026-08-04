use sha2::{Digest, Sha256};

use super::auth::Session;
use crate::util::hex_encode;

const ACCOUNT_ID_BYTES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountScope(String);

impl AccountScope {
    pub fn new(homeserver: &str, user_id: &str) -> Self {
        let mut hasher = Sha256::new();
        for part in [homeserver, user_id] {
            hash_length_prefixed(&mut hasher, part);
        }
        let digest = hasher.finalize();
        Self(hex_encode(
            digest.get(..ACCOUNT_ID_BYTES).unwrap_or(&digest),
        ))
    }

    pub fn from_session(session: &Session) -> Self {
        Self::new(&session.homeserver, &session.user_id)
    }

    pub fn from_id(id: String) -> Self {
        Self(id)
    }

    pub fn id(&self) -> &str {
        &self.0
    }
}

fn hash_length_prefixed(hasher: &mut Sha256, part: &str) {
    hasher.update((part.len() as u64).to_be_bytes());
    hasher.update(part.as_bytes());
}
