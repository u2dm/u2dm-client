use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;

use futures_util::{Stream, StreamExt};
use matrix_sdk::encryption::LocalTrust;
use matrix_sdk::encryption::identities::IdentityUpdates;
use matrix_sdk::ruma::{OwnedUserId, UserId};
use matrix_sdk::send_queue::{LocalEcho, LocalEchoContent, RoomSendQueueUpdate};
use matrix_sdk::{QueueWedgeError, Room};
use matrix_sdk_base::crypto::{IdentityState, IdentityStatusChange, RoomIdentityState};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc};

use crate::adapters::matrix::member_name::member_name;
use crate::domain::identity::{RoomAlert, SessionsOf};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::error::{AppError, Result};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Change {
    NoLongerVerified,
    Reset,
}

type Changes = BTreeMap<OwnedUserId, Change>;
type Holders = BTreeSet<OwnedUserId>;
type Statuses = Pin<Box<dyn Stream<Item = Vec<IdentityStatusChange>> + Send>>;
type Identities = Pin<Box<dyn Stream<Item = IdentityUpdates> + Send>>;

struct RoomWatch {
    statuses: Statuses,
    identities: Identities,
    queue: broadcast::Receiver<RoomSendQueueUpdate>,
    changes: Changes,
    holders: Holders,
}

impl RoomWatch {
    async fn next_change(&mut self, room: &Room) -> Result<bool> {
        tokio::select! {
            batch = self.statuses.next() => {
                let Some(batch) = batch else { return Ok(false) };
                note(&mut self.changes, batch);
            }
            update = self.identities.next() => {
                let Some(update) = update else { return Ok(false) };
                if names(&update, room.own_user_id()) {
                    self.reassess_members(room).await?;
                }
            }
            update = self.queue.recv() => {
                let settled = match update {
                    Ok(update) => settles_a_send(&update),
                    Err(RecvError::Lagged(_)) => true,
                    Err(RecvError::Closed) => return Ok(false),
                };
                if settled {
                    self.holders = reread_holders(room).await?;
                }
            }
        }
        Ok(true)
    }

    async fn reassess_members(&mut self, room: &Room) -> Result<()> {
        tracing::debug!("the account's own identity changed; reassessing the members' identities");
        self.statuses = statuses_of(room).await?;
        let mut changes = Changes::new();
        note(
            &mut changes,
            RoomIdentityState::new(room.clone()).await.current_state(),
        );
        changes.remove(room.own_user_id());
        self.changes = changes;
        Ok(())
    }
}

pub(super) async fn follow(room: &Room, alerts_tx: mpsc::Sender<Vec<RoomAlert>>) -> Result<()> {
    if !is_encrypted(room).await {
        tracing::debug!("the room is not encrypted, so no identity change concerns it");
        return Ok(());
    }
    let identities = room
        .client()
        .encryption()
        .user_identities_stream()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    let statuses = statuses_of(room).await?;
    let (echoes, queue) = room
        .send_queue()
        .subscribe()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    let mut watch = RoomWatch {
        statuses,
        identities: Box::pin(identities),
        queue,
        changes: Changes::new(),
        holders: holders_of(&echoes),
    };
    let mut reported = None;
    loop {
        let alerts = alerts(room, &watch.changes, &watch.holders).await;
        if reported.as_ref() != Some(&alerts) {
            if alerts_tx.send(alerts.clone()).await.is_err() {
                return Ok(());
            }
            reported = Some(alerts);
        }
        if !watch.next_change(room).await? {
            return Ok(());
        }
    }
}

pub(super) async fn send_anyway(room: &Room, user: &UserId) -> Result<()> {
    let (echoes, _updates) = room
        .send_queue()
        .subscribe()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    let encryption = room.client().encryption();
    let mut released = 0_usize;
    for echo in echoes {
        let LocalEchoContent::Event {
            send_handle,
            send_error: Some(QueueWedgeError::InsecureDevices { user_device_map }),
            ..
        } = echo.content
        else {
            continue;
        };
        let Some(devices) = user_device_map.get(user) else {
            continue;
        };
        for device_id in devices {
            let device = encryption
                .get_device(user, device_id)
                .await
                .map_err(|e| AppError::Other(e.to_string()))?;
            if let Some(device) = device {
                device
                    .set_local_trust(LocalTrust::Ignored)
                    .await
                    .map_err(|e| AppError::Other(e.to_string()))?;
            } else {
                tracing::debug!(%user, %device_id, "the unverified session is gone");
            }
        }
        send_handle
            .unwedge()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        released += 1;
    }
    if released == 0 {
        return Err(AppError::Other(format!(
            "no message is held by the unverified sessions of {user}"
        )));
    }
    room.send_queue().set_enabled(true);
    tracing::info!(%user, released, "sending to the user's unverified sessions anyway");
    Ok(())
}

