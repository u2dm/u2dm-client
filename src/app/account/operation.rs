use super::Step;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{AccountActivity, AccountStep};
use crate::domain::uia::{AccountPassword, AuthAnswer, Guarded};
use crate::error::{AppError, Result as AppResult};
use crate::ports::matrix::AccountPort;

#[derive(Clone)]
pub(super) enum Operation {
    SignOut {
        device_id: String,
        name: String,
    },
    ChangePassword {
        password: AccountPassword,
        sign_out_others: bool,
    },
    Deactivate {
        erase: bool,
    },
}

impl Operation {
    pub(super) fn activity(&self) -> AccountActivity {
        match self {
            Self::SignOut { .. } => AccountActivity::SigningOut,
            Self::ChangePassword { .. } => AccountActivity::ChangingPassword,
            Self::Deactivate { .. } => AccountActivity::Deactivating,
        }
    }

    pub(super) fn origin(&self) -> Step {
        match self {
            Self::SignOut { .. } => Step::Overview,
            Self::ChangePassword { .. } => Step::ChangingPassword,
            Self::Deactivate { .. } => Step::ConfirmingDeactivation,
        }
    }

    pub(super) fn origin_step(&self) -> AccountStep {
        match self {
            Self::SignOut { .. } => AccountStep::Overview,
            Self::ChangePassword { .. } => AccountStep::ChangingPassword,
            Self::Deactivate { .. } => AccountStep::ConfirmingDeactivation,
        }
    }

    pub(super) fn subject(&self) -> String {
        match self {
            Self::SignOut { name, .. } => name.clone(),
            Self::ChangePassword { .. } | Self::Deactivate { .. } => String::new(),
        }
    }

    pub(super) fn device(&self) -> Option<&str> {
        match self {
            Self::SignOut { device_id, .. } => Some(device_id),
            Self::ChangePassword { .. } | Self::Deactivate { .. } => None,
        }
    }

    pub(super) fn failure(&self, error: &AppError) -> UserMessage {
        match (self, error) {
            (Self::ChangePassword { .. }, AppError::WeakPassword(reason)) => {
                UserMessage::about(UserMessageKind::NewPasswordRefused, reason)
            }
            (Self::SignOut { name, .. }, _) => {
                UserMessage::about(UserMessageKind::SessionSignOutFailed, name)
            }
            (Self::ChangePassword { .. }, _) => {
                UserMessage::new(UserMessageKind::PasswordChangeFailed)
            }
            (Self::Deactivate { .. }, _) => UserMessage::new(UserMessageKind::DeactivationFailed),
        }
    }

    pub(super) async fn run(
        &self,
        port: &dyn AccountPort,
        auth: Option<&AuthAnswer>,
    ) -> AppResult<Guarded> {
        match self {
            Self::SignOut { device_id, .. } => port.sign_out_session(device_id, auth).await,
            Self::ChangePassword {
                password,
                sign_out_others,
            } => port.change_password(password, *sign_out_others, auth).await,
            Self::Deactivate { erase } => port.deactivate(*erase, auth).await,
        }
    }
}
