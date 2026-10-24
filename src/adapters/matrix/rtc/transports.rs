use matrix_sdk::Client;
use matrix_sdk::reqwest::{Method, StatusCode};
use serde_json::Value;

use super::http::{Auth, JsonRequest, Reply, homeserver_url, send_json};
use super::routes::{TRANSPORTS_STABLE, advertises};
use crate::domain::call::{Transport, TransportLookup};

const STABLE_PATH: &str = "/_matrix/client/v1/rtc/transports";
const UNSTABLE_PATH: &str = "/_matrix/client/unstable/org.matrix.msc4143/rtc/transports";
const WELL_KNOWN_PATH: &str = "/.well-known/matrix/client";
const WELL_KNOWN_FOCI: [&str; 2] = ["org.matrix.msc4143.rtc_foci", "m.rtc_foci"];
const LISTS: [&str; 2] = ["transports", "rtc_transports"];
const LIVEKIT_TYPES: [&str; 2] = ["m.livekit", "livekit"];

pub(super) struct Discovered {
    pub(super) lookup: TransportLookup,
    pub(super) stable: bool,
}

enum Asked {
    Listed(Vec<Transport>),
    Missing,
    Refused(StatusCode),
}

#[derive(Clone, Copy)]
enum Route {
    Stable,
    Unstable,
}

impl Route {
    const fn path(self) -> &'static str {
        match self {
            Self::Stable => STABLE_PATH,
            Self::Unstable => UNSTABLE_PATH,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Stable => "v1",
            Self::Unstable => "MSC4143 unstable",
        }
    }
}

pub(super) async fn own_transports(client: &Client) -> Result<Discovered, String> {
    if advertises(client, TRANSPORTS_STABLE).await {
        match ask(client, Route::Stable).await? {
            Asked::Listed(found) if !found.is_empty() => return Ok(listed(found, true)),
            Asked::Listed(_) => return Ok(from_well_known(client).await),
            Asked::Missing => {}
            Asked::Refused(status) => return Ok(refused(status)),
        }
    }
    match ask(client, Route::Unstable).await? {
        Asked::Listed(found) if !found.is_empty() => Ok(listed(found, false)),
        Asked::Listed(_) | Asked::Missing => Ok(from_well_known(client).await),
        Asked::Refused(status) => Ok(refused(status)),
    }
}

async fn ask(client: &Client, route: Route) -> Result<Asked, String> {
    let url = homeserver_url(client, route.path(), &[])?;
    let request = JsonRequest {
        method: Method::GET,
        url: &url,
        body: None,
        auth: Auth::Homeserver,
    };
    let reply = send_json(client, &request)
        .await
        .map_err(|e| format!("the call transports endpoint did not answer: {e}"))?;
    tracing::debug!(
        route = route.name(),
        answer = reply.described(),
        "asked for the homeserver's call transports"
    );
    asked(&reply)
}

fn asked(reply: &Reply) -> Result<Asked, String> {
    if reply.status.is_success() {
        return Ok(Asked::Listed(entries(&reply.body)));
    }
    if reply.lacks_endpoint() {
        return Ok(Asked::Missing);
    }
    if reply.refuses() {
        return Ok(Asked::Refused(reply.status));
    }
    Err(format!(
        "the call transports endpoint answered {}",
        reply.described()
    ))
}

fn listed(found: Vec<Transport>, stable: bool) -> Discovered {
    tracing::debug!(
        count = found.len(),
        stable,
        "the homeserver's call transports"
    );
    Discovered {
        lookup: TransportLookup::Found(found),
        stable,
    }
}

fn refused(status: StatusCode) -> Discovered {
    tracing::info!(%status, "the homeserver refused to name its call transports");
    Discovered {
        lookup: TransportLookup::Refused,
        stable: false,
    }
}

async fn from_well_known(client: &Client) -> Discovered {
    let found = well_known(client).await;
    tracing::debug!(
        count = found.len(),
        "read the call transports from the well-known"
    );
    Discovered {
        lookup: if found.is_empty() {
            TransportLookup::NoneOffered
        } else {
            TransportLookup::Found(found)
        },
        stable: false,
    }
}

async fn well_known(client: &Client) -> Vec<Transport> {
    let Some(server) = client.user_id().map(|user| user.server_name().to_owned()) else {
        return Vec::new();
    };
    let url = format!("https://{server}{WELL_KNOWN_PATH}");
    let request = JsonRequest {
        method: Method::GET,
        url: &url,
        body: None,
        auth: Auth::Anonymous,
    };
    match send_json(client, &request).await {
        Ok(reply) if reply.status == StatusCode::OK => WELL_KNOWN_FOCI
            .iter()
            .map(|key| listed_under(reply.body.get(*key)))
            .find(|found| !found.is_empty())
            .unwrap_or_default(),
        Ok(reply) => {
            tracing::debug!(status = %reply.status, "the well-known did not answer");
            Vec::new()
        }
        Err(e) => {
            tracing::debug!("the well-known did not answer: {e}");
            Vec::new()
        }
    }
}

fn entries(body: &Value) -> Vec<Transport> {
    LISTS
        .iter()
        .find_map(|key| body.get(*key))
        .map(|list| listed_under(Some(list)))
        .unwrap_or_default()
}

fn listed_under(list: Option<&Value>) -> Vec<Transport> {
    list.and_then(Value::as_array)
        .map(|list| list.iter().filter_map(entry).collect())
        .unwrap_or_default()
}

fn entry(value: &Value) -> Option<Transport> {
    let kind = value.get("type").and_then(Value::as_str)?;
    if !LIVEKIT_TYPES.contains(&kind) {
        return None;
    }
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let transport = Transport {
        service_url: text("livekit_service_url"),
        sfu_url: text("url"),
    };
    (transport.service_url.is_some() || transport.sfu_url.is_some()).then_some(transport)
}