async fn statuses_of(room: &Room) -> Result<Statuses> {
    let statuses = room
        .subscribe_to_identity_status_changes()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(Box::pin(statuses))
}

fn names(update: &IdentityUpdates, user: &UserId) -> bool {
    update.new.contains_key(user) || update.changed.contains_key(user)
}

async fn is_encrypted(room: &Room) -> bool {
    match room.latest_encryption_state().await {
        Ok(state) => state.is_encrypted(),
        Err(e) => {
            tracing::debug!("could not tell whether the room is encrypted, so following it: {e}");
            true
        }
    }
}

fn note(changes: &mut Changes, batch: Vec<IdentityStatusChange>) {
    for IdentityStatusChange {
        user_id,
        changed_to,
    } in batch
    {
        tracing::info!(%user_id, state = ?changed_to, "a member's identity status changed");
        match changed_to {
            IdentityState::VerificationViolation => {
                changes.insert(user_id, Change::NoLongerVerified);
            }
            IdentityState::PinViolation => {
                changes.insert(user_id, Change::Reset);
            }
            IdentityState::Pinned | IdentityState::Verified => {
                changes.remove(&user_id);
            }
        }
    }
}

fn settles_a_send(update: &RoomSendQueueUpdate) -> bool {
    match update {
        RoomSendQueueUpdate::SendError { .. }
        | RoomSendQueueUpdate::RetryEvent { .. }
        | RoomSendQueueUpdate::SentEvent { .. }
        | RoomSendQueueUpdate::CancelledLocalEvent { .. } => true,
        RoomSendQueueUpdate::NewLocalEvent(_)
        | RoomSendQueueUpdate::ReplacedLocalEvent { .. }
        | RoomSendQueueUpdate::MediaUpload { .. } => false,
    }
}

async fn reread_holders(room: &Room) -> Result<Holders> {
    let (echoes, _updates) = room
        .send_queue()
        .subscribe()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(holders_of(&echoes))
}

fn holders_of(echoes: &[LocalEcho]) -> Holders {
    echoes
        .iter()
        .filter_map(|echo| match &echo.content {
            LocalEchoContent::Event {
                send_error: Some(QueueWedgeError::InsecureDevices { user_device_map }),
                ..
            } => Some(user_device_map.keys().cloned()),
            LocalEchoContent::Event { .. }
            | LocalEchoContent::React { .. }
            | LocalEchoContent::Redaction { .. } => None,
        })
        .flatten()
        .collect()
}

async fn alerts(room: &Room, changes: &Changes, holders: &Holders) -> Vec<RoomAlert> {
    let mut alerts = Vec::with_capacity(changes.len() + holders.len());
    for (user, change) in changes {
        let user = named(room, user).await;
        alerts.push(match change {
            Change::NoLongerVerified => RoomAlert::NoLongerVerified(user),
            Change::Reset => RoomAlert::IdentityReset(user),
        });
    }
    for user in holders {
        let sessions = if user == room.own_user_id() {
            SessionsOf::You(user.to_string())
        } else {
            SessionsOf::Member(named(room, user).await)
        };
        alerts.push(RoomAlert::UnverifiedSessions(sessions));
    }
    alerts
}

async fn named(room: &Room, user: &UserId) -> NamedUser {
    let name = match room.get_member_no_sync(user).await {
        Ok(Some(member)) => member_name(room, &member).await,
        Ok(None) => MemberName::Unset,
        Err(e) => {
            tracing::debug!(%user, "could not read the member's name: {e}");
            MemberName::Unset
        }
    };
    NamedUser::new(user.to_string(), name)
}
