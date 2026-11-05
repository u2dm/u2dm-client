use serde::Serialize;

use super::{MessageDto, message};
use crate::adapters::demo::probe::names;
use crate::adapters::ui::identity_banner;
use crate::commands::view::{
    AccountStep, AccountView, AuthPrompt, EncryptionStep, EncryptionView, IdentityAlertView,
    LifecycleView, LoginStep, LogoutView,
};
use crate::domain::capabilities::{AVATAR_FIELD, AccountApi, AccountFacts, DISPLAY_NAME_FIELD};
use crate::domain::encryption::KeyStorage;
use crate::domain::sessions::OwnSessions;
use crate::domain::sync::{ConnectionStatus, ReauthReason};

#[derive(Serialize)]
pub(super) struct IdentityAlertDto {
    kind: &'static str,
    user_id: Option<String>,
    name: Option<String>,
    busy: bool,
}

#[derive(Serialize)]
pub(super) struct EncryptionDto {
    standing: &'static str,
    verified_sessions: bool,
    recovery: &'static str,
    key_storage: &'static str,
    open: bool,
    step: Option<&'static str>,
    activity: Option<&'static str>,
    recovery_key: Option<String>,
    key_file: Option<String>,
    keys_exported: Option<usize>,
    keys_imported: Option<(usize, usize)>,
    auth: Option<AuthDto>,
    error: Option<MessageDto>,
}

#[derive(Serialize)]
struct AuthDto {
    stage: &'static str,
    error: MessageDto,
}

#[derive(Serialize)]
pub(super) struct AccountDto {
    step: &'static str,
    activity: &'static str,
    busy_session: Option<String>,
    auth: Option<AuthDto>,
    auth_for: Option<&'static str>,
    this_session: Option<ThisSessionDto>,
    sessions: Option<Vec<OwnSessionDto>>,
    facts: Option<AccountFactsDto>,
    error: MessageDto,
    notice: MessageDto,
}

#[derive(Serialize)]
struct ThisSessionDto {
    device_id: String,
    name: Option<String>,
    fingerprint: Option<String>,
}

#[derive(Serialize)]
struct OwnSessionDto {
    device_id: String,
    name: Option<String>,
    last_seen_ms: Option<u64>,
    last_seen_ip: Option<String>,
    trust: &'static str,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct AccountFactsDto {
    api: &'static str,
    pages: bool,
    change_password: bool,
    display_name_settable: bool,
    avatar_settable: bool,
    default_room_version: String,
    forget_forced_upon_leave: bool,
}

#[derive(Serialize)]
pub(super) struct LogoutDto {
    keys_at_risk: bool,
    unsent: usize,
}

#[derive(Serialize)]
pub(super) struct LifecycleDto {
    step: &'static str,
    reauth_reason: Option<&'static str>,
    activity: &'static str,
    method: &'static str,
    resolved_homeserver: String,
    user_id: String,
    has_avatar: bool,
    messages: Vec<MessageDto>,
}

#[derive(Serialize)]
pub(super) struct ConnectionDto {
    status: &'static str,
    detail: Option<String>,
}

pub(super) fn identity_alert(source: &IdentityAlertView) -> IdentityAlertDto {
    let banner = identity_banner(source.alert.as_ref());
    IdentityAlertDto {
        kind: names::identity_alert_kind(banner.kind),
        user_id: (!banner.user_id.is_empty()).then(|| banner.user_id.to_owned()),
        name: (!banner.name.is_empty()).then_some(banner.name),
        busy: source.busy,
    }
}

pub(super) fn encryption(source: &EncryptionView) -> EncryptionDto {
    let dialog = source.dialog.as_ref();
    EncryptionDto {
        standing: names::encryption_standing(source.standing),
        verified_sessions: source.verified_sessions,
        recovery: names::recovery_status(source.recovery),
        key_storage: match source.key_storage {
            KeyStorage::Allowed => "allowed",
            KeyStorage::TurnedOff => "turned-off",
        },
        open: dialog.is_some(),
        step: dialog.map(|dialog| names::encryption_step(&dialog.step)),
        activity: dialog.map(|dialog| names::encryption_activity(dialog.activity)),
        recovery_key: dialog.and_then(|dialog| match &dialog.step {
            EncryptionStep::ShowingKey(key) => Some(key.expose().to_owned()),
            _ => None,
        }),
        key_file: dialog.and_then(|dialog| match &dialog.step {
            EncryptionStep::ExportingKeys(file)
            | EncryptionStep::ImportingKeys(file)
            | EncryptionStep::KeysExported { file, .. } => Some(file.clone()),
            _ => None,
        }),
        keys_exported: dialog.and_then(|dialog| match &dialog.step {
            EncryptionStep::KeysExported { count, .. } => Some(*count),
            _ => None,
        }),
        keys_imported: dialog.and_then(|dialog| match &dialog.step {
            EncryptionStep::KeysImported { imported, total } => Some((*imported, *total)),
            _ => None,
        }),
        auth: dialog.and_then(|dialog| match &dialog.step {
            EncryptionStep::AuthenticatingSetup(prompt)
            | EncryptionStep::AuthenticatingReset(prompt) => Some(auth(prompt)),
            _ => None,
        }),
        error: dialog.map(|dialog| message(&dialog.error)),
    }
}

