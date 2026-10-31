use futures_util::StreamExt;
use matrix_sdk::Client;
use matrix_sdk::encryption::identities::UserIdentity;
use matrix_sdk::ruma::UserId;
use tokio::sync::{Notify, mpsc};

use crate::adapters::matrix::encryption::master_key;
use crate::adapters::matrix::user_info::own_user;
use crate::domain::identity::OwnIdentity;
use crate::error::{AppError, Result};

const ACKNOWLEDGED_MASTER_KEY: &[u8] = b"u2dm.own-identity.acknowledged-master-key";

pub(super) async fn follow(
    client: &Client,
    acknowledged: &Notify,
    own_tx: mpsc::Sender<OwnIdentity>,
) -> Result<()> {
    let own = own_user(client)?;
    let mut updates = Box::pin(
        client
            .encryption()
            .user_identities_stream()
            .await
            .map_err(AppError::other)?,
    );
    let mut reported = None;
    loop {
        match assess(client, &own).await {
            Ok(identity) if reported != Some(identity) => {
                tracing::info!(?identity, "the account's identity was assessed");
                if own_tx.send(identity).await.is_err() {
                    return Ok(());
                }
                reported = Some(identity);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("could not check the account's own identity: {e}"),
        }
        loop {
            tokio::select! {
                update = updates.next() => {
                    let Some(update) = update else { return Ok(()) };
                    if update.new.contains_key(&own) || update.changed.contains_key(&own) {
                        break;
                    }
                }
                () = acknowledged.notified() => break,
            }
        }
    }
}

pub(super) async fn acknowledge(client: &Client) -> Result<()> {
    let identity = own_identity(client, &own_user(client)?)
        .await?
        .ok_or_else(|| AppError::Other("the account has no identity to acknowledge".to_owned()))?;
    if identity.has_verification_violation() {
        identity
            .withdraw_verification()
            .await
            .map_err(AppError::other)?;
    }
    acknowledge_master_key(client, &master_key(&identity)).await?;
    tracing::info!("acknowledged that the account's identity was reset");
    Ok(())
}

enum Sighting {
    First,
    Acknowledged,
    VerifiedHere,
    Replaced,
}

fn sighting(acknowledged: Option<&str>, current: &str, verified_here: bool) -> Sighting {
    match acknowledged {
        None => Sighting::First,
        Some(acknowledged) if acknowledged == current => Sighting::Acknowledged,
        Some(_) if verified_here => Sighting::VerifiedHere,
        Some(_) => Sighting::Replaced,
    }
}

async fn assess(client: &Client, own: &UserId) -> Result<OwnIdentity> {
    let Some(identity) = own_identity(client, own).await? else {
        return Ok(OwnIdentity::Unchanged);
    };
    if identity.has_verification_violation() {
        return Ok(OwnIdentity::Reset);
    }
    let current = master_key(&identity);
    let acknowledged = acknowledged_master_key(client).await?;
    match sighting(acknowledged.as_deref(), &current, identity.is_verified()) {
        Sighting::Acknowledged => Ok(OwnIdentity::Unchanged),
        Sighting::Replaced => Ok(OwnIdentity::Reset),
        Sighting::First | Sighting::VerifiedHere => {
            acknowledge_master_key(client, &current).await?;
            tracing::debug!(%current, "remembering the account's identity");
            Ok(OwnIdentity::Unchanged)
        }
    }
}

async fn own_identity(client: &Client, own: &UserId) -> Result<Option<UserIdentity>> {
    client
        .encryption()
        .get_user_identity(own)
        .await
        .map_err(AppError::other)
}

async fn acknowledged_master_key(client: &Client) -> Result<Option<String>> {
    let stored = client
        .state_store()
        .get_custom_value(ACKNOWLEDGED_MASTER_KEY)
        .await
        .map_err(AppError::other)?;
    stored
        .map(String::from_utf8)
        .transpose()
        .map_err(AppError::other)
}

async fn acknowledge_master_key(client: &Client, master_key: &str) -> Result<()> {
    client
        .state_store()
        .set_custom_value(ACKNOWLEDGED_MASTER_KEY, master_key.as_bytes().to_vec())
        .await
        .map(drop)
        .map_err(AppError::other)
}
