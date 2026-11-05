use slint::SharedString;

use crate::adapters::ui::backend::{UiBackend, UiEventContext};
use crate::adapters::ui::decode::{AvatarSlot, load_avatar_async};
use crate::adapters::ui::present::{VerifyStep, user_initial, verification_cancellation};
use crate::adapters::ui::props::{BoolProp, ImageProp, IntProp, MessageProp, StringProp, UiProps};
use crate::adapters::ui::richtext::recognise_own_user;
use crate::commands::effects::{VerificationActivity, VerificationUpdate};
use crate::commands::messages::UserMessage;
use crate::commands::view::{
    AccountAuth, AccountDialog, AccountStep, AccountView, AuthPrompt, EncryptionDialog,
    EncryptionStep, EncryptionView, LifecycleView, LoginStep, LogoutView,
};
use crate::domain::capabilities::{AVATAR_FIELD, AccountApi, AccountFacts, DISPLAY_NAME_FIELD};
use crate::domain::encryption::KeyStorage;
use crate::domain::sessions::{OwnSessions, ThisSession};
use crate::domain::sync::ReauthReason;
use crate::domain::verification::VerificationEvent as DomainVerificationEvent;
use crate::util::saturating_i32;

pub(super) fn apply_lifecycle(
    w: &impl UiProps,
    last: Option<&LifecycleView>,
    next: &LifecycleView,
) {
    let LifecycleView {
        step,
        activity,
        messages,
        method,
        resolved_homeserver,
        user_id,
        device_id: _,
        avatar_path,
    } = next;

    if last.is_none_or(|l| l.step != *step) {
        w.set_login_phase(*step);
        w.set_bool(
            BoolProp::SessionRefused,
            matches!(step, LoginStep::Reauthenticate(ReauthReason::Refused)),
        );
    }
    if last.is_none_or(|l| l.activity != *activity) {
        w.set_login_activity(*activity);
    }
    if last.is_none_or(|l| l.messages != *messages) {
        w.apply_login_messages(messages);
    }
    if last.is_none_or(|l| l.method != *method) {
        w.set_login_method_kind(*method);
    }
    if last.is_none_or(|l| l.resolved_homeserver != *resolved_homeserver) {
        w.set_string(
            StringProp::ResolvedHomeserver,
            SharedString::from(resolved_homeserver),
        );
    }
    if last.is_none_or(|l| l.user_id != *user_id) {
        recognise_own_user(user_id);
        w.set_string(StringProp::UserId, SharedString::from(user_id));
        w.set_string(
            StringProp::UserInitial,
            SharedString::from(user_initial(user_id)),
        );
    }
    if last.is_none_or(|l| l.avatar_path != *avatar_path) {
        let avatar = load_avatar_async(avatar_path.as_deref(), AvatarSlot::User);
        w.set_image(ImageProp::UserAvatar, avatar);
    }
}

pub(super) fn apply_encryption(
    w: &impl UiProps,
    last: Option<&EncryptionView>,
    view: &EncryptionView,
) {
    if last == Some(view) {
        return;
    }
    w.set_encryption_standing(view.standing);
    w.set_bool(BoolProp::EncryptionVerifiedSessions, view.verified_sessions);
    w.set_encryption_recovery(view.recovery);
    w.set_bool(
        BoolProp::EncryptionKeyStorageOff,
        view.key_storage == KeyStorage::TurnedOff,
    );
    let closed = EncryptionDialog::default();
    let dialog = view.dialog.as_ref().unwrap_or(&closed);
    w.set_bool(BoolProp::EncryptionVisible, view.dialog.is_some());
    w.set_encryption_step(&dialog.step);
    w.set_encryption_activity(dialog.activity);
    w.set_message(MessageProp::EncryptionError, &dialog.error);
    let shown_key = if let EncryptionStep::ShowingKey(key) = &dialog.step {
        key.expose()
    } else {
        ""
    };
    w.set_string(
        StringProp::EncryptionRecoveryKey,
        SharedString::from(shown_key),
    );
    let (key_file, keys_count, keys_total) = match &dialog.step {
        EncryptionStep::ExportingKeys(file) | EncryptionStep::ImportingKeys(file) => {
            (file.as_str(), 0, 0)
        }
        EncryptionStep::KeysExported { count, file } => (file.as_str(), *count, 0),
        EncryptionStep::KeysImported { imported, total } => ("", *imported, *total),
        _ => ("", 0, 0),
    };
    w.set_string(StringProp::EncryptionKeyFile, SharedString::from(key_file));
    w.set_int(IntProp::EncryptionKeysCount, saturating_i32(keys_count));
    w.set_int(IntProp::EncryptionKeysTotal, saturating_i32(keys_total));
    let closed_prompt = AuthPrompt::default();
    let prompt = match &dialog.step {
        EncryptionStep::AuthenticatingSetup(prompt)
        | EncryptionStep::AuthenticatingReset(prompt) => prompt,
        _ => &closed_prompt,
    };
    w.set_encryption_auth_stage(prompt.stage);
    w.set_message(MessageProp::EncryptionAuthError, &prompt.error);
}

