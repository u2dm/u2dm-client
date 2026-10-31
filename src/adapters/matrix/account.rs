use std::sync::Arc;

use async_trait::async_trait;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::ruma::api::MatrixVersion;
use matrix_sdk::ruma::api::client::account::{change_password, deactivate};
use matrix_sdk::ruma::api::client::device::{delete_devices, get_devices, update_device};
use matrix_sdk::ruma::api::client::discovery::get_authorization_server_metadata::v1::{
    AccountManagementActionData, AuthorizationServerMetadata, DeviceDeleteData,
};
use matrix_sdk::ruma::api::client::discovery::get_capabilities;
use matrix_sdk::ruma::{OwnedDeviceId, OwnedUserId};
use matrix_sdk::{Client, HttpError};
use tokio::sync::{Notify, mpsc};
use url::Url;

use super::session::ClientHandle;
use super::uia::{answered, guarded, refused_as_weak};
use super::user_info::{device_trust, next_wake, own_user, trust_wakes};
use crate::domain::capabilities::{
    AVATAR_FIELD, AccountApi, AccountFacts, AccountPage, Capabilities, DISPLAY_NAME_FIELD,
    ProfileFields,
};
use crate::domain::link::LauncherSafeUrl;
use crate::domain::sessions::{LogoutRisks, OwnSession, OwnSessions, ThisSession};
use crate::domain::uia::{AccountPassword, AuthAnswer, Guarded};
use crate::domain::user_info::DeviceTrust;
use crate::error::{AppError, Result};
use crate::ports::matrix::AccountPort;

pub(super) struct MatrixAccount {
    matrix: Arc<ClientHandle>,
    changed: Notify,
}

impl MatrixAccount {
    pub(super) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self {
            matrix,
            changed: Notify::new(),
        }
    }
}

#[async_trait]
impl AccountPort for MatrixAccount {
    async fn facts(&self) -> AccountFacts {
        let Ok(client) = self.matrix.client().await else {
            return AccountFacts::default();
        };
        let facts = AccountFacts {
            api: account_api(&client).await,
            capabilities: capabilities(&client).await,
        };
        tracing::info!(
            api = ?facts.api,
            change_password = facts.capabilities.change_password,
            display_name = facts.capabilities.profile_fields.can_set(DISPLAY_NAME_FIELD),
            avatar = facts.capabilities.profile_fields.can_set(AVATAR_FIELD),
            default_room_version = %facts.capabilities.default_room_version,
            forget_forced_upon_leave = facts.capabilities.forget_forced_upon_leave,
            "read how the homeserver manages this account"
        );
        facts
    }

    async fn follow_sessions(&self, sessions_tx: mpsc::Sender<OwnSessions>) -> Result<()> {
        let client = self.matrix.client().await?;
        let own = own_user(&client)?;
        let mut wakes = trust_wakes(&client, [own.clone(), own.clone()]).await?;
        let mut reported = None;
        loop {
            match own_sessions(&client, &own).await {
                Ok(sessions) if reported.as_ref() != Some(&sessions) => {
                    tracing::debug!(
                        others = sessions.others.len(),
                        "read this account's sessions"
                    );
                    if sessions_tx.send(sessions.clone()).await.is_err() {
                        return Ok(());
                    }
                    reported = Some(sessions);
                }
                Ok(_) => {}
                Err(e) => tracing::debug!("could not read this account's sessions: {e}"),
            }
            tokio::select! {
                woke = next_wake(&mut wakes) => {
                    if !woke {
                        return Ok(());
                    }
                }
                () = self.changed.notified() => {}
            }
        }
    }

    async fn rename_session(&self, device_id: &str, name: &str) -> Result<()> {
        let client = self.matrix.client().await?;
        let mut request = update_device::v3::Request::new(OwnedDeviceId::from(device_id));
        request.display_name = Some(name.to_owned());
        client
            .send(request)
            .with_request_config(RequestConfig::short_retry())
            .await
            .map_err(|e| AppError::Other(format!("could not rename the session: {e}")))?;
        self.changed.notify_one();
        Ok(())
    }

