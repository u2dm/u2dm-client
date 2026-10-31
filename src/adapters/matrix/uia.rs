use matrix_sdk::ruma::api::client::uiaa::{
    AuthData, AuthType, Dummy, FallbackAcknowledgement, MatrixUserIdentifier, OAuthParams,
    Password, UiaaInfo, UserIdentifier,
};
use matrix_sdk::ruma::api::error::{ErrorBody, ErrorKind, StandardErrorBody};
use matrix_sdk::{Client, HttpError};

use super::user_info::own_user;
use crate::domain::uia::{AuthAnswer, AuthChallenge, AuthRefusal, AuthResponse, Guarded};
use crate::error::{AppError, Result};

pub(super) fn challenge(client: &Client, info: &UiaaInfo) -> AuthChallenge {
    AuthChallenge {
        session: info.session.clone(),
        flows: info
            .flows
            .iter()
            .map(|flow| stage_names(&flow.stages))
            .collect(),
        completed: stage_names(&info.completed),
        oauth_url: match info.params::<OAuthParams>(&AuthType::OAuth) {
            Ok(params) => params.map(|params| params.url),
            Err(e) => {
                tracing::debug!("the homeserver's m.oauth parameters do not parse: {e}");
                None
            }
        },
        homeserver: client.homeserver().to_string(),
        refusal: info.auth_error.as_ref().map(|error| AuthRefusal {
            code: error.kind.errcode().as_str().to_owned(),
            message: error.message.clone(),
        }),
    }
}

fn stage_names(stages: &[AuthType]) -> Vec<String> {
    stages
        .iter()
        .map(|stage| stage.as_str().to_owned())
        .collect()
}

pub(super) fn auth_data(client: &Client, answer: &AuthAnswer) -> Result<Option<AuthData>> {
    Ok(match &answer.response {
        AuthResponse::Password(password) => {
            let identifier =
                UserIdentifier::Matrix(MatrixUserIdentifier::new(own_user(client)?.to_string()));
            let mut data = Password::new(identifier, password.expose().to_owned());
            data.session.clone_from(&answer.session);
            Some(AuthData::Password(data))
        }
        AuthResponse::Dummy => {
            let mut data = Dummy::new();
            data.session.clone_from(&answer.session);
            Some(AuthData::Dummy(data))
        }
        AuthResponse::Acknowledged => answer.session.clone().map(|session| {
            AuthData::FallbackAcknowledgement(FallbackAcknowledgement::new(session))
        }),
    })
}

pub(super) fn answered(client: &Client, answer: Option<&AuthAnswer>) -> Result<Option<AuthData>> {
    Ok(answer
        .map(|answer| auth_data(client, answer))
        .transpose()?
        .flatten())
}

pub(super) fn guarded(client: &Client, error: &HttpError) -> Option<Guarded> {
    error
        .as_uiaa_response()
        .map(|info| Guarded::NeedsAuth(Box::new(challenge(client, info))))
}

pub(super) fn refused_as_weak(error: &HttpError) -> Option<AppError> {
    let body = &error.as_client_api_error()?.body;
    match body {
        ErrorBody::Standard(StandardErrorBody {
            kind: ErrorKind::WeakPassword,
            message,
            ..
        }) => Some(AppError::WeakPassword(message.clone())),
        _ => None,
    }
}