pub(super) fn apply_account<B: UiBackend>(
    w: &B::Window,
    last: Option<&AccountView>,
    view: &AccountView,
    ctx: &UiEventContext<'_, B>,
) {
    if last == Some(view) {
        return;
    }
    let closed = AccountDialog::default();
    let dialog = view.dialog.as_ref().unwrap_or(&closed);
    let last_dialog = last.and_then(|last| last.dialog.as_ref());
    w.set_bool(BoolProp::AccountVisible, view.dialog.is_some());
    w.set_account_step(&dialog.step);
    w.set_account_activity(dialog.activity);
    w.set_string(
        StringProp::AccountBusySession,
        dialog
            .busy_session
            .as_deref()
            .map_or_else(SharedString::new, SharedString::from),
    );
    w.set_message(MessageProp::AccountError, &dialog.error);
    w.set_message(MessageProp::AccountNotice, &dialog.notice);
    let closed_auth = AccountAuth::default();
    let auth = match &dialog.step {
        AccountStep::Authenticating(auth) => auth,
        _ => &closed_auth,
    };
    w.set_account_auth_stage(auth.prompt.stage);
    w.set_message(MessageProp::AccountAuthError, &auth.prompt.error);
    w.set_account_auth_for(auth.guarding);
    w.set_string(
        StringProp::AccountAuthSubject,
        SharedString::from(&auth.subject),
    );
    if last_dialog.is_none_or(|last| last.sessions != dialog.sessions) {
        apply_own_sessions::<B>(w, dialog.sessions.as_ref(), ctx);
    }
    if last_dialog.is_none_or(|last| last.facts != dialog.facts) {
        apply_account_facts(w, dialog.facts.as_ref());
    }
}

fn apply_own_sessions<B: UiBackend>(
    w: &B::Window,
    sessions: Option<&OwnSessions>,
    ctx: &UiEventContext<'_, B>,
) {
    w.set_bool(BoolProp::AccountSessionsKnown, sessions.is_some());
    let this = sessions.map(|sessions| &sessions.this);
    w.set_string(
        StringProp::AccountThisDeviceId,
        this.map_or_else(SharedString::new, |this| {
            SharedString::from(this.device_id.as_str())
        }),
    );
    w.set_string(
        StringProp::AccountThisName,
        this.and_then(|this| this.name.as_deref())
            .map_or_else(SharedString::new, SharedString::from),
    );
    w.set_string(
        StringProp::AccountThisKey,
        this.and_then(ThisSession::fingerprint)
            .map_or_else(SharedString::new, SharedString::from),
    );
    let rows = sessions
        .iter()
        .flat_map(|sessions| sessions.others.iter())
        .map(B::convert_own_session)
        .collect::<Vec<_>>();
    ctx.models.account_sessions.set_vec(rows);
}

