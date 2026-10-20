use std::fmt;

use url::Url;

use crate::domain::link::LauncherSafeUrl;

const PASSWORD_STAGE: &str = "m.login.password";
const DUMMY_STAGE: &str = "m.login.dummy";
pub const OAUTH_STAGE: &str = "m.oauth";
const FALLBACK_PATH_BEFORE_STAGE: [&str; 4] = ["_matrix", "client", "v3", "auth"];
const FALLBACK_PATH_AFTER_STAGE: [&str; 2] = ["fallback", "web"];
const SESSION_PARAMETER: &str = "session";
const WRONG_PASSWORD_CODE: &str = "M_FORBIDDEN";
pub const MIN_PASSWORD_CHARS: usize = 8;

#[derive(Clone, PartialEq, Eq)]
pub struct AccountPassword(String);

impl AccountPassword {
    pub fn new(password: impl Into<String>) -> Self {
        Self(password.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AccountPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccountPassword([REDACTED])")
    }
}

pub fn is_complex_enough(password: &AccountPassword) -> bool {
    let text = password.expose();
    text.chars().count() >= MIN_PASSWORD_CHARS
        && text.chars().any(char::is_lowercase)
        && text.chars().any(char::is_uppercase)
        && text.chars().any(char::is_numeric)
        && text.chars().any(|c| !c.is_alphanumeric())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRefusal {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthChallenge {
    pub session: Option<String>,
    pub flows: Vec<Vec<String>>,
    pub completed: Vec<String>,
    pub oauth_url: Option<String>,
    pub homeserver: String,
    pub refusal: Option<AuthRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStep {
    Password,
    Dummy,
    Approve(LauncherSafeUrl),
    Fallback {
        stage: String,
        page: LauncherSafeUrl,
    },
}

impl AuthStep {
    pub fn page(&self) -> Option<&LauncherSafeUrl> {
        match self {
            Self::Approve(page) | Self::Fallback { page, .. } => Some(page),
            Self::Password | Self::Dummy => None,
        }
    }

    pub fn stage(&self) -> &str {
        match self {
            Self::Password => PASSWORD_STAGE,
            Self::Dummy => DUMMY_STAGE,
            Self::Approve(_) => OAUTH_STAGE,
            Self::Fallback { stage, .. } => stage,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Effort {
    Nothing,
    Typing,
    Browser,
    UnknownStage,
}

impl AuthChallenge {
    pub fn next_step(&self) -> Option<AuthStep> {
        let mut best: Option<((Effort, usize), AuthStep)> = None;
        for flow in &self.flows {
            let Some(remaining) = flow.strip_prefix(self.completed.as_slice()) else {
                continue;
            };
            let Some(cost) = self.cost_of(remaining) else {
                continue;
            };
            let Some(step) = remaining.first().and_then(|stage| self.step_for(stage)) else {
                continue;
            };
            if best.as_ref().is_none_or(|(best_cost, _)| cost < *best_cost) {
                best = Some((cost, step));
            }
        }
        best.map(|(_, step)| step)
    }

    pub fn answer(&self, response: AuthResponse) -> AuthAnswer {
        AuthAnswer {
            session: self.session.clone(),
            response,
        }
    }

    pub fn refused_password(&self) -> bool {
        self.refusal
            .as_ref()
            .is_some_and(|refusal| refusal.code == WRONG_PASSWORD_CODE)
    }

    fn cost_of(&self, stages: &[String]) -> Option<(Effort, usize)> {
        let mut worst = Effort::Nothing;
        for stage in stages {
            worst = worst.max(self.effort(stage)?);
        }
        Some((worst, stages.len()))
    }

    fn effort(&self, stage: &str) -> Option<Effort> {
        match stage {
            DUMMY_STAGE => Some(Effort::Nothing),
            PASSWORD_STAGE => Some(Effort::Typing),
            OAUTH_STAGE => self.oauth_url.as_ref().map(|_| Effort::Browser),
            _ => self.session.as_ref().map(|_| Effort::UnknownStage),
        }
    }

    fn step_for(&self, stage: &str) -> Option<AuthStep> {
        match stage {
            DUMMY_STAGE => Some(AuthStep::Dummy),
            PASSWORD_STAGE => Some(AuthStep::Password),
            OAUTH_STAGE => {
                let page = Url::parse(self.oauth_url.as_deref()?).ok()?;
                LauncherSafeUrl::sign_in_page(page)
                    .ok()
                    .map(AuthStep::Approve)
            }
            _ => {
                let page = fallback_page(&self.homeserver, stage, self.session.as_deref()?)?;
                Some(AuthStep::Fallback {
                    stage: stage.to_owned(),
                    page,
                })
            }
        }
    }
}

fn fallback_page(homeserver: &str, stage: &str, session: &str) -> Option<LauncherSafeUrl> {
    let mut page = Url::parse(homeserver).ok()?;
    page.path_segments_mut()
        .ok()?
        .pop_if_empty()
        .extend(FALLBACK_PATH_BEFORE_STAGE)
        .push(stage)
        .extend(FALLBACK_PATH_AFTER_STAGE);
    page.set_query(None);
    page.query_pairs_mut()
        .append_pair(SESSION_PARAMETER, session);
    LauncherSafeUrl::sign_in_page(page).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthResponse {
    Password(AccountPassword),
    Dummy,
    Acknowledged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthAnswer {
    pub session: Option<String>,
    pub response: AuthResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Guarded {
    Done,
    NeedsAuth(Box<AuthChallenge>),
}
