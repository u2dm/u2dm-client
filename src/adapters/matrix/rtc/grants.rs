use matrix_sdk::Client;
use matrix_sdk::reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use super::http::{Auth, JsonRequest, Reply, host_of, send_json};
use crate::domain::call::Secret;
use crate::domain::call::identity::{MediaIdentity, spellings};
use crate::domain::call::sfu::{GrantFailure, GrantRequest, GrantResult, SfuGrant, SfuRole};

const HOMESERVER_TOKEN_PATH: &str =
    "/_matrix/client/unstable/io.element.msc4195/rtc/livekit/get_token";
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
            if let Some(sfu) = &transport.sfu_url {
                match asker.ask_homeserver(sfu).await {
                    Err(GrantFailure::NoRoute) => {}
                    answered => return answered,
                }
            }
            match &transport.service_url {
                Some(service) => asker.subscribe_through(service).await,
                None => Err(GrantFailure::NoRoute),
            }
        }
    }
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
        let url = self
            .client
            .homeserver()
            .join(HOMESERVER_TOKEN_PATH)
            .map_err(|e| GrantFailure::Malformed(e.to_string()))?;
        let body = json!({
            "url": sfu,
            "room_id": self.room_id,
            "slot_id": ROOM_SLOT,
            "member_id": self.request.member_id,
        });
        let reply = self
            .ask(
                Route::HomeserverGetToken,
                url.as_str(),
                &body,
                Auth::Homeserver,
            )
            .await?;
        if lacks_route(&reply) {
            return Err(GrantFailure::NoRoute);
        }
        self.granted(Route::HomeserverGetToken, &reply)
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
        if route == Route::ServiceGetToken && lacks_route(&reply) {
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
        tracing::info!(
            route = route.name(),
            host = host_of(url),
            status = %reply.status,
            errcode = reply.errcode(),
            error = reply.text("error"),
            "a call token request answered"
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
        let url = reply
            .text("url")
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

fn lacks_route(reply: &Reply) -> bool {
    matches!(
        reply.status,
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ) || matches!(reply.errcode(), Some(UNRECOGNIZED | NOT_FOUND))
}
