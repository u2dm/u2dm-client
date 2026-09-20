use std::mem;
use std::time::Duration;

use matrix_sdk::authentication::oauth::OAuthError;
use matrix_sdk::authentication::oauth::error::{BasicErrorResponseType, RequestTokenError};
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::{HttpError, RefreshTokenError};
use matrix_sdk_ui::encryption_sync_service::Error as EncryptionSyncError;
use matrix_sdk_ui::room_list_service::Error as RoomListError;
use matrix_sdk_ui::sync_service::Error as SyncServiceError;
use tokio::time::Instant;

use crate::domain::sync::SessionLoss;

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

fn rejected_credentials(kind: Option<&ErrorKind>) -> Option<SessionLoss> {
    match kind? {
        ErrorKind::UnknownToken(data) if data.soft_logout => Some(SessionLoss::SoftLogout),
        ErrorKind::UnknownToken(_) | ErrorKind::Unauthorized | ErrorKind::Forbidden => {
            Some(SessionLoss::Expired)
        }
        _ => None,
    }
}

fn revokes_refresh_grant(err: &OAuthError) -> bool {
    matches!(
        err,
        OAuthError::RefreshToken(RequestTokenError::ServerResponse(response))
            if *response.error() == BasicErrorResponseType::InvalidGrant
    )
}

fn rejected_refresh(err: &RefreshTokenError) -> Option<SessionLoss> {
    match err {
        RefreshTokenError::RefreshTokenRequired => Some(SessionLoss::Expired),
        RefreshTokenError::MatrixAuth(http) => rejected_over_http(http),
        RefreshTokenError::OAuth(oauth) => {
            revokes_refresh_grant(oauth).then_some(SessionLoss::Expired)
        }
    }
}

fn rejected_over_http(err: &HttpError) -> Option<SessionLoss> {
    match err {
        HttpError::RefreshToken(refresh) => rejected_refresh(refresh),
        HttpError::Cached(inner) => rejected_over_http(inner),
        _ => rejected_credentials(err.client_api_error_kind()),
    }
}

pub(super) fn session_loss(err: &SyncServiceError) -> Option<SessionLoss> {
    match extract_sdk_error(err) {
        Some(matrix_sdk::Error::Http(http)) => rejected_over_http(http),
        _ => None,
    }
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
