use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::reqwest::header::{CONTENT_TYPE, HeaderValue};
use matrix_sdk::reqwest::{Method, Response, StatusCode};
use serde_json::Value;

const BODY_MAX_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const JSON: &str = "application/json";
const UNKNOWN_TOKEN: &str = "M_UNKNOWN_TOKEN";

#[derive(Debug)]
pub(super) struct Reply {
    pub(super) status: StatusCode,
    pub(super) body: Value,
}

impl Reply {
    pub(super) fn errcode(&self) -> Option<&str> {
        self.body.get("errcode").and_then(Value::as_str)
    }

    pub(super) fn text(&self, key: &str) -> Option<&str> {
        self.body.get(key).and_then(Value::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Auth {
    Homeserver,
    Anonymous,
}

pub(super) struct JsonRequest<'a> {
    pub(super) method: Method,
    pub(super) url: &'a str,
    pub(super) body: Option<&'a Value>,
    pub(super) auth: Auth,
}

pub(super) async fn send_json(client: &Client, request: &JsonRequest<'_>) -> Result<Reply, String> {
    let presented = match request.auth {
        Auth::Homeserver => client.access_token(),
        Auth::Anonymous => None,
    };
    let reply = send_once(client, request, presented.as_deref()).await?;
    if request.auth == Auth::Anonymous || !refuses_token(&reply) {
        return Ok(reply);
    }
    if client.access_token() == presented {
        client
            .refresh_access_token()
            .await
            .map_err(|e| format!("the access token did not refresh: {e}"))?;
    }
    let renewed = client.access_token();
    send_once(client, request, renewed.as_deref()).await
}

fn refuses_token(reply: &Reply) -> bool {
    reply.status == StatusCode::UNAUTHORIZED && reply.errcode() == Some(UNKNOWN_TOKEN)
}

async fn send_once(
    client: &Client,
    request: &JsonRequest<'_>,
    access_token: Option<&str>,
) -> Result<Reply, String> {
    let mut builder = client
        .http_client()
        .request(request.method.clone(), request.url)
        .timeout(REQUEST_TIMEOUT);
    if let Some(token) = access_token {
        builder = builder.bearer_auth(token);
    }
    if let Some(body) = request.body {
        let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
        builder = builder
            .header(CONTENT_TYPE, HeaderValue::from_static(JSON))
            .body(bytes);
    }
    let response = builder.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let bytes = read_capped(response).await?;
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok(Reply { status, body })
}

async fn read_capped(mut response: Response) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len().saturating_add(chunk.len()) > BODY_MAX_BYTES {
            return Err(format!("the answer passed {BODY_MAX_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(super) fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}
