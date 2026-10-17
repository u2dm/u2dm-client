use matrix_sdk::Client;
use matrix_sdk::reqwest::{Method, StatusCode};
use matrix_sdk::ruma::api::client::rtc::RtcTransport;
use serde_json::Value;

use super::http::{Auth, JsonRequest, send_json};
use crate::domain::call::Transport;

const TRANSPORTS_PATH: &str = "/_matrix/client/unstable/org.matrix.msc4143/rtc/transports";
const WELL_KNOWN_PATH: &str = "/.well-known/matrix/client";
const WELL_KNOWN_FOCI: [&str; 2] = ["org.matrix.msc4143.rtc_foci", "m.rtc_foci"];
const LIVEKIT: &str = "livekit";

pub(super) async fn own_transports(client: &Client) -> Vec<Transport> {
    if let Some(found) = discovered(client).await {
        return found;
    }
    let found = lenient(client).await;
    tracing::debug!(
        count = found.len(),
        "read the homeserver's call transports leniently"
    );
    found
}

async fn discovered(client: &Client) -> Option<Vec<Transport>> {
    let found = match client.discover_rtc_transports().await {
        Ok(Some(found)) => found,
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::debug!(
                "the homeserver's call transports did not parse, reading them leniently: {e}"
            );
            return None;
        }
    };
    let transports: Vec<Transport> = found.iter().filter_map(typed).collect();
    tracing::debug!(count = transports.len(), "the homeserver's call transports");
    (!transports.is_empty()).then_some(transports)
}

fn typed(transport: &RtcTransport) -> Option<Transport> {
    match transport {
        RtcTransport::LiveKit(livekit) => Some(Transport {
            service_url: Some(livekit.service_url.clone()),
            sfu_url: None,
        }),
        _ => None,
    }
}

async fn lenient(client: &Client) -> Vec<Transport> {
    let endpoint = client.homeserver().join(TRANSPORTS_PATH).ok();
    if let Some(url) = endpoint {
        let request = JsonRequest {
            method: Method::GET,
            url: url.as_str(),
            body: None,
            auth: Auth::Homeserver,
        };
        match send_json(client, &request).await {
            Ok(reply) if reply.status == StatusCode::OK => {
                let found = entries(reply.body.get("rtc_transports"));
                if !found.is_empty() {
                    return found;
                }
            }
            Ok(reply) => {
                tracing::debug!(status = %reply.status, "the call transports endpoint refused");
            }
            Err(e) => tracing::debug!("the call transports endpoint did not answer: {e}"),
        }
    }
    well_known(client).await
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
            .map(|key| entries(reply.body.get(*key)))
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

fn entries(list: Option<&Value>) -> Vec<Transport> {
    list.and_then(Value::as_array)
        .map(|list| list.iter().filter_map(entry).collect())
        .unwrap_or_default()
}

fn entry(value: &Value) -> Option<Transport> {
    if value.get("type").and_then(Value::as_str) != Some(LIVEKIT) {
        return None;
    }
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let transport = Transport {
        service_url: text("livekit_service_url"),
        sfu_url: text("url"),
    };
    (transport.service_url.is_some() || transport.sfu_url.is_some()).then_some(transport)
}
