use super::catalog::{Flag, Scenarios};
use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use crate::domain::auth::AuthMethod;

const ENV_VAR: &str = "U2DM_DEMO_LOGIN";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "starts logged out and shapes what the server advertises",
    combinable: false,
    flags: &[
        Flag {
            value: "password",
            effect: "password auth only",
            note: "also the fallback for any unrecognised value",
        },
        Flag {
            value: "oauth",
            effect: "OAuth only; the flow itself errors, exercising the error banner",
            note: "",
        },
        Flag {
            value: "oauth-ok",
            effect: "OAuth that succeeds, pausing so cancelling mid-flow is reachable",
            note: "the only way to reach the durable transaction in establish_session",
        },
        Flag {
            value: "oauth-insecure",
            effect: "OAuth whose sign-in page is an smb: URL, which U2DM refuses to open",
            note: "the only way to see the insecure sign-in page banner",
        },
        Flag {
            value: "both",
            effect: "both methods, so the divider and second button render",
            note: "",
        },
        Flag {
            value: "sso",
            effect: "only m.login.sso/m.login.token, which U2DM cannot drive",
            note: "",
        },
        Flag {
            value: "restore",
            effect: "keeps the saved session but restores it slowly",
            note: "the only way to see the loading screen",
        },
    ],
    notes: &[
        "merely SETTING this variable switches the demo from logged-in to logged-out",
        "each auth step pauses 900ms, which is what makes transient states wide enough to click",
    ],
};
const STEP_DELAY: Duration = Duration::from_millis(900);
const SIGN_IN_PAGE: &str = "https://example.invalid/demo-oauth";
const INSECURE_SIGN_IN_PAGE: &str = "smb://example.invalid/demo-oauth";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OAuthFlow {
    Fails,
    Succeeds,
    OffersInsecurePage,
}

pub struct LoginDemo {
    pub methods: Vec<AuthMethod>,
    pub unsupported_flows: Vec<String>,
    pub keeps_session: bool,
    pub oauth: OAuthFlow,
}

pub fn requested() -> Option<&'static LoginDemo> {
    static DEMO: OnceLock<Option<LoginDemo>> = OnceLock::new();
    DEMO.get_or_init(from_env).as_ref()
}

pub fn oauth_succeeds() -> bool {
    requested().is_some_and(|demo| demo.oauth == OAuthFlow::Succeeds)
}

pub fn sign_in_page() -> Option<&'static str> {
    match requested()?.oauth {
        OAuthFlow::Fails => None,
        OAuthFlow::Succeeds => Some(SIGN_IN_PAGE),
        OAuthFlow::OffersInsecurePage => Some(INSECURE_SIGN_IN_PAGE),
    }
}

impl Default for LoginDemo {
    fn default() -> Self {
        Self {
            methods: vec![AuthMethod::Password],
            unsupported_flows: Vec::new(),
            keeps_session: false,
            oauth: OAuthFlow::Fails,
        }
    }
}

fn from_env() -> Option<LoginDemo> {
    let demo = match env::var(ENV_VAR).ok()?.as_str() {
        "oauth" => LoginDemo {
            methods: vec![AuthMethod::OAuth],
            ..LoginDemo::default()
        },
        "oauth-ok" => LoginDemo {
            methods: vec![AuthMethod::OAuth],
            oauth: OAuthFlow::Succeeds,
            ..LoginDemo::default()
        },
        "oauth-insecure" => LoginDemo {
            methods: vec![AuthMethod::OAuth],
            oauth: OAuthFlow::OffersInsecurePage,
            ..LoginDemo::default()
        },
        "both" => LoginDemo {
            methods: vec![AuthMethod::Password, AuthMethod::OAuth],
            ..LoginDemo::default()
        },
        "sso" => LoginDemo {
            methods: Vec::new(),
            unsupported_flows: vec!["m.login.sso".to_owned(), "m.login.token".to_owned()],
            ..LoginDemo::default()
        },
        "restore" => LoginDemo {
            keeps_session: true,
            ..LoginDemo::default()
        },
        _ => LoginDemo::default(),
    };
    announce(&demo);
    Some(demo)
}

fn announce(demo: &LoginDemo) {
    announce_start(demo);
    announce_methods(demo);
}

fn announce_start(demo: &LoginDemo) {
    if demo.keeps_session {
        tracing::info!(
            "demo mode: restoring the saved session slowly so the loading step is visible"
        );
    } else {
        tracing::info!("demo mode: starting logged out so the login steps are reachable");
    }
}

fn announce_methods(demo: &LoginDemo) {
    match demo.oauth {
        OAuthFlow::Fails => {}
        OAuthFlow::Succeeds => tracing::info!(
            "demo mode: the OAuth flow completes slowly so cancelling it is reachable"
        ),
        OAuthFlow::OffersInsecurePage => tracing::info!(
            page = INSECURE_SIGN_IN_PAGE,
            "demo mode: the server offers a sign-in page U2DM must refuse to open"
        ),
    }
    if demo.methods.is_empty() {
        tracing::info!(
            flows = ?demo.unsupported_flows,
            "demo mode: the server offers no login method U2DM supports"
        );
    }
}

pub async fn pause() {
    if requested().is_some() {
        sleep(STEP_DELAY).await;
    }
}
