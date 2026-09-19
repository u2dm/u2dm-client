use std::borrow::Cow;
use std::fmt::Display;
use std::io::{Cursor, Read};
use std::time::Duration;

use matrix_sdk::media::{MediaFormat, MediaRequestParameters, MediaThumbnailSettings};
use matrix_sdk::reqwest::{Request, Response, StatusCode};
use matrix_sdk::ruma::MxcUri;
use matrix_sdk::ruma::api::auth_scheme::SendAccessToken;
use matrix_sdk::ruma::api::client::{authenticated_media, media};
use matrix_sdk::ruma::api::error::{Error as ServerError, ErrorKind};
use matrix_sdk::ruma::api::path_builder::VersionHistory;
use matrix_sdk::ruma::api::{
    EndpointError, Metadata, OutgoingRequest, OutgoingRequestExt, SupportedVersions,
};
use matrix_sdk::ruma::events::room::{EncryptedFile, MediaSource};
use matrix_sdk::ruma::exports::http;
use matrix_sdk::{Client, SupportedAuthScheme};
use matrix_sdk_base::crypto::AttachmentDecryptor;

use crate::domain::media::{MediaFailure, MediaResult};

const SEND_QUEUE_SERVER_NAME: &str = "send-queue.localhost";
const CACHE_IN_SDK_MEDIA_STORE: bool = false;
const REFUSAL_BODY_MAX_BYTES: usize = 64 * 1024;

pub(super) async fn fetch(
    client: &Client,
    request: &MediaRequestParameters,
    max_bytes: usize,
    request_timeout: Duration,
) -> MediaResult<Vec<u8>> {
    if is_send_queue_echo(&request.source) {
        return read_send_queue_echo(client, request, max_bytes).await;
    }
    let transfer = Transfer {
        client,
        max_bytes,
        request_timeout,
    };
    match &request.source {
        MediaSource::Plain(uri) => {
            let endpoint = Endpoint {
                uri,
                thumbnail: thumbnail_settings(&request.format),
            };
            transfer.body(endpoint).await
        }
        MediaSource::Encrypted(file) => {
            let endpoint = Endpoint {
                uri: &file.url,
                thumbnail: None,
            };
            decrypt(transfer.body(endpoint).await?, file)
        }
    }
}

fn is_send_queue_echo(source: &MediaSource) -> bool {
    let uri = match source {
        MediaSource::Plain(uri) => uri,
        MediaSource::Encrypted(file) => &file.url,
    };
    uri.server_name()
        .is_ok_and(|server| server.as_str() == SEND_QUEUE_SERVER_NAME)
}

async fn read_send_queue_echo(
    client: &Client,
    request: &MediaRequestParameters,
    max_bytes: usize,
) -> MediaResult<Vec<u8>> {
    let data = client
        .media()
        .get_media_content(request, CACHE_IN_SDK_MEDIA_STORE)
        .await
        .map_err(|e| download_failed("local echo media is not in the send queue store", &e))?;
    if data.len() > max_bytes {
        tracing::debug!(
            "local echo media {} bytes exceeds the {max_bytes} byte cap",
            data.len()
        );
        return Err(MediaFailure::TooLarge);
    }
    Ok(data)
}

fn thumbnail_settings(format: &MediaFormat) -> Option<&MediaThumbnailSettings> {
    match format {
        MediaFormat::File => None,
        MediaFormat::Thumbnail(settings) => Some(settings),
    }
}

struct Transfer<'a> {
    client: &'a Client,
    max_bytes: usize,
    request_timeout: Duration,
}

enum Reply {
    Body(Vec<u8>),
    TokenRefused,
}

impl Transfer<'_> {
    async fn body(&self, endpoint: Endpoint<'_>) -> MediaResult<Vec<u8>> {
        let versions = self
            .client
            .supported_versions()
            .await
            .map_err(|e| download_failed("server versions for a media request", &e))?;
        let presented = self.client.access_token();
        if let Reply::Body(body) = self.send(endpoint, &versions, presented.as_deref()).await? {
            return Ok(body);
        }
        renew_access_token(self.client, presented.as_deref()).await?;
        let renewed = self.client.access_token();
        match self.send(endpoint, &versions, renewed.as_deref()).await? {
            Reply::Body(body) => Ok(body),
            Reply::TokenRefused => Err(download_failed(
                "media request",
                &"access token refused again after renewal",
            )),
        }
    }

    async fn send(
        &self,
        endpoint: Endpoint<'_>,
        versions: &SupportedVersions,
        access_token: Option<&str>,
    ) -> MediaResult<Reply> {
        let homeserver = self.client.homeserver();
        let target = Target {
            homeserver: homeserver.as_str(),
            versions,
            access_token,
        };
        let mut request = endpoint.http_request(&target)?;
        *request.timeout_mut() = Some(self.request_timeout);
        let response = self
            .client
            .http_client()
            .execute(request)
            .await
            .map_err(|e| download_failed("media request", &e))?;
        if response.status().is_success() {
            read_capped(response, self.max_bytes).await.map(Reply::Body)
        } else {
            classify_refusal(response).await
        }
    }
}

async fn renew_access_token(client: &Client, refused: Option<&str>) -> MediaResult<()> {
    if client.access_token().as_deref() != refused {
        return Ok(());
    }
    client
        .refresh_access_token()
        .await
        .map_err(|e| download_failed("token refresh for a media request", &e))
}

