use std::mem;
use std::time::Duration;

use matrix_sdk::HttpError;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk_ui::encryption_sync_service::Error as EncryptionSyncError;
use matrix_sdk_ui::room_list_service::Error as RoomListError;
use matrix_sdk_ui::sync_service::Error as SyncServiceError;
use tokio::time::Instant;

const SYNC_RESTART_BACKOFF_START: Duration = Duration::from_secs(1);
const SYNC_RESTART_BACKOFF_MAX: Duration = Duration::from_secs(30);
const SYNC_RESTART_HEALTHY_AFTER: Duration = Duration::from_mins(1);

fn extract_sdk_error(err: &SyncServiceError) -> Option<&matrix_sdk::Error> {
    match err {
        SyncServiceError::RoomList(RoomListError::SlidingSync(e))
        | SyncServiceError::EncryptionSync(EncryptionSyncError::SlidingSync(e)) => Some(e),
        _ => None,
    }
}

fn is_refresh_token_error(err: &matrix_sdk::Error) -> bool {
    match err {
        matrix_sdk::Error::Http(http) => matches!(http.as_ref(), HttpError::RefreshToken(_)),
        _ => false,
    }
}

pub(super) fn is_auth_error(err: &SyncServiceError) -> bool {
    extract_sdk_error(err).is_some_and(|e| {
        if matches!(
            e.client_api_error_kind(),
            Some(ErrorKind::UnknownToken { .. } | ErrorKind::Unauthorized | ErrorKind::Forbidden)
        ) {
            return true;
        }
        is_refresh_token_error(e)
    })
}

pub(super) struct SyncHealth {
    connected: bool,
    needs_resync: bool,
    backoff: Duration,
    restart_at: Option<Instant>,
    running_since: Option<Instant>,
}

impl SyncHealth {
    pub(super) fn started() -> Self {
        Self {
            connected: true,
            needs_resync: false,
            backoff: SYNC_RESTART_BACKOFF_START,
            restart_at: None,
            running_since: Some(Instant::now()),
        }
    }

    pub(super) fn restart_at(&self) -> Option<Instant> {
        self.restart_at
    }

    pub(super) fn on_running(&mut self) -> bool {
        self.restart_at = None;
        self.running_since = Some(Instant::now());
        mem::take(&mut self.needs_resync)
    }

    pub(super) fn should_announce_connected(&mut self) -> bool {
        !mem::replace(&mut self.connected, true)
    }

    pub(super) fn on_offline(&mut self) {
        self.needs_resync = true;
    }

    pub(super) fn on_restart(&mut self) {
        self.restart_at = None;
    }

    pub(super) fn on_error(&mut self) -> Duration {
        if self
            .running_since
            .is_some_and(|since| since.elapsed() >= SYNC_RESTART_HEALTHY_AFTER)
        {
            self.backoff = SYNC_RESTART_BACKOFF_START;
        }
        self.connected = false;
        self.needs_resync = true;
        self.running_since = None;
        let delay = self.backoff;
        self.restart_at = Some(Instant::now() + delay);
        self.backoff = self.backoff.saturating_mul(2).min(SYNC_RESTART_BACKOFF_MAX);
        delay
    }
}
