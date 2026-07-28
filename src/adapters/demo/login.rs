use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use crate::domain::models::AuthMethod;

const ENV_VAR: &str = "U2DM_DEMO_LOGIN";
const STEP_DELAY: Duration = Duration::from_millis(900);

pub struct LoginDemo {
    pub methods: Vec<AuthMethod>,
    pub keeps_session: bool,
    pub oauth_succeeds: bool,
}

pub fn requested() -> Option<&'static LoginDemo> {
    static DEMO: OnceLock<Option<LoginDemo>> = OnceLock::new();
    DEMO.get_or_init(from_env).as_ref()
}

pub fn oauth_succeeds() -> bool {
    requested().is_some_and(|demo| demo.oauth_succeeds)
}

impl Default for LoginDemo {
    fn default() -> Self {
        Self {
            methods: vec![AuthMethod::Password],
            keeps_session: false,
            oauth_succeeds: false,
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
            oauth_succeeds: true,
            ..LoginDemo::default()
        },
        "both" => LoginDemo {
            methods: vec![AuthMethod::Password, AuthMethod::OAuth],
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
    if demo.keeps_session {
        tracing::info!(
            "demo mode: restoring the saved session slowly so the loading step is visible"
        );
    } else {
        tracing::info!("demo mode: starting logged out so the login steps are reachable");
    }
    if demo.oauth_succeeds {
        tracing::info!("demo mode: the OAuth flow completes slowly so cancelling it is reachable");
    }
}

pub async fn pause() {
    if requested().is_some() {
        sleep(STEP_DELAY).await;
    }
}