#[derive(Clone, Copy)]
struct Endpoint<'a> {
    uri: &'a MxcUri,
    thumbnail: Option<&'a MediaThumbnailSettings>,
}

impl Endpoint<'_> {
    fn http_request(self, target: &Target<'_>) -> MediaResult<Request> {
        if authenticated_media::get_content::v1::Request::PATH_BUILDER.is_supported(target.versions)
        {
            self.authenticated(target)
        } else {
            self.legacy(target)
        }
    }

    fn authenticated(self, target: &Target<'_>) -> MediaResult<Request> {
        let Some(settings) = self.thumbnail else {
            let request = authenticated_media::get_content::v1::Request::from_uri(self.uri)
                .map_err(|e| invalid_uri(self.uri, &e))?;
            return target.serialize(request);
        };
        let mut request = authenticated_media::get_content_thumbnail::v1::Request::from_uri(
            self.uri,
            settings.width,
            settings.height,
        )
        .map_err(|e| invalid_uri(self.uri, &e))?;
        request.method = Some(settings.method.clone());
        request.animated = Some(settings.animated);
        target.serialize(request)
    }

    #[allow(deprecated)]
    fn legacy(self, target: &Target<'_>) -> MediaResult<Request> {
        let Some(settings) = self.thumbnail else {
            let request = media::get_content::v3::Request::from_url(self.uri)
                .map_err(|e| invalid_uri(self.uri, &e))?;
            return target.serialize(request);
        };
        let mut request = media::get_content_thumbnail::v3::Request::from_url(
            self.uri,
            settings.width,
            settings.height,
        )
        .map_err(|e| invalid_uri(self.uri, &e))?;
        request.method = Some(settings.method.clone());
        request.animated = Some(settings.animated);
        target.serialize(request)
    }
}

struct Target<'a> {
    homeserver: &'a str,
    versions: &'a SupportedVersions,
    access_token: Option<&'a str>,
}

impl Target<'_> {
    fn serialize<R>(&self, endpoint: R) -> MediaResult<Request>
    where
        R: OutgoingRequest<PathBuilder = VersionHistory>,
        R::Authentication: SupportedAuthScheme,
    {
        let token = self
            .access_token
            .map_or(SendAccessToken::None, SendAccessToken::IfRequired);
        let request = endpoint
            .try_into_http_request::<Vec<u8>>(
                self.homeserver,
                R::Authentication::authentication_input(token),
                Cow::Borrowed(self.versions),
            )
            .map_err(|e| download_failed("media request did not serialize", &e))?;
        Request::try_from(request).map_err(|e| download_failed("media request did not convert", &e))
    }
}

async fn read_capped(mut response: Response, max_bytes: usize) -> MediaResult<Vec<u8>> {
    let declared = response.content_length();
    if let Some(declared) = declared.filter(|declared| *declared > max_bytes as u64) {
        tracing::debug!("media response declares {declared} bytes, over the {max_bytes} byte cap");
        return Err(MediaFailure::TooLarge);
    }
    let mut body = Vec::with_capacity(
        declared
            .and_then(|declared| usize::try_from(declared).ok())
            .unwrap_or_default(),
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| download_failed("media response body", &e))?
    {
        append_under_cap(&mut body, &chunk, max_bytes)?;
    }
    Ok(body)
}

fn append_under_cap(body: &mut Vec<u8>, chunk: &[u8], max_bytes: usize) -> MediaResult<()> {
    let needed = body.len().saturating_add(chunk.len());
    if needed > max_bytes {
        tracing::debug!("media response passed the {max_bytes} byte cap while streaming");
        return Err(MediaFailure::TooLarge);
    }
    if needed > body.capacity() {
        let grown = body.capacity().saturating_mul(2).max(needed).min(max_bytes);
        body.reserve_exact(grown.saturating_sub(body.len()));
    }
    body.extend_from_slice(chunk);
    Ok(())
}

async fn classify_refusal(response: Response) -> MediaResult<Reply> {
    let status = response.status();
    let body = read_capped(response, REFUSAL_BODY_MAX_BYTES)
        .await
        .unwrap_or_default();
    if refuses_access_token(status, &body) {
        return Ok(Reply::TokenRefused);
    }
    tracing::debug!("media request answered {status}");
    Err(MediaFailure::Download)
}

fn refuses_access_token(status: StatusCode, body: &[u8]) -> bool {
    http::Response::builder()
        .status(status)
        .body(body)
        .is_ok_and(|response| {
            matches!(
                ServerError::from_http_response(response).error_kind(),
                Some(ErrorKind::UnknownToken(_))
            )
        })
}

fn decrypt(ciphertext: Vec<u8>, file: &EncryptedFile) -> MediaResult<Vec<u8>> {
    let mut plaintext = Vec::with_capacity(ciphertext.len());
    let mut cursor = Cursor::new(ciphertext);
    let mut decryptor = AttachmentDecryptor::new(&mut cursor, file.clone().into())
        .map_err(|e| download_failed("encrypted media carries unusable keys", &e))?;
    decryptor
        .read_to_end(&mut plaintext)
        .map_err(|e| download_failed("encrypted media failed decryption", &e))?;
    Ok(plaintext)
}

fn invalid_uri(uri: &MxcUri, error: &dyn Display) -> MediaFailure {
    download_failed(&format!("media uri {uri} is not usable"), error)
}

fn download_failed(context: &str, error: &dyn Display) -> MediaFailure {
    tracing::debug!("{context}: {error}");
    MediaFailure::Download
}
