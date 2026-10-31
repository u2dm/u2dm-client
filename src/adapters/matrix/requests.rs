use std::future::{Future, IntoFuture};
use std::time::Duration;
use std::{fmt, result};

use futures_util::FutureExt;
use matrix_sdk::HttpError;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::exports::http::StatusCode;
use tokio::time::timeout;

use crate::error::{AppError, Result};

pub(super) fn in_time<T, E, W>(
    wait: Duration,
    late: impl Into<String>,
    work: W,
) -> impl Future<Output = Result<T>>
where
    E: fmt::Display,
    W: IntoFuture<Output = result::Result<T, E>>,
{
    timeout(wait, work).map(|answer| {
        answer
            .map_err(|_| AppError::Other(late.into()))?
            .map_err(AppError::other)
    })
}

pub(super) fn absent_as_none<T>(sent: result::Result<T, HttpError>) -> Result<Option<T>> {
    match sent {
        Ok(response) => Ok(Some(response)),
        Err(e) if e.client_api_error_kind() == Some(&ErrorKind::NotFound) => Ok(None),
        Err(e) => Err(AppError::other(e)),
    }
}

pub(super) fn refuses_endpoint(http: &HttpError) -> bool {
    http.as_client_api_error().is_some_and(|refusal| {
        matches!(
            refusal.status_code,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ) || matches!(refusal.error_kind(), Some(ErrorKind::Unrecognized))
    })
}
