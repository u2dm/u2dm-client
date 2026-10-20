use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{AuthPrompt, AuthStage};
use crate::domain::link::LauncherSafeUrl;
use crate::domain::uia::{AccountPassword, AuthAnswer, AuthChallenge, AuthResponse, AuthStep};

pub(super) enum Reply {
    Password(AccountPassword),
    Continue,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sent {
    Nothing,
    Password,
    Acknowledged,
    Dummy,
}

pub(super) struct AuthRound {
    challenge: AuthChallenge,
    step: AuthStep,
    sent: Sent,
    error: UserMessage,
}

pub(super) enum Next {
    Ask {
        round: AuthRound,
        open_page: bool,
    },
    Answer {
        round: AuthRound,
        answer: AuthAnswer,
    },
    Unsupported,
}

impl AuthRound {
    pub(super) fn after(challenge: AuthChallenge, previous: Option<&Self>) -> Next {
        let Some(step) = challenge.next_step() else {
            tracing::warn!(
                flows = ?challenge.flows,
                completed = ?challenge.completed,
                "the homeserver asks for no authentication U2DM can complete"
            );
            return Next::Unsupported;
        };
        let repeated = previous.filter(|previous| previous.step.stage() == step.stage());
        let sent = repeated.map_or(Sent::Nothing, |previous| previous.sent);
        tracing::debug!(
            stage = step.stage(),
            ?sent,
            "the homeserver asks for authentication"
        );
        if step == AuthStep::Dummy {
            if sent == Sent::Dummy {
                return Next::Unsupported;
            }
            let answer = challenge.answer(AuthResponse::Dummy);
            let round = Self {
                challenge,
                step,
                sent: Sent::Dummy,
                error: UserMessage::default(),
            };
            return Next::Answer { round, answer };
        }
        let error = match sent {
            Sent::Password if challenge.refused_password() => {
                UserMessage::new(UserMessageKind::AuthPasswordWrong)
            }
            Sent::Acknowledged => UserMessage::new(UserMessageKind::AuthStepUnfinished),
            Sent::Nothing | Sent::Password | Sent::Dummy => challenge
                .refusal
                .as_ref()
                .map(|refusal| UserMessage::about(UserMessageKind::AuthRefused, &refusal.message))
                .unwrap_or_default(),
        };
        let open_page = step.page().is_some() && repeated.is_none();
        Next::Ask {
            round: Self {
                challenge,
                step,
                sent: Sent::Nothing,
                error,
            },
            open_page,
        }
    }

    pub(super) fn reply(&mut self, reply: Reply) -> Option<AuthAnswer> {
        let (sent, response) = match (reply, &self.step) {
            (Reply::Password(password), AuthStep::Password) => {
                (Sent::Password, AuthResponse::Password(password))
            }
            (Reply::Continue, AuthStep::Approve(_) | AuthStep::Fallback { .. }) => {
                (Sent::Acknowledged, AuthResponse::Acknowledged)
            }
            (Reply::Password(_) | Reply::Continue, _) => return None,
        };
        self.sent = sent;
        self.error = UserMessage::default();
        Some(self.challenge.answer(response))
    }

    pub(super) fn is_silent(&self) -> bool {
        self.step == AuthStep::Dummy
    }

    pub(super) fn page(&self) -> Option<&LauncherSafeUrl> {
        self.step.page()
    }

    pub(super) fn prompt(&self) -> AuthPrompt {
        AuthPrompt {
            stage: match self.step {
                AuthStep::Password | AuthStep::Dummy => AuthStage::Password,
                AuthStep::Approve(_) => AuthStage::Approve,
                AuthStep::Fallback { .. } => AuthStage::Browser,
            },
            error: self.error.clone(),
        }
    }
}
