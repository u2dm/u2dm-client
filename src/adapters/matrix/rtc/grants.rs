use matrix_sdk::config::RequestConfig;
use matrix_sdk::reqwest::{Method, StatusCode};
use matrix_sdk::ruma::api::client::rtc::livekit::get_token;
use matrix_sdk::ruma::api::error::{ErrorBody, FromHttpResponseError};
use matrix_sdk::ruma::{OwnedRoomId, OwnedServerName};
use matrix_sdk::{Client, HttpError};
use serde_json::{Value, json};

use super::http::{Auth, JsonRequest, REQUEST_TIMEOUT, Reply, host_of, send_json};
use crate::domain::call::Secret;
use crate::domain::call::identity::{MediaIdentity, spellings};
use crate::domain::call::sfu::{GrantFailure, GrantRequest, GrantResult, SfuGrant, SfuRole};

const ROOM_SLOT: &str = "m.call#ROOM";
const UNRECOGNIZED: &str = "M_UNRECOGNIZED";
const NOT_FOUND: &str = "M_NOT_FOUND";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    ServiceSfuGet,
    ServiceGetToken,
    HomeserverGetToken,
}

impl Route {
    const fn name(self) -> &'static str {
        match self {
            Self::ServiceSfuGet => "/sfu/get",
            Self::ServiceGetToken => "/get_token",
            Self::HomeserverGetToken => "MSC4195 get_token",
        }
    }
}

struct Asker<'a> {
    client: &'a Client,
    room_id: &'a str,
    user: String,
    request: &'a GrantRequest,
}

pub(super) async fn grant(client: &Client, room_id: &str, request: &GrantRequest) -> GrantResult {
    let user = client
        .user_id()
        .map(ToString::to_string)
        .ok_or(GrantFailure::NoRoute)?;
    let asker = Asker {
        client,
        room_id,
        user,
        request,
    };
    let transport = &request.target.transport;
    match request.target.role {
        SfuRole::Publish => match (&transport.service_url, &transport.sfu_url) {
            (Some(service), _) => asker.ask_service(service, Route::ServiceSfuGet).await,
            (None, Some(sfu)) => asker.ask_homeserver(sfu).await,
            (None, None) => Err(GrantFailure::NoRoute),
        },
        SfuRole::Subscribe => {
            let answered = match &transport.sfu_url {
                Some(sfu) => asker.ask_homeserver(sfu).await,
                None => Err(GrantFailure::NoRoute),
            };
            match (answered, &transport.service_url) {
                (Err(failure), Some(service)) if worth_asking_the_service(&failure) => {
                    asker.subscribe_through(service).await
                }
                (answered, _) => answered,
            }
        }
    }
}

fn worth_asking_the_service(failure: &GrantFailure) -> bool {
    matches!(
        failure,
        GrantFailure::NoRoute | GrantFailure::Refused(500..=599)
    )
}