fn auth(prompt: &AuthPrompt) -> AuthDto {
    AuthDto {
        stage: names::auth_stage(prompt.stage),
        error: message(&prompt.error),
    }
}

pub(super) fn account(source: &AccountView) -> Option<AccountDto> {
    let dialog = source.dialog.as_ref()?;
    Some(AccountDto {
        step: names::account_step(&dialog.step),
        activity: names::account_activity(dialog.activity),
        busy_session: dialog.busy_session.clone(),
        auth: match &dialog.step {
            AccountStep::Authenticating(account_auth) => Some(auth(&account_auth.prompt)),
            _ => None,
        },
        auth_for: match &dialog.step {
            AccountStep::Authenticating(account_auth) => {
                Some(names::account_activity(account_auth.guarding))
            }
            _ => None,
        },
        this_session: dialog.sessions.as_ref().map(|sessions| ThisSessionDto {
            device_id: sessions.this.device_id.clone(),
            name: sessions.this.name.clone(),
            fingerprint: sessions.this.fingerprint(),
        }),
        sessions: dialog.sessions.as_ref().map(own_sessions),
        facts: dialog.facts.as_ref().map(account_facts),
        error: message(&dialog.error),
        notice: message(&dialog.notice),
    })
}

fn own_sessions(sessions: &OwnSessions) -> Vec<OwnSessionDto> {
    sessions
        .others
        .iter()
        .map(|session| OwnSessionDto {
            device_id: session.device_id.clone(),
            name: session.name.clone(),
            last_seen_ms: session.last_seen_ms,
            last_seen_ip: session.last_seen_ip.clone(),
            trust: names::device_trust(session.trust),
        })
        .collect()
}

fn account_facts(facts: &AccountFacts) -> AccountFactsDto {
    let capabilities = &facts.capabilities;
    AccountFactsDto {
        api: match facts.api {
            AccountApi::Legacy => "legacy",
            AccountApi::OAuth { .. } => "oauth",
        },
        pages: matches!(facts.api, AccountApi::OAuth { pages: true }),
        change_password: capabilities.change_password,
        display_name_settable: capabilities.profile_fields.can_set(DISPLAY_NAME_FIELD),
        avatar_settable: capabilities.profile_fields.can_set(AVATAR_FIELD),
        default_room_version: capabilities.default_room_version.clone(),
        forget_forced_upon_leave: capabilities.forget_forced_upon_leave,
    }
}

pub(super) fn logout(source: LogoutView) -> Option<LogoutDto> {
    source.warning.map(|risks| LogoutDto {
        keys_at_risk: risks.keys_at_risk,
        unsent: risks.unsent,
    })
}

pub(super) fn lifecycle(source: &LifecycleView) -> LifecycleDto {
    LifecycleDto {
        step: names::login_step(source.step),
        reauth_reason: reauth_reason(source.step),
        activity: names::login_activity(source.activity),
        method: names::login_method(source.method),
        resolved_homeserver: source.resolved_homeserver.clone(),
        user_id: source.user_id.clone(),
        has_avatar: source.avatar_path.is_some(),
        messages: source.messages.iter().map(message).collect(),
    }
}

fn reauth_reason(step: LoginStep) -> Option<&'static str> {
    let LoginStep::Reauthenticate(reason) = step else {
        return None;
    };
    Some(match reason {
        ReauthReason::SoftLogout => "soft-logout",
        ReauthReason::Refused => "refused",
    })
}

pub(super) fn connection(source: &ConnectionStatus) -> ConnectionDto {
    ConnectionDto {
        status: names::connection_status(source),
        detail: match source {
            ConnectionStatus::Error(detail) => Some(detail.clone()),
            _ => None,
        },
    }
}
