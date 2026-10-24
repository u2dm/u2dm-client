use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use matrix_sdk::config::RequestConfig;
use matrix_sdk::reqwest::{Method, StatusCode};
use matrix_sdk::ruma::api::client::delayed_events::update_delayed_event::{
    UpdateAction, unstable_v1, unstable_v2,
};
use matrix_sdk::ruma::api::client::delayed_events::{DelayParameters, delayed_state_event};
use matrix_sdk::ruma::api::client::discovery::get_capabilities;
use matrix_sdk::ruma::api::error::{ErrorKind, FromHttpResponseError};
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{OwnedRoomId, TransactionId};
use matrix_sdk::{Client, HttpError};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use super::CALL_MEMBER_TYPE;
use super::http::{Auth, JsonRequest, Reply, homeserver_url, send_json};
use super::routes::{DELAYED_EVENTS_STABLE, advertises};
use crate::domain::call::{RestartFailure, RestartResult, ScheduleFailure};

const DELAY_TOO_LARGE: [&str; 3] = [
    "M_DELAY_TOO_LARGE",
    "ORG.MATRIX.MSC4140_DELAY_TOO_LARGE",
    "M_MAX_DELAY_EXCEEDED",
];
const MAX_DELAY_FIELD: &str = "org.matrix.msc4140.max_delay";
const NOT_FOUND: &str = "M_NOT_FOUND";
const LIMITS: [&str; 2] = ["m.delayed_events", "org.matrix.msc4140.delayed_events"];
const SCHEDULE_PATH: &str = "/_matrix/client/v3/rooms";
const UPDATE_PATH: &str = "/_matrix/client/v1/delayed_events";

pub(super) struct DelayedEvents {
    v1_only: AtomicBool,
    stable_missing: AtomicBool,
    dialect: OnceCell<Dialect>,
}

#[derive(Debug, Clone, Copy)]
struct Dialect {
    stable: bool,
    limits: Limits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Limits {
    Known {
        max_delay_ms: u64,
        max_scheduled: u64,
    },
    Absent,
    Unknown,
}

enum Stable<T> {
    Answered(T),
    Missing,
}

impl DelayedEvents {
    pub(super) const fn new() -> Self {
        Self {
            v1_only: AtomicBool::new(false),
            stable_missing: AtomicBool::new(false),
            dialect: OnceCell::const_new(),
        }
    }

    async fn dialect(&self, client: &Client) -> Dialect {
        *self
            .dialect
            .get_or_init(|| async {
                let dialect = Dialect {
                    stable: advertises(client, DELAYED_EVENTS_STABLE).await,
                    limits: limits(client).await,
                };
                tracing::debug!(?dialect, "the homeserver's delayed events");
                dialect
            })
            .await
    }

    fn stable(&self, dialect: Dialect) -> bool {
        dialect.stable && !self.stable_missing.load(Ordering::Relaxed)
    }

    fn stable_is_missing(&self) {
        tracing::info!(
            "the homeserver advertises the v1 delayed events but has no such endpoint, using the deployed ones"
        );
        self.stable_missing.store(true, Ordering::Relaxed);
    }

    pub(super) async fn schedule_leave(
        &self,
        client: &Client,
        room_id: OwnedRoomId,
        state_key: String,
        delay: Duration,
    ) -> Result<String, ScheduleFailure> {
        let dialect = self.dialect(client).await;
        let known_max = within_limits(dialect, delay)?;
        if self.stable(dialect) {
            match schedule_v1(client, &room_id, &state_key, delay, known_max).await {
                Stable::Answered(scheduled) => return scheduled,
                Stable::Missing => self.stable_is_missing(),
            }
        }
        schedule_deployed(client, room_id, state_key, delay, known_max).await
    }

    pub(super) async fn update(
        &self,
        client: &Client,
        delay_id: &str,
        action: UpdateAction,
    ) -> RestartResult {
        let dialect = self.dialect(client).await;
        if self.stable(dialect) {
            match update_v1(client, delay_id, &action).await {
                Stable::Answered(updated) => return updated,
                Stable::Missing => self.stable_is_missing(),
            }
        }
        self.update_deployed(client, delay_id, action)
            .await
            .map_err(|e| restart_failure(&e))
    }

