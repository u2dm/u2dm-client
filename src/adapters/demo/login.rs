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
}

pub fn requested() -> Option<&'static LoginDemo> {
    static DEMO: OnceLock<Option<LoginDemo>> = OnceLock::new();
    DEMO.get_or_init(from_env).as_ref()
}

fn from_env() -> Option<LoginDemo> {
    let (methods, keeps_session) = match env::var(ENV_VAR).ok()?.as_str() {
        "oauth" => (vec![AuthMethod::OAuth], false),
        "both" => (vec![AuthMethod::Password, AuthMethod::OAuth], false),
        "restore" => (vec![AuthMethod::Password], true),
        _ => (vec![AuthMethod::Password], false),
    };
    if keeps_session {
        tracing::info!(
            "demo mode: restoring the saved session slowly so the loading step is visible"
        );
    } else {
        tracing::info!("demo mode: starting logged out so the login steps are reachable");
    }
    Some(LoginDemo {
        methods,
        keeps_session,
    })
}

pub async fn pause() {
    if requested().is_some() {
        sleep(STEP_DELAY).await;
    }
}