    async fn sign_out_session(
        &self,
        device_id: &str,
        auth: Option<&AuthAnswer>,
    ) -> Result<Guarded> {
        let client = self.matrix.client().await?;
        let mut request = delete_devices::v3::Request::new(vec![OwnedDeviceId::from(device_id)]);
        request.auth = answered(&client, auth)?;
        let sent = client
            .send(request)
            .with_request_config(RequestConfig::short_retry())
            .await;
        let outcome = settle(&client, sent.err(), "sign the session out");
        self.changed.notify_one();
        outcome
    }

    async fn change_password(
        &self,
        password: &AccountPassword,
        sign_out_others: bool,
        auth: Option<&AuthAnswer>,
    ) -> Result<Guarded> {
        let client = self.matrix.client().await?;
        let mut request = change_password::v3::Request::new(password.expose().to_owned());
        request.logout_devices = sign_out_others;
        request.auth = answered(&client, auth)?;
        let sent = client
            .send(request)
            .with_request_config(RequestConfig::short_retry())
            .await;
        let refused = sent.err();
        if let Some(weak) = refused.as_ref().and_then(refused_as_weak) {
            return Err(weak);
        }
        let outcome = settle(&client, refused, "change the password");
        self.changed.notify_one();
        outcome
    }

    async fn deactivate(&self, erase: bool, auth: Option<&AuthAnswer>) -> Result<Guarded> {
        let client = self.matrix.client().await?;
        let mut request = deactivate::v3::Request::new();
        request.erase = erase;
        request.auth = answered(&client, auth)?;
        let sent = client
            .send(request)
            .with_request_config(RequestConfig::short_retry().disable_retry())
            .await;
        settle(&client, sent.err(), "deactivate the account")
    }

    async fn page(&self, page: &AccountPage) -> Option<LauncherSafeUrl> {
        let client = self.matrix.client().await.ok()?;
        let metadata = match client.oauth().cached_server_metadata().await {
            Ok(metadata) => metadata,
            Err(e) => {
                tracing::warn!("could not read the homeserver's account management address: {e}");
                return None;
            }
        };
        let url = page_url(&metadata, page)?;
        match LauncherSafeUrl::sign_in_page(url) {
            Ok(url) => Some(url),
            Err(refused) => {
                tracing::warn!("not opening the account page: {refused}");
                None
            }
        }
    }

    async fn logout_risks(&self) -> LogoutRisks {
        let Ok(client) = self.matrix.client().await else {
            return LogoutRisks::default();
        };
        LogoutRisks {
            keys_at_risk: keys_at_risk(&client).await,
            unsent: unsent_sends(&client).await,
        }
    }
}

fn settle(client: &Client, refused: Option<HttpError>, what: &str) -> Result<Guarded> {
    match refused {
        None => Ok(Guarded::Done),
        Some(error) => guarded(client, &error)
            .ok_or_else(|| AppError::Other(format!("could not {what}: {error}"))),
    }
}

fn page_url(metadata: &AuthorizationServerMetadata, page: &AccountPage) -> Option<Url> {
    let device;
    let action = match page {
        AccountPage::Account => return metadata.account_management_uri.clone(),
        AccountPage::Sessions => AccountManagementActionData::DevicesList,
        AccountPage::SignOut(device_id) => {
            device = OwnedDeviceId::from(device_id.as_str());
            AccountManagementActionData::DeviceDelete(DeviceDeleteData::new(&device))
        }
        AccountPage::Deactivate => AccountManagementActionData::AccountDeactivate,
    };
    metadata.account_management_url_with_action(action)
}

async fn account_api(client: &Client) -> AccountApi {
    match client.oauth().cached_server_metadata().await {
        Ok(metadata) => AccountApi::OAuth {
            pages: metadata.account_management_uri.is_some(),
        },
        Err(e) if e.is_not_supported() => AccountApi::Legacy,
        Err(e) => {
            tracing::warn!("could not ask whether the homeserver uses OAuth 2.0: {e}");
            if client.oauth().client_id().is_some() {
                AccountApi::OAuth { pages: false }
            } else {
                AccountApi::Legacy
            }
        }
    }
}

