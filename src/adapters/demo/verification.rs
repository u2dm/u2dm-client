use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use crate::domain::verification::{VerificationEmoji, VerificationEvent};

const ENV_VAR: &str = "U2DM_DEMO_VERIFY";
const REQUEST_DELAY: Duration = Duration::from_secs(3);
const STEP_DELAY: Duration = Duration::from_millis(900);
const SENDER: &str = "@sarah:matrix.org";

const EMOJIS: [(&str, &str); 7] = [
    ("🐶", "Dog"),
    ("🦄", "Unicorn"),
    ("🌰", "Chestnut"),
    ("🎸", "Guitar"),
    ("🚀", "Rocket"),
    ("🔑", "Key"),
    ("🍀", "Clover"),
];

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum FailingStep {
    #[default]
    None,
    Accept,
    Confirm,
    Reject,
}

pub struct VerificationDemo {
    pub is_self: bool,
    pub times_out: bool,
    pub fails: FailingStep,
}

impl Default for VerificationDemo {
    fn default() -> Self {
        Self {
            is_self: true,
            times_out: false,
            fails: FailingStep::None,
        }
    }
}

pub fn requested() -> Option<&'static VerificationDemo> {
    static DEMO: OnceLock<Option<VerificationDemo>> = OnceLock::new();
    DEMO.get_or_init(from_env).as_ref()
}

fn from_env() -> Option<VerificationDemo> {
    let demo = match env::var(ENV_VAR).ok()?.as_str() {
        "other" => VerificationDemo {
            is_self: false,
            ..VerificationDemo::default()
        },
        "accept-fails" => VerificationDemo {
            fails: FailingStep::Accept,
            ..VerificationDemo::default()
        },
        "confirm-fails" => VerificationDemo {
            fails: FailingStep::Confirm,
            ..VerificationDemo::default()
        },
        "reject-fails" => VerificationDemo {
            fails: FailingStep::Reject,
            ..VerificationDemo::default()
        },
        "timeout" => VerificationDemo {
            times_out: true,
            ..VerificationDemo::default()
        },
        _ => VerificationDemo::default(),
    };
    announce(&demo);
    Some(demo)
}

fn announce(demo: &VerificationDemo) {
    tracing::info!(
        "demo mode: a verification request arrives {}s after the chat opens",
        REQUEST_DELAY.as_secs()
    );
    if demo.times_out {
        tracing::info!("demo mode: the request times out on its own, reaching the cancelled step");
    }
    if demo.fails != FailingStep::None {
        tracing::info!("demo mode: an action fails so the in-dialog error is reachable");
    }
}

pub fn request(demo: &VerificationDemo) -> VerificationEvent {
    VerificationEvent::Requested {
        sender: SENDER.to_owned(),
        is_self: demo.is_self,
    }
}

pub fn emojis() -> VerificationEvent {
    VerificationEvent::Emojis(
        EMOJIS
            .iter()
            .map(|(symbol, description)| VerificationEmoji {
                symbol: (*symbol).to_owned(),
                description: (*description).to_owned(),
            })
            .collect(),
    )
}

pub async fn pause() {
    sleep(STEP_DELAY).await;
}

pub async fn wait_for_request() {
    sleep(REQUEST_DELAY).await;
}
