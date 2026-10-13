use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::pending;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use matrix_sdk::Client;
use matrix_sdk::encryption::backups::{BackupState, Backups};
use matrix_sdk::ruma::OwnedRoomId;
use matrix_sdk_base::crypto::types::events::UtdCause;
use matrix_sdk_ui::timeline::{EncryptedMessage, EventTimelineItem, Timeline, TimelineItem};
use tokio::task::JoinSet;
use tokio::time::{Instant, Sleep, sleep_until};
use tracing::Instrument;

const FIRST_TRY_AFTER: Duration = Duration::from_secs(2);
const TRIES: u32 = 5;
const NEWEST_SESSIONS: usize = 8;

type BackupEnabled = Pin<Box<dyn Stream<Item = bool> + Send>>;
type RoomKeys = Result<BTreeMap<String, BTreeSet<String>>, String>;
type KeyArrivals = Pin<Box<dyn Stream<Item = RoomKeys> + Send>>;

#[derive(Clone, Copy)]
enum Wait {
    Until(Instant),
    Fetching,
    ForBackup,
    Settled,
}

struct Missing {
    tries: u32,
    wait: Wait,
}

#[derive(Clone, Copy)]
enum Fetch {
    Imported,
    NoBackupKey,
    Lacking,
}

pub(super) struct MissingKeys {
    room_id: OwnedRoomId,
    sessions: HashMap<String, Missing>,
    in_flight: Option<String>,
    fetch: JoinSet<Fetch>,
    backup_enabled: BackupEnabled,
    arrivals: KeyArrivals,
    timer: Pin<Box<Sleep>>,
}

#[derive(Default)]
pub(super) struct KeysFromBackup(Vec<String>);

impl KeysFromBackup {
    fn arrived(keys: RoomKeys) -> Self {
        match keys {
            Ok(keys) => Self(keys.into_values().flatten().collect()),
            Err(error) => {
                tracing::warn!(%error, "room key stream lagged");
                Self::default()
            }
        }
    }

    pub(super) async fn retry_decryption(self, timeline: &Timeline) {
        if self.0.is_empty() {
            return;
        }
        tracing::debug!(
            sessions = self.0.len(),
            "room keys arrived from the key backup"
        );
        timeline.retry_decryption(self.0).await;
    }
}

impl MissingKeys {
    pub(super) fn new(client: &Client, room_id: OwnedRoomId) -> Self {
        let backups = client.encryption().backups();
        let backup_enabled = backups
            .state_stream()
            .map(|state| matches!(state, Ok(BackupState::Enabled)))
            .fuse();
        let arrivals = backups
            .room_keys_for_room_stream(&room_id)
            .map(|keys| keys.map_err(|error| error.to_string()))
            .fuse();
        Self {
            room_id,
            sessions: HashMap::new(),
            in_flight: None,
            fetch: JoinSet::new(),
            backup_enabled: Box::pin(backup_enabled),
            arrivals: Box::pin(arrivals),
            timer: Box::pin(sleep_until(Instant::now())),
        }
    }

    fn track(&mut self, items: &[Arc<TimelineItem>]) {
        let newest = newest_missing_sessions(items);
        self.sessions.retain(|session, _| newest.contains(session));
        let first_try = Instant::now() + FIRST_TRY_AFTER;
        for session in newest {
            self.sessions.entry(session).or_insert(Missing {
                tries: 0,
                wait: Wait::Until(first_try),
            });
        }
    }

    pub(super) async fn step(
        &mut self,
        client: &Client,
        items: &[Arc<TimelineItem>],
    ) -> KeysFromBackup {
        self.track(items);
        let due = self.next_due().filter(|_| self.in_flight.is_none());
        if let Some(at) = due {
            self.timer.as_mut().reset(at);
        }
        tokio::select! {
            Some(keys) = self.arrivals.next() => return KeysFromBackup::arrived(keys),
            Some(joined) = self.fetch.join_next() => {
                self.settle(joined.unwrap_or(Fetch::Lacking));
            }
            () = &mut self.timer, if due.is_some() => {
                self.start_due(client);
            }
            Some(enabled) = self.backup_enabled.next() => {
                if enabled {
                    self.backup_now_enabled();
                }
            }
            else => pending::<()>().await,
        }
        KeysFromBackup::default()
    }

