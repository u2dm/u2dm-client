use matrix_sdk::Client;
use matrix_sdk::ruma::api::client::keys::get_keys;
use matrix_sdk::ruma::{DeviceId, DeviceKeyAlgorithm, DeviceKeyId, UserId};

use crate::error::{AppError, AuthFailure, Result};

pub(super) async fn ensure_identity_matches_server(client: &Client) -> Result<()> {
    let (Some(user_id), Some(device_id)) = (client.user_id(), client.device_id()) else {
        return Ok(());
    };
    let Some(local) = client.encryption().ed25519_key().await else {
        return Ok(());
    };
    let Some(published) = published_signing_key(client, user_id, device_id).await else {
        return Ok(());
    };

    if published == local {
        tracing::debug!(%device_id, %local, "the published signing key matches the local one");
        return Ok(());
    }

    tracing::warn!(
        %device_id, %local, %published,
        "the local signing key differs from the published one"
    );
    Err(AppError::Auth {
        kind: AuthFailure::IdentityDiverged,
        detail: format!("device {device_id} published {published}, the local store holds {local}"),
    })
}

async fn published_signing_key(
    client: &Client,
    user_id: &UserId,
    device_id: &DeviceId,
) -> Option<String> {
    let mut request = get_keys::v3::Request::new();
    request
        .device_keys
        .insert(user_id.to_owned(), vec![device_id.to_owned()]);

    let response = match client.send(request).await {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!("could not read the published signing key: {e}");
            return None;
        }
    };

    let key_id = DeviceKeyId::from_parts(DeviceKeyAlgorithm::Ed25519, device_id);
    let published = response
        .device_keys
        .get(user_id)
        .and_then(|devices| devices.get(device_id))
        .and_then(|keys| keys.deserialize().ok())
        .and_then(|keys| keys.keys.get(&key_id).cloned());

    if published.is_none() {
        tracing::info!(%device_id, "the homeserver has no signing key for this device yet");
    }
    published
}