impl Asker<'_> {
    fn identity(&self, route: Route) -> MediaIdentity {
        let [legacy, hashed_member, hashed_device] =
            spellings(&self.user, &self.request.device, &self.request.member_id);
        match route {
            Route::ServiceSfuGet => legacy,
            Route::HomeserverGetToken => hashed_member,
            Route::ServiceGetToken => hashed_device,
        }
    }

    async fn ask_homeserver(&self, sfu: &str) -> GrantResult {
        let route = Route::HomeserverGetToken;
        let homeserver = self.client.homeserver();
        let answer = self
            .client
            .send(self.token_request(sfu)?)
            .with_request_config(
                RequestConfig::new()
                    .disable_retry()
                    .timeout(REQUEST_TIMEOUT),
            )
            .await;
        match answer {
            Ok(response) => {
                answered(route, homeserver.as_str(), StatusCode::OK, None, None);
                self.granted_with(route, &response.jwt, None)
            }
            Err(error) => Err(refused(route, homeserver.as_str(), &error)),
        }
    }

    fn token_request(&self, sfu: &str) -> Result<get_token::v1::Request, GrantFailure> {
        let room_id = OwnedRoomId::try_from(self.room_id)
            .map_err(|e| GrantFailure::Malformed(format!("{} is no room id: {e}", self.room_id)))?;
        let server = &self.request.target.server_name;
        let server_name = OwnedServerName::try_from(server.as_str())
            .map_err(|e| GrantFailure::Malformed(format!("{server} is no server name: {e}")))?;
        let mut request = get_token::v1::Request::new(
            sfu.to_owned(),
            room_id,
            ROOM_SLOT.to_owned(),
            self.request.member_id.clone(),
        );
        request.server_name = Some(server_name);
        Ok(request)
    }

    async fn subscribe_through(&self, service: &str) -> GrantResult {
        match self.ask_service(service, Route::ServiceGetToken).await {
            Err(GrantFailure::NoRoute) => {
                tracing::info!(
                    host = host_of(service),
                    "the call token service has no /get_token, subscribing through /sfu/get"
                );
                self.ask_service(service, Route::ServiceSfuGet).await
            }
            answered => answered,
        }
    }

    async fn ask_service(&self, service: &str, route: Route) -> GrantResult {
        let openid = self
            .client
            .account()
            .request_openid_token()
            .await
            .map_err(|e| GrantFailure::Unreachable(format!("no OpenID token: {e}")))?;
        let openid_token = json!({
            "access_token": openid.access_token,
            "token_type": openid.token_type.as_str(),
            "matrix_server_name": openid.matrix_server_name.as_str(),
            "expires_in": openid.expires_in.as_secs(),
        });
        let device = self.request.device.to_string();
        let (path, body) = match route {
            Route::ServiceSfuGet => (
                "sfu/get",
                json!({
                    "room": self.room_id,
                    "openid_token": openid_token,
                    "device_id": device,
                }),
            ),
            Route::ServiceGetToken | Route::HomeserverGetToken => (
                "get_token",
                json!({
                    "room_id": self.room_id,
                    "slot_id": ROOM_SLOT,
                    "openid_token": openid_token,
                    "member": {
                        "id": self.request.member_id,
                        "claimed_user_id": self.user,
                        "claimed_device_id": device,
                    },
                }),
            ),
        };
        let url = format!("{}/{path}", service.trim_end_matches('/'));
        let reply = self.ask(route, &url, &body, Auth::Anonymous).await?;
        if route == Route::ServiceGetToken && lacks_route(reply.status, reply.errcode()) {
            return Err(GrantFailure::NoRoute);
        }
        self.granted(route, &reply)
    }

    async fn ask(
        &self,
        route: Route,
        url: &str,
        body: &Value,
        auth: Auth,
    ) -> Result<Reply, GrantFailure> {
        let request = JsonRequest {
            method: Method::POST,
            url,
            body: Some(body),
            auth,
        };
        let reply = send_json(self.client, &request).await.map_err(|e| {
            tracing::info!(
                route = route.name(),
                host = host_of(url),
                "a call token request failed: {e}"
            );
            GrantFailure::Unreachable(e)
        })?;
        answered(
            route,
            url,
            reply.status,
            reply.errcode(),
            reply.text("error"),
        );
        Ok(reply)
    }

    fn granted(&self, route: Route, reply: &Reply) -> GrantResult {
        if !reply.status.is_success() {
            return Err(GrantFailure::Refused(reply.status.as_u16()));
        }
        let token = reply
            .text("jwt")
            .ok_or_else(|| GrantFailure::Malformed("no jwt in the answer".into()))?;
        self.granted_with(route, token, reply.text("url"))
    }

    fn granted_with(&self, route: Route, token: &str, answered_url: Option<&str>) -> GrantResult {
        let url = answered_url
            .or(self.request.target.transport.sfu_url.as_deref())
            .ok_or_else(|| GrantFailure::Malformed("no LiveKit url in the answer".into()))?;
        Ok(SfuGrant {
            sfu: self.request.target.id.clone(),
            role: self.request.target.role,
            url: url.to_owned(),
            token: Secret::new(token),
            identity: self.identity(route),
        })
    }
}

fn answered(
    route: Route,
    url: &str,
    status: StatusCode,
    errcode: Option<&str>,
    error: Option<&str>,
) {
    tracing::info!(
        route = route.name(),
        host = host_of(url),
        status = %status,
        errcode,
        error,
        "a call token request answered"
    );
}

fn refused(route: Route, url: &str, error: &HttpError) -> GrantFailure {
    let Some(refusal) = error.as_client_api_error() else {
        tracing::info!(
            route = route.name(),
            host = host_of(url),
            "a call token request failed: {error}"
        );
        return unanswered(error);
    };
    let errcode = refusal
        .error_kind()
        .map(|kind| kind.errcode().as_str().to_owned());
    let message = if let ErrorBody::Standard(body) = &refusal.body {
        Some(body.message.as_str())
    } else {
        None
    };
    answered(route, url, refusal.status_code, errcode.as_deref(), message);
    if lacks_route(refusal.status_code, errcode.as_deref()) {
        GrantFailure::NoRoute
    } else {
        GrantFailure::Refused(refusal.status_code.as_u16())
    }
}

fn unanswered(error: &HttpError) -> GrantFailure {
    if let HttpError::Api(api) = error
        && matches!(**api, FromHttpResponseError::Deserialization(_))
    {
        return GrantFailure::Malformed(error.to_string());
    }
    GrantFailure::Unreachable(error.to_string())
}

fn lacks_route(status: StatusCode, errcode: Option<&str>) -> bool {
    matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ) || matches!(errcode, Some(UNRECOGNIZED | NOT_FOUND))
}
