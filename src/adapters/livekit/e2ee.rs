use livekit::e2ee::key_provider::{KeyDerivationAlgorithm, KeyProvider, KeyProviderOptions};
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::id::ParticipantIdentity;

use crate::domain::call::keys::{KeyIndex, MediaKey};

const RATCHET_SALT: &[u8] = b"LKFrameEncryptionKey";
const RATCHET_WINDOW: i32 = 0;
const NEVER_INVALIDATE: i32 = -1;
const KEY_RING_SIZE: i32 = 256;

pub(super) fn key_provider() -> KeyProvider {
    KeyProvider::new(KeyProviderOptions {
        ratchet_window_size: RATCHET_WINDOW,
        ratchet_salt: RATCHET_SALT.to_vec(),
        failure_tolerance: NEVER_INVALIDATE,
        key_ring_size: KEY_RING_SIZE,
        key_derivation_algorithm: KeyDerivationAlgorithm::HKDF,
    })
}

pub(super) fn options(provider: &KeyProvider) -> E2eeOptions {
    E2eeOptions {
        encryption_type: EncryptionType::Gcm,
        key_provider: provider.clone(),
    }
}

pub(super) fn install(
    provider: &KeyProvider,
    identity: &ParticipantIdentity,
    index: KeyIndex,
    key: &MediaKey,
) {
    provider.set_key(identity, i32::from(index.get()), key.bytes().to_vec());
}