    async fn update_deployed(
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

fn within_limits(dialect: Dialect, delay: Duration) -> Result<Option<u64>, ScheduleFailure> {
    match dialect.limits {
        Limits::Known {
            max_delay_ms,
            max_scheduled,
        } => {
            if max_delay_ms == 0 || max_scheduled == 0 {
                tracing::info!(
                    max_delay_ms,
                    max_scheduled,
                    "the homeserver has delayed events switched off"
                );
                return Err(ScheduleFailure::Unsupported);
            }
            if delay.as_millis() > u128::from(max_delay_ms) {
                return Err(ScheduleFailure::TooLong {
                    max_ms: Some(max_delay_ms),
                });
            }
            Ok(Some(max_delay_ms))
        }
        Limits::Absent if dialect.stable => {
            tracing::info!(
                "the homeserver advertises v1 delayed events without their limits, so they are off"
            );
            Err(ScheduleFailure::Unsupported)
        }
        Limits::Absent | Limits::Unknown => Ok(None),
    }
}

async fn limits(client: &Client) -> Limits {
    let answer = client
        .send(get_capabilities::v3::Request::new())
        .with_request_config(RequestConfig::short_retry())
        .await;
    let capabilities = match answer {
        Ok(answer) => answer.capabilities,
        Err(e) => {
            tracing::debug!("could not read the homeserver's capabilities: {e}");
            return Limits::Unknown;
        }
    };
    let Some(limits) = LIMITS.iter().find_map(|name| capabilities.get(name)) else {
        return Limits::Absent;
    };
    let number = |key: &str| limits.get(key).and_then(Value::as_u64);
    let (Some(max_delay_ms), Some(max_scheduled)) =
        (number("max_delay_ms"), number("max_scheduled"))
    else {
        tracing::debug!(%limits, "the homeserver's delayed event limits do not parse");
        return Limits::Unknown;
    };
    Limits::Known {
        max_delay_ms,
        max_scheduled,
    }
}

async fn schedule_v1(
    client: &Client,
    room_id: &OwnedRoomId,
    state_key: &str,
    delay: Duration,
    known_max: Option<u64>,
) -> Stable<Result<String, ScheduleFailure>> {
    let txn_id = TransactionId::new();
    let url = match homeserver_url(
        client,
        SCHEDULE_PATH,
        &[
            room_id.as_str(),
            "delayed_event",
            CALL_MEMBER_TYPE,
            txn_id.as_str(),
        ],
    ) {
        Ok(url) => url,
        Err(e) => return Stable::Answered(Err(ScheduleFailure::Failed(e))),
    };
    let body = json!({
        "delay_ms": u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
        "state_key": state_key,
        "content": {},
    });
    let reply = match ask(client, Method::PUT, &url, &body).await {
        Ok(reply) => reply,
        Err(e) => return Stable::Answered(Err(ScheduleFailure::Failed(e))),
    };
    if reply.status.is_success() {
        return Stable::Answered(
            reply
                .text("delay_id")
                .map(str::to_owned)
                .ok_or_else(|| ScheduleFailure::Failed("no delay_id in the answer".into())),
        );
    }
    if reply.lacks_endpoint() {
        return Stable::Missing;
    }
    Stable::Answered(Err(refusal(&reply, known_max)))
}

fn refusal(reply: &Reply, known_max: Option<u64>) -> ScheduleFailure {
    if reply.refuses() {
        tracing::info!(
            answer = reply.described(),
            "the homeserver refused the delayed leave"
        );
        return ScheduleFailure::Refused;
    }
    if reply
        .errcode()
        .is_some_and(|code| DELAY_TOO_LARGE.contains(&code))
    {
        return ScheduleFailure::TooLong { max_ms: known_max };
    }
    ScheduleFailure::Failed(reply.described())
}

async fn update_v1(
    client: &Client,
    delay_id: &str,
    action: &UpdateAction,
) -> Stable<RestartResult> {
    let url = match homeserver_url(client, UPDATE_PATH, &[delay_id, action.as_str()]) {
        Ok(url) => url,
        Err(e) => return Stable::Answered(Err(RestartFailure::Failed(e))),
    };
    let reply = match ask(client, Method::POST, &url, &json!({})).await {
        Ok(reply) => reply,
        Err(e) => return Stable::Answered(Err(RestartFailure::Failed(e))),
    };
    if reply.status.is_success() {
        return Stable::Answered(Ok(()));
    }
    let gone = reply.status == StatusCode::CONFLICT
        || (reply.status == StatusCode::NOT_FOUND && reply.errcode() == Some(NOT_FOUND));
    if gone {
        return Stable::Answered(Err(RestartFailure::Gone));
    }
    if reply.lacks_endpoint() {
        return Stable::Missing;
    }
    Stable::Answered(Err(RestartFailure::Failed(reply.described())))
}

async fn ask(client: &Client, method: Method, url: &str, body: &Value) -> Result<Reply, String> {
    let request = JsonRequest {
        method,
        url,
        body: Some(body),
        auth: Auth::Homeserver,
    };
    send_json(client, &request).await
}

async fn schedule_deployed(
    client: &Client,
    room_id: OwnedRoomId,
    state_key: String,
    delay: Duration,
    known_max: Option<u64>,
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
        .map_err(|e| schedule_failure(&e, known_max))
}

fn unrecognized(error: &HttpError) -> bool {
    matches!(error.client_api_error_kind(), Some(ErrorKind::Unrecognized))
}

fn errcode(error: &HttpError) -> Option<String> {
    error
        .client_api_error_kind()
        .map(|kind| kind.errcode().as_str().to_owned())
}

fn schedule_failure(error: &HttpError, known_max: Option<u64>) -> ScheduleFailure {
    if let HttpError::Api(api) = error
        && matches!(**api, FromHttpResponseError::Deserialization(_))
    {
        return ScheduleFailure::Unsupported;
    }
    if let Some(code) = errcode(error).filter(|code| DELAY_TOO_LARGE.contains(&code.as_str())) {
        let max_ms = error
            .client_api_error_kind()
            .and_then(|kind| kind.custom_json_data())
            .and_then(|data| data.get(MAX_DELAY_FIELD))
            .and_then(Value::as_u64)
            .or(known_max);
        tracing::debug!(code, ?max_ms, "the homeserver refused the leave's delay");
        return ScheduleFailure::TooLong { max_ms };
    }
    let refusal = error
        .as_client_api_error()
        .map(|refusal| refusal.status_code);
    if matches!(
        refusal,
        Some(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
    ) {
        tracing::info!("the homeserver refused the delayed leave: {error}");
        return ScheduleFailure::Refused;
    }
    let endpoint_missing = unrecognized(error)
        || matches!(
            refusal,
            Some(StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED)
        );
    if endpoint_missing {
        ScheduleFailure::Unsupported
    } else {
        ScheduleFailure::Failed(error.to_string())
    }
}

fn restart_failure(error: &HttpError) -> RestartFailure {
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
