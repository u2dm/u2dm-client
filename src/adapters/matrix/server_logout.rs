use std::time::Duration;

use matrix_sdk::authentication::oauth::OAuthError;
use matrix_sdk::authentication::oauth::error::{
    BasicErrorResponseType, OAuthDiscoveryError, OAuthTokenRevocationError, RequestTokenError,
};
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::{AuthApi, Client, HttpError, RefreshTokenError};
use tokio::time::{sleep, timeout};

use super::auth::is_transport_failure;
use crate::ports::matrix::DeviceRemoval;

const LOGOUT_BUDGET: Duration = Duration::from_secs(10);
const LOGOUT_RETRY_FLOOR: Duration = Duration::from_secs(1);

enum Answer {
    Gone,
    Kept(String),
    Unreachable(String),
}

pub(super) async fn logout(client: &Client) -> DeviceRemoval {
    let device_id = client
        .device_id()
        .map(ToString::to_string)
        .unwrap_or_default();
    match timeout(LOGOUT_BUDGET, ask_until_answered(client)).await {
        Ok(Ok(())) => {
            tracing::info!(%device_id, "the homeserver signed this device out");
            DeviceRemoval::Confirmed
        }
        Ok(Err(refusal)) => {
            tracing::warn!(%device_id, "the homeserver kept this device: {refusal}");
            DeviceRemoval::Unconfirmed
        }
        Err(_) => {
            tracing::warn!(
                %device_id,
                budget_s = LOGOUT_BUDGET.as_secs(),
                "the homeserver did not answer the logout in time"
            );
            DeviceRemoval::Unconfirmed
        }
    }
}

async fn ask_until_answered(client: &Client) -> Result<(), String> {
    client
        .encryption()
        .wait_for_e2ee_initialization_tasks()
        .await;
    let mut retry_in = LOGOUT_RETRY_FLOOR;
    loop {
        match ask(client).await {
            Answer::Gone => return Ok(()),
            Answer::Kept(refusal) => return Err(refusal),
            Answer::Unreachable(cause) => {
                tracing::debug!(
                    retry_in_s = retry_in.as_secs(),
                    "the homeserver did not answer the logout: {cause}"
                );
                sleep(retry_in).await;
                retry_in = retry_in.saturating_mul(2);
            }
        }
    }
}

async fn ask(client: &Client) -> Answer {
    if let Some(answer) = refresh_before_revoking(client).await {
        return answer;
    }
    match client.logout().await {
        Ok(()) => Answer::Gone,
        Err(e) => logout_answer(&e),
    }
}

async fn refresh_before_revoking(client: &Client) -> Option<Answer> {
    if !matches!(client.auth_api(), Some(AuthApi::OAuth(_))) {
        return None;
    }
    match client.refresh_access_token().await {
        Ok(()) | Err(RefreshTokenError::RefreshTokenRequired) => None,
        Err(e) => refresh_answer(&e),
    }
}

fn refresh_answer(error: &RefreshTokenError) -> Option<Answer> {
    let RefreshTokenError::OAuth(oauth) = error else {
        return None;
    };
    match oauth.as_ref() {
        OAuthError::RefreshToken(RequestTokenError::ServerResponse(response))
            if *response.error() == BasicErrorResponseType::InvalidGrant =>
        {
            Some(Answer::Gone)
        }
        OAuthError::RefreshToken(RequestTokenError::Request(_)) => {
            Some(Answer::Unreachable(error.to_string()))
        }
        OAuthError::Discovery(OAuthDiscoveryError::Http(http)) if is_transport_failure(http) => {
            Some(Answer::Unreachable(error.to_string()))
        }
        _ => None,
    }
}

fn logout_answer(error: &matrix_sdk::Error) -> Answer {
    match error {
        matrix_sdk::Error::Http(http) => http_answer(http, error.to_string()),
        matrix_sdk::Error::OAuth(oauth) => oauth_answer(oauth, error.to_string()),
        _ => Answer::Kept(error.to_string()),
    }
}

fn http_answer(http: &HttpError, cause: String) -> Answer {
    match http {
        HttpError::Reqwest(_) => Answer::Unreachable(cause),
        HttpError::Cached(inner)
        | HttpError::RefreshToken(RefreshTokenError::MatrixAuth(inner)) => {
            http_answer(inner, cause)
        }
        _ => match http.client_api_error_kind() {
            Some(ErrorKind::UnknownToken(token)) if !token.soft_logout => Answer::Gone,
            _ => Answer::Kept(cause),
        },
    }
}

fn oauth_answer(oauth: &OAuthError, cause: String) -> Answer {
    match oauth {
        OAuthError::Logout(OAuthTokenRevocationError::Revoke(RequestTokenError::Request(_))) => {
            Answer::Unreachable(cause)
        }
        OAuthError::Discovery(OAuthDiscoveryError::Http(http)) if is_transport_failure(http) => {
            Answer::Unreachable(cause)
        }
        _ => Answer::Kept(cause),
    }
}