    fn next_due(&self) -> Option<Instant> {
        self.sessions
            .values()
            .filter_map(|missing| match missing.wait {
                Wait::Until(at) => Some(at),
                Wait::Fetching | Wait::ForBackup | Wait::Settled => None,
            })
            .min()
    }

    fn start_due(&mut self, client: &Client) {
        let now = Instant::now();
        let Some((session, missing)) = self
            .sessions
            .iter_mut()
            .filter(|(_, missing)| matches!(missing.wait, Wait::Until(at) if at <= now))
            .min_by_key(|(_, missing)| missing.tries)
        else {
            return;
        };
        missing.wait = Wait::Fetching;
        tracing::debug!(
            session,
            tries = missing.tries,
            "asking the key backup again for an undecryptable message's room key"
        );
        self.in_flight = Some(session.clone());
        self.fetch.spawn(
            fetch(
                client.encryption().backups(),
                self.room_id.clone(),
                session.clone(),
            )
            .in_current_span(),
        );
    }

    fn settle(&mut self, fetched: Fetch) {
        let Some(session) = self.in_flight.take() else {
            return;
        };
        let Some(missing) = self.sessions.get_mut(&session) else {
            return;
        };
        missing.wait = match fetched {
            Fetch::Imported => Wait::Settled,
            Fetch::NoBackupKey => Wait::ForBackup,
            Fetch::Lacking => {
                missing.tries += 1;
                if missing.tries >= TRIES {
                    tracing::debug!(
                        session,
                        "the key backup still lacks this room key; giving up"
                    );
                    Wait::Settled
                } else {
                    Wait::Until(Instant::now() + FIRST_TRY_AFTER * 2_u32.pow(missing.tries))
                }
            }
        };
    }

    fn backup_now_enabled(&mut self) {
        let first_try = Instant::now() + FIRST_TRY_AFTER;
        for missing in self.sessions.values_mut() {
            if matches!(missing.wait, Wait::ForBackup) {
                missing.tries = 0;
                missing.wait = Wait::Until(first_try);
            }
        }
    }
}

async fn fetch(backups: Backups, room_id: OwnedRoomId, session: String) -> Fetch {
    match backups.download_room_key(&room_id, &session).await {
        Ok(true) => {
            tracing::debug!(session, "the key backup returned the missing room key");
            Fetch::Imported
        }
        Ok(false) => Fetch::NoBackupKey,
        Err(error) => {
            tracing::debug!(session, %error, "the key backup does not hold this room key yet");
            Fetch::Lacking
        }
    }
}

fn newest_missing_sessions(items: &[Arc<TimelineItem>]) -> Vec<String> {
    let mut sessions: Vec<String> = Vec::new();
    for session in items
        .iter()
        .rev()
        .filter_map(|item| missing_key_session(item.as_event()?))
    {
        if sessions.len() == NEWEST_SESSIONS {
            break;
        }
        if !sessions.contains(&session) {
            sessions.push(session);
        }
    }
    sessions
}

fn missing_key_session(event: &EventTimelineItem) -> Option<String> {
    match event.content().as_unable_to_decrypt()? {
        EncryptedMessage::MegolmV1AesSha2 {
            session_id, cause, ..
        } if !refused_by_trust(*cause) => Some(session_id.clone()),
        _ => None,
    }
}

fn refused_by_trust(cause: UtdCause) -> bool {
    matches!(
        cause,
        UtdCause::UnsignedDevice | UtdCause::UnknownDevice | UtdCause::VerificationViolation
    )
}
