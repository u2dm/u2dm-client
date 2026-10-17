use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use matrix_sdk::config::RequestConfig;
use matrix_sdk::reqwest::StatusCode;
use matrix_sdk::ruma::OwnedRoomId;
use matrix_sdk::ruma::api::client::delayed_events::update_delayed_event::{
    UpdateAction, unstable_v1, unstable_v2,
};
use matrix_sdk::ruma::api::client::delayed_events::{DelayParameters, delayed_state_event};
use matrix_sdk::ruma::api::error::{ErrorKind, FromHttpResponseError};
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::{Client, HttpError};
use serde_json::{Value, json};

use super::CALL_MEMBER_TYPE;
use crate::domain::call::{RestartFailure, ScheduleFailure};

const MAX_DELAY_EXCEEDED: &str = "M_MAX_DELAY_EXCEEDED";
const DELAY_TOO_LARGE: &str = "ORG.MATRIX.MSC4140_DELAY_TOO_LARGE";
const MAX_DELAY_FIELD: &str = "org.matrix.msc4140.max_delay";

pub(super) struct DelayedEvents {
    v1_only: AtomicBool,
}

pub(super) async fn schedule_leave(
    client: &Client,
    room_id: OwnedRoomId,
    state_key: String,
    delay: Duration,
) -> Result<String, ScheduleFailure> {
    let body = Raw::new(&json!({}))
        .map(Raw::cast_unchecked)
        .map_err(|e| ScheduleFailure::Failed(e.to_string()))?;
    let request = delayed_state_event::unstable::Request::new_raw(
        room_id,
        state_key,
        StateEventType::from(CALL_MEMBER_TYPE),
        DelayParameters::Timeout { timeout: delay },
        body,
    );
    client
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await
        .map(|response| response.delay_id)
        .map_err(|e| schedule_failure(&e))
}

impl DelayedEvents {
    pub(super) const fn new() -> Self {
        Self {
            v1_only: AtomicBool::new(false),
        }
    }

    pub(super) async fn update(
        &self,
        client: &Client,
        delay_id: &str,
        action: UpdateAction,
    ) -> Result<(), HttpError> {
        if !self.v1_only.load(Ordering::Relaxed) {
            let request = unstable_v2::Request::new(delay_id.to_owned(), action.clone());
            match client
                .send(request)
                .with_request_config(RequestConfig::short_retry())
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) if unrecognized(&e) => {
                    tracing::debug!("the homeserver has no MSC4140 v2 update endpoint, using v1");
                    self.v1_only.store(true, Ordering::Relaxed);
                }
                Err(e) => return Err(e),
            }
        }
        let request = unstable_v1::Request::new(delay_id.to_owned(), action);
        client
            .send(request)
            .with_request_config(RequestConfig::short_retry())
            .await
            .map(|_| ())
    }
}

fn unrecognized(error: &HttpError) -> bool {
    matches!(error.client_api_error_kind(), Some(ErrorKind::Unrecognized))
}

fn errcode(error: &HttpError) -> Option<String> {
    error
        .client_api_error_kind()
        .map(|kind| kind.errcode().as_str().to_owned())
}

fn schedule_failure(error: &HttpError) -> ScheduleFailure {
    if let HttpError::Api(api) = error
        && matches!(**api, FromHttpResponseError::Deserialization(_))
    {
        return ScheduleFailure::Unsupported;
    }
    if let Some(code) =
        errcode(error).filter(|code| code == MAX_DELAY_EXCEEDED || code == DELAY_TOO_LARGE)
    {
        let max_ms = error
            .client_api_error_kind()
            .and_then(|kind| kind.custom_json_data())
            .and_then(|data| data.get(MAX_DELAY_FIELD))
            .and_then(Value::as_u64);
        tracing::debug!(code, ?max_ms, "the homeserver refused the leave's delay");
        return ScheduleFailure::TooLong { max_ms };
    }
    let endpoint_missing = unrecognized(error)
        || error.as_client_api_error().is_some_and(|refusal| {
            matches!(
                refusal.status_code,
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            )
        });
    if endpoint_missing {
        ScheduleFailure::Unsupported
    } else {
        ScheduleFailure::Failed(error.to_string())
    }
}

pub(super) fn restart_failure(error: &HttpError) -> RestartFailure {
    let gone = error.as_client_api_error().is_some_and(|refusal| {
        refusal.status_code == StatusCode::CONFLICT
            || (refusal.status_code == StatusCode::NOT_FOUND
                && matches!(refusal.error_kind(), Some(ErrorKind::NotFound)))
    });
    if gone {
        RestartFailure::Gone
    } else {
        RestartFailure::Failed(error.to_string())
    }
}