fn apply_account_facts(w: &impl UiProps, facts: Option<&AccountFacts>) {
    let shown = facts.cloned().unwrap_or_default();
    let capabilities = &shown.capabilities;
    w.set_bool(BoolProp::AccountFactsKnown, facts.is_some());
    w.set_bool(BoolProp::AccountOAuth, shown.api.is_oauth());
    w.set_bool(
        BoolProp::AccountHasPages,
        matches!(shown.api, AccountApi::OAuth { pages: true }),
    );
    w.set_bool(
        BoolProp::AccountMayChangePassword,
        capabilities.change_password,
    );
    w.set_bool(
        BoolProp::AccountNameManaged,
        !capabilities.profile_fields.can_set(DISPLAY_NAME_FIELD),
    );
    w.set_bool(
        BoolProp::AccountAvatarManaged,
        !capabilities.profile_fields.can_set(AVATAR_FIELD),
    );
    w.set_bool(
        BoolProp::AccountForgetsRooms,
        capabilities.forget_forced_upon_leave,
    );
    w.set_string(
        StringProp::AccountDefaultRoomVersion,
        SharedString::from(&capabilities.default_room_version),
    );
}

pub(super) fn apply_logout(w: &impl UiProps, last: Option<&LogoutView>, view: &LogoutView) {
    if last == Some(view) {
        return;
    }
    let risks = view.warning.unwrap_or_default();
    w.set_bool(BoolProp::LogoutVisible, view.warning.is_some());
    w.set_bool(BoolProp::LogoutKeysAtRisk, risks.keys_at_risk);
    w.set_int(IntProp::LogoutUnsent, saturating_i32(risks.unsent));
}

pub(super) fn apply_verification(w: &impl UiProps, update: &VerificationUpdate) {
    match update {
        VerificationUpdate::Flow(event) => apply_verification_flow(w, event),
        VerificationUpdate::Busy(activity) => w.set_verification_activity(*activity),
        VerificationUpdate::Failed(message) => {
            w.set_verification_activity(VerificationActivity::None);
            w.set_message(MessageProp::VerificationError, message);
        }
        VerificationUpdate::Dismissed => reset_verification(w),
    }
}

fn apply_verification_flow(w: &impl UiProps, event: &DomainVerificationEvent) {
    w.set_verification_activity(VerificationActivity::None);
    match event {
        DomainVerificationEvent::Requested { sender, is_self } => {
            open_verification(w, VerifyStep::Requested, sender, *is_self);
        }
        DomainVerificationEvent::RequestSent { to, is_self } => {
            open_verification(w, VerifyStep::Waiting, to, *is_self);
        }
        DomainVerificationEvent::Emojis(emojis) => {
            w.set_verification_phase(VerifyStep::Emojis);
            w.apply_emoji_model(emojis);
        }
        DomainVerificationEvent::Confirming => {
            w.set_verification_phase(VerifyStep::Confirming);
        }
        DomainVerificationEvent::Done => {
            w.set_verification_phase(VerifyStep::Done);
        }
        DomainVerificationEvent::Cancelled(reason) => {
            w.set_verification_phase(VerifyStep::Cancelled);
            w.set_message(
                MessageProp::VerificationError,
                &verification_cancellation(reason),
            );
        }
    }
}

fn open_verification(w: &impl UiProps, step: VerifyStep, sender: &str, is_self: bool) {
    w.set_bool(BoolProp::VerificationVisible, true);
    w.set_verification_phase(step);
    w.set_string(StringProp::VerificationSender, SharedString::from(sender));
    w.set_bool(BoolProp::VerificationIsSelf, is_self);
    w.set_message(MessageProp::VerificationError, &UserMessage::default());
}

pub(super) fn reset_verification(w: &impl UiProps) {
    w.set_bool(BoolProp::VerificationVisible, false);
    w.set_verification_activity(VerificationActivity::None);
    w.set_verification_phase(VerifyStep::None);
    w.set_string(StringProp::VerificationSender, SharedString::default());
    w.set_bool(BoolProp::VerificationIsSelf, false);
    w.set_message(MessageProp::VerificationError, &UserMessage::default());
    w.clear_emoji_model();
}
