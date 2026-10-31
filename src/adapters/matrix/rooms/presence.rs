use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::ruma::OwnedUserId;
use matrix_sdk::ruma::api::client::presence::{get_presence, set_presence};
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::exports::http::StatusCode;
use matrix_sdk::ruma::presence::PresenceState;
use matrix_sdk::{Client, HttpError};
use tokio::time::timeout;
use tracing::Instrument;

use super::send_queue::retry_after;
use crate::adapters::matrix::requests::refuses_endpoint;
use crate::adapters::matrix::session::ClientHandle;
use crate::domain::presence::{
    Availability, BeaconFailure, BeaconResult, LookupFailure, LookupResult, PresenceReport,
};
use crate::domain::user_info::UserId;
use crate::ports::matrix::PresencePort;

const ANNOUNCE_WAIT: Duration = Duration::from_secs(2);
const LOOKUP_WAIT: Duration = Duration::from_secs(5);

pub(in crate::adapters::matrix) struct MatrixPresence {
    matrix: Arc<ClientHandle>,
}

impl MatrixPresence {
    pub(in crate::adapters::matrix) const fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

#[async_trait]
impl PresencePort for MatrixPresence {
    async fn announce(&self, availability: Availability, status: Option<&str>) -> BeaconResult {
        let client = self.matrix.client().await.map_err(|e| {
            tracing::debug!("your presence has no session to go out on: {e}");
            BeaconFailure::Failed
        })?;
        announce(&client, availability, status)
            .instrument(tracing::debug_span!("presence", ?availability))
            .await
    }

    async fn presence_of(&self, user_id: &UserId) -> LookupResult {
        let client = self.matrix.client().await.map_err(|e| {
            tracing::debug!(%user_id, "no session to read presence with: {e}");
            LookupFailure::Failed
        })?;
        let Ok(subject) = OwnedUserId::try_from(user_id.as_ref()) else {
            return Err(LookupFailure::Refused);
        };
        let request = get_presence::v3::Request::new(subject);
        let sent = client
            .send(request)
            .with_request_config(RequestConfig::short_retry());
        match timeout(LOOKUP_WAIT, sent).await {
            Ok(Ok(response)) => Ok(report(response)),
            Ok(Err(e)) if refused(&e) => Err(LookupFailure::Refused),
            Ok(Err(e)) => {
                tracing::debug!(%user_id, "reading presence failed: {e}");
                Err(LookupFailure::Failed)
            }
            Err(_) => {
                tracing::debug!(%user_id, "reading presence got no answer in time");
                Err(LookupFailure::Failed)
            }
        }
    }
}

async fn announce(
    client: &Client,
    availability: Availability,
    status: Option<&str>,
) -> BeaconResult {
    let state = presence_state(availability);
    if let Err(e) = client.set_presence(state.clone(), None, false).await {
        tracing::debug!("the sliding sync requests could not take your presence: {e}");
    }
    let Some(user_id) = client.user_id() else {
        return Err(BeaconFailure::Failed);
    };
    let mut request = set_presence::v3::Request::new(user_id.to_owned(), state);
    request.status_msg = status.map(str::to_owned);
    let sent = client
        .send(request)
        .with_request_config(RequestConfig::short_retry().disable_retry());
    match timeout(ANNOUNCE_WAIT, sent).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(beacon_failure(&e)),
        Err(_) => {
            tracing::debug!("your presence got no answer in time");
            Err(BeaconFailure::Failed)
        }
    }
}

fn beacon_failure(error: &HttpError) -> BeaconFailure {
    if refused(error) {
        return BeaconFailure::Refused;
    }
    let kind = error.client_api_error_kind();
    if matches!(kind, Some(ErrorKind::LimitExceeded(_))) {
        return BeaconFailure::Throttled(retry_after(kind));
    }
    tracing::debug!("your presence did not go out: {error}");
    BeaconFailure::Failed
}

fn refused(error: &HttpError) -> bool {
    refuses_endpoint(error)
        || error
            .as_client_api_error()
            .is_some_and(|refusal| refusal.status_code == StatusCode::FORBIDDEN)
}

const fn presence_state(availability: Availability) -> PresenceState {
    match availability {
        Availability::Online => PresenceState::Online,
        Availability::Unavailable => PresenceState::Unavailable,
        Availability::Offline => PresenceState::Offline,
    }
}

fn report(response: get_presence::v3::Response) -> PresenceReport {
    PresenceReport {
        availability: availability(&response.presence),
        currently_active: response.currently_active.unwrap_or_default(),
        last_active_ago: response.last_active_ago,
        status: response.status_msg,
    }
}

fn availability(state: &PresenceState) -> Option<Availability> {
    match state {
        PresenceState::Online => Some(Availability::Online),
        PresenceState::Unavailable => Some(Availability::Unavailable),
        PresenceState::Offline => Some(Availability::Offline),
        _ => None,
    }
}