async fn capabilities(client: &Client) -> Capabilities {
    let response = client
        .send(get_capabilities::v3::Request::new())
        .with_request_config(RequestConfig::short_retry())
        .await;
    let capabilities = match response {
        Ok(response) => response.capabilities,
        Err(e) => {
            tracing::warn!("could not read the homeserver's capabilities: {e}");
            return Capabilities::default();
        }
    };
    let profile_fields = match &capabilities.profile_fields {
        Some(fields) => ProfileFields::new(
            fields.enabled,
            fields.allowed.as_ref().map(|allowed| {
                allowed
                    .iter()
                    .map(|field| field.as_str().to_owned())
                    .collect()
            }),
            fields
                .disallowed
                .iter()
                .flatten()
                .map(|field| field.as_str().to_owned())
                .collect(),
        ),
        None => legacy_profile_fields(&capabilities, speaks_profile_fields(client).await),
    };
    Capabilities {
        change_password: capabilities.change_password.enabled,
        profile_fields,
        default_room_version: capabilities.room_versions.default.to_string(),
        forget_forced_upon_leave: capabilities.forget_forced_upon_leave.enabled,
    }
}

async fn speaks_profile_fields(client: &Client) -> bool {
    match client.server_versions().await {
        Ok(versions) => versions
            .iter()
            .any(|version| *version >= MatrixVersion::V1_16),
        Err(e) => {
            tracing::debug!("could not read the homeserver's versions: {e}");
            false
        }
    }
}

#[allow(deprecated)]
fn legacy_profile_fields(
    capabilities: &get_capabilities::v3::Capabilities,
    speaks_profile_fields: bool,
) -> ProfileFields {
    let fields = [
        (DISPLAY_NAME_FIELD, capabilities.set_displayname.enabled),
        (AVATAR_FIELD, capabilities.set_avatar_url.enabled),
    ];
    if speaks_profile_fields {
        let refused = fields
            .into_iter()
            .filter(|(_, enabled)| !enabled)
            .map(|(field, _)| field.to_owned())
            .collect();
        return ProfileFields::new(true, None, refused);
    }
    ProfileFields::only(
        fields
            .into_iter()
            .filter_map(|(field, enabled)| enabled.then_some(field)),
    )
}

async fn own_sessions(client: &Client, own: &OwnedUserId) -> Result<OwnSessions> {
    let listed = client
        .send(get_devices::v3::Request::new())
        .with_request_config(RequestConfig::short_retry())
        .await
        .map_err(|e| AppError::Other(format!("could not list the sessions: {e}")))?;
    let crypto = client
        .encryption()
        .get_user_devices(own)
        .await
        .map_err(AppError::other)?;
    let this_device = client
        .device_id()
        .map(ToString::to_string)
        .ok_or_else(|| AppError::Other("the session has no device id".to_owned()))?;
    let key = client.encryption().ed25519_key().await;
    let mut this = ThisSession {
        device_id: this_device.clone(),
        name: None,
        key,
    };
    let mut others = Vec::with_capacity(listed.devices.len());
    for device in listed.devices {
        if device.device_id.as_str() == this_device {
            this.name = device.display_name;
            continue;
        }
        let known = crypto.get(&device.device_id);
        if known.as_ref().is_some_and(|known| known.is_dehydrated()) {
            continue;
        }
        others.push(OwnSession {
            device_id: device.device_id.to_string(),
            name: device.display_name,
            last_seen_ms: device.last_seen_ts.map(|at| u64::from(at.get())),
            last_seen_ip: device.last_seen_ip,
            trust: known
                .filter(|known| !known.is_deleted())
                .map_or(DeviceTrust::NotSignedByOwner, |known| device_trust(&known)),
        });
    }
    Ok(OwnSessions::new(this, others))
}

async fn keys_at_risk(client: &Client) -> bool {
    if client.encryption().backups().are_enabled().await {
        return false;
    }
    let Ok(own) = own_user(client) else {
        return true;
    };
    let this = client.device_id().map(ToOwned::to_owned);
    match client.encryption().get_user_devices(&own).await {
        Ok(devices) => !devices.devices().any(|device| {
            Some(device.device_id()) != this.as_deref()
                && !device.is_deleted()
                && !device.is_dehydrated()
                && device.is_cross_signed_by_owner()
        }),
        Err(e) => {
            tracing::debug!("could not list this account's other sessions: {e}");
            true
        }
    }
}

async fn unsent_sends(client: &Client) -> usize {
    match client.send_queue().local_echoes().await {
        Ok(echoes) => echoes.values().map(Vec::len).sum(),
        Err(e) => {
            tracing::debug!("could not read the send queue: {e}");
            0
        }
    }
}
