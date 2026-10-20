use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::sleep;
use url::Url;

use super::catalog::{Flag, Scenarios};
use crate::domain::capabilities::{
    AVATAR_FIELD, AccountApi, AccountFacts, AccountPage, Capabilities, DISPLAY_NAME_FIELD,
    ProfileFields,
};
use crate::domain::link::LauncherSafeUrl;
use crate::domain::sessions::{LogoutRisks, OwnSession, OwnSessions, ThisSession};
use crate::domain::uia::{
    AccountPassword, AuthAnswer, AuthChallenge, AuthRefusal, AuthResponse, Guarded, OAUTH_STAGE,
};
use crate::domain::user_info::DeviceTrust;
use crate::util::unix_now_ms;

const ENV_VAR: &str = "U2DM_DEMO_ACCOUNT";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes the account dialog: how the homeserver manages the account, what it asks before a sensitive action, the other sessions",
    combinable: true,
    flags: &[
        Flag {
            value: "oauth",
            effect: "the homeserver uses OAuth 2.0 with account pages: sign-outs, deactivation and the account open in the browser, and resetting the identity asks for an approval there",
            note: "a session signed out in the browser leaves the list 2s later",
        },
        Flag {
            value: "oauth-no-pages",
            effect: "the homeserver uses OAuth 2.0 but advertises no account management page",
            note: "the dialog lists the sessions and offers nothing to change",
        },
        Flag {
            value: "sso",
            effect: "sensitive actions ask for single sign-on through the homeserver's fallback page instead of a password",
            note: "Continue after the page opened finishes it",
        },
        Flag {
            value: "sso-unfinished",
            effect: "like sso, but the first Continue is answered as not finished yet",
            note: "",
        },
        Flag {
            value: "dummy",
            effect: "sensitive actions ask only for m.login.dummy, which U2DM answers without asking you",
            note: "",
        },
        Flag {
            value: "two-stages",
            effect: "sensitive actions ask for the password, then for an email confirmation on the fallback page",
            note: "",
        },
        Flag {
            value: "unsupported",
            effect: "sensitive actions ask for a stage U2DM cannot complete",
            note: "the dialog says to use another Matrix app",
        },
        Flag {
            value: "no-password-change",
            effect: "m.change_password is off, so the dialog says the password cannot be changed here",
            note: "",
        },
        Flag {
            value: "profile-managed",
            effect: "m.profile_fields is off, so the dialog says the homeserver manages the name and avatar",
            note: "",
        },
        Flag {
            value: "forgets-rooms",
            effect: "m.forget_forced_upon_leave is on, so the dialog says leaving a room also forgets it",
            note: "",
        },
        Flag {
            value: "last-session",
            effect: "this is the account's only session",
            note: "with U2DM_DEMO_ENCRYPTION=no-recovery, Log Out warns that the room keys are lost",
        },
        Flag {
            value: "unsent",
            effect: "Log Out finds two messages still waiting in the send queue and warns",
            note: "",
        },
        Flag {
            value: "weak-password",
            effect: "the homeserver refuses every new password as too weak, once the password is confirmed",
            note: "",
        },
        Flag {
            value: "slow",
            effect: "every account action takes 1.5s",
            note: "the dialog's busy states show meanwhile",
        },
        Flag {
            value: "fails",
            effect: "renaming, signing out, changing the password and deactivating all fail",
            note: "each failure shows in the dialog",
        },
    ],
    notes: &[
        "unset, the account has a password, three other sessions and every capability on",
        "the account password the confirmations ask for is DEMO_ACCOUNT_PASSWORD, demo password",
    ],
};

pub const DEMO_ACCOUNT_PASSWORD: &str = "demo password";
const ACTION_DELAY: Duration = Duration::from_millis(1500);
const BROWSER_LAG: Duration = Duration::from_secs(2);
const DEMO_HOMESERVER: &str = "https://matrix.org";
const ACCOUNT_PAGE: &str = "https://account.matrix.org/account/";
const APPROVAL_PAGE: &str =
    "https://account.matrix.org/account/?action=org.matrix.cross_signing_reset";
const THIS_SESSION_NAME: &str = "U2DM on this computer";
const THIS_SESSION_KEY: &str = "qJ8wR3nV6bT1mZ5xK9cF2hD7sL4pA0eG3yU8iO6tWkE";
const DEFAULT_ROOM_VERSION: &str = "11";
const PASSWORD_STAGE: &str = "m.login.password";
const DUMMY_STAGE: &str = "m.login.dummy";
const SSO_STAGE: &str = "m.login.sso";
const EMAIL_STAGE: &str = "m.login.email.identity";
const RECAPTCHA_STAGE: &str = "m.login.recaptcha";
const WRONG_PASSWORD_CODE: &str = "M_FORBIDDEN";
const WRONG_PASSWORD_MESSAGE: &str = "Invalid password";
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;
pub const WEAK_PASSWORD_REASON: &str = "Password must include a word you never used before";

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub oauth: bool,
    oauth_no_pages: bool,
    sso: bool,
    sso_unfinished: bool,
    dummy: bool,
    two_stages: bool,
    unsupported: bool,
    no_password_change: bool,
    profile_managed: bool,
    forgets_rooms: bool,
    last_session: bool,
    unsent: bool,
    pub weak_password: bool,
    slow: bool,
    pub fails: bool,
}

pub fn scenario() -> Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    *SCENARIO.get_or_init(from_env)
}

fn from_env() -> Scenario {
    let Ok(raw) = env::var(ENV_VAR) else {
        return Scenario::default();
    };
    let mut scenario = Scenario::default();
    for flag in raw.split(',').map(str::trim) {
        apply(&mut scenario, flag);
    }
    tracing::info!(
        oauth = scenario.oauth,
        oauth_no_pages = scenario.oauth_no_pages,
        sso = scenario.sso,
        sso_unfinished = scenario.sso_unfinished,
        dummy = scenario.dummy,
        two_stages = scenario.two_stages,
        unsupported = scenario.unsupported,
        no_password_change = scenario.no_password_change,
        profile_managed = scenario.profile_managed,
        forgets_rooms = scenario.forgets_rooms,
        last_session = scenario.last_session,
        unsent = scenario.unsent,
        weak_password = scenario.weak_password,
        slow = scenario.slow,
        fails = scenario.fails,
        "demo mode: shaping the account"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "oauth" => scenario.oauth = true,
        "oauth-no-pages" => scenario.oauth_no_pages = true,
        "sso" => scenario.sso = true,
        "sso-unfinished" => scenario.sso_unfinished = true,
        "dummy" => scenario.dummy = true,
        "two-stages" => scenario.two_stages = true,
        "unsupported" => scenario.unsupported = true,
        "no-password-change" => scenario.no_password_change = true,
        "profile-managed" => scenario.profile_managed = true,
        "forgets-rooms" => scenario.forgets_rooms = true,
        "last-session" => scenario.last_session = true,
        "unsent" => scenario.unsent = true,
        "weak-password" => scenario.weak_password = true,
        "slow" => scenario.slow = true,
        "fails" => scenario.fails = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

impl Scenario {
    fn uses_oauth(self) -> bool {
        self.oauth || self.oauth_no_pages
    }

    fn flows(self, reset_on_oauth: bool) -> Vec<Vec<String>> {
        let stages: &[&str] = if reset_on_oauth {
            &[OAUTH_STAGE]
        } else if self.unsupported {
            &[RECAPTCHA_STAGE]
        } else if self.dummy {
            &[DUMMY_STAGE]
        } else if self.sso || self.sso_unfinished {
            &[SSO_STAGE]
        } else if self.two_stages {
            &[PASSWORD_STAGE, EMAIL_STAGE]
        } else {
            &[PASSWORD_STAGE]
        };
        vec![stages.iter().map(|stage| (*stage).to_owned()).collect()]
    }
}

pub async fn pause() {
    if scenario().slow {
        sleep(ACTION_DELAY).await;
    }
}

pub fn facts() -> AccountFacts {
    let scenario = scenario();
    let api = if scenario.oauth {
        AccountApi::OAuth { pages: true }
    } else if scenario.oauth_no_pages {
        AccountApi::OAuth { pages: false }
    } else {
        AccountApi::Legacy
    };
    AccountFacts {
        api,
        capabilities: Capabilities {
            change_password: !scenario.no_password_change && !scenario.uses_oauth(),
            profile_fields: if scenario.profile_managed {
                ProfileFields::new(false, None, Vec::new())
            } else {
                ProfileFields::only([DISPLAY_NAME_FIELD, AVATAR_FIELD])
            },
            default_room_version: DEFAULT_ROOM_VERSION.to_owned(),
            forget_forced_upon_leave: scenario.forgets_rooms,
        },
    }
}

pub fn page(page: &AccountPage) -> Option<LauncherSafeUrl> {
    if !scenario().oauth {
        return None;
    }
    let mut url = Url::parse(ACCOUNT_PAGE).ok()?;
    match page {
        AccountPage::Account => {}
        AccountPage::Sessions => {
            url.query_pairs_mut()
                .append_pair("action", "org.matrix.devices_list");
        }
        AccountPage::SignOut(device_id) => {
            url.query_pairs_mut()
                .append_pair("action", "org.matrix.device_delete")
                .append_pair("device_id", device_id);
        }
        AccountPage::Deactivate => {
            url.query_pairs_mut()
                .append_pair("action", "org.matrix.account_deactivate");
        }
    }
    LauncherSafeUrl::sign_in_page(url).ok()
}

#[derive(Default)]
struct Round {
    completed: Vec<String>,
    acknowledged: u32,
}

pub struct AccountBook {
    others: Mutex<Vec<OwnSession>>,
    this_name: Mutex<Option<String>>,
    rounds: Mutex<HashMap<String, Round>>,
    next_round: AtomicU64,
    revision: watch::Sender<u64>,
}

impl Default for AccountBook {
    fn default() -> Self {
        let others = if scenario().last_session {
            Vec::new()
        } else {
            initial_sessions(unix_now_ms())
        };
        Self {
            others: Mutex::new(others),
            this_name: Mutex::new(Some(THIS_SESSION_NAME.to_owned())),
            rounds: Mutex::new(HashMap::new()),
            next_round: AtomicU64::new(0),
            revision: watch::Sender::new(0),
        }
    }
}

fn initial_sessions(now: u64) -> Vec<OwnSession> {
    vec![
        OwnSession {
            device_id: "IPHONEXQZ".to_owned(),
            name: Some("Element X on iPhone".to_owned()),
            last_seen_ms: Some(now.saturating_sub(5 * MINUTE_MS)),
            last_seen_ip: Some("192.0.2.10".to_owned()),
            trust: DeviceTrust::Verified,
        },
        OwnSession {
            device_id: "WEBFIREFOX".to_owned(),
            name: Some("Element on Firefox".to_owned()),
            last_seen_ms: Some(now.saturating_sub(2 * DAY_MS)),
            last_seen_ip: Some("198.51.100.7".to_owned()),
            trust: DeviceTrust::Verified,
        },
        OwnSession {
            device_id: "OLDTABLET".to_owned(),
            name: Some("Old tablet".to_owned()),
            last_seen_ms: Some(now.saturating_sub(97 * DAY_MS)),
            last_seen_ip: None,
            trust: DeviceTrust::NotSignedByOwner,
        },
    ]
}

impl AccountBook {
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    pub fn sessions(&self, device_id: &str) -> OwnSessions {
        let this = ThisSession {
            device_id: device_id.to_owned(),
            name: self.this_name.lock().map_or(None, |name| name.clone()),
            key: Some(THIS_SESSION_KEY.to_owned()),
        };
        let others = self
            .others
            .lock()
            .map(|others| others.clone())
            .unwrap_or_default();
        OwnSessions::new(this, others)
    }

    pub fn rename(&self, this_device: &str, device_id: &str, name: &str) {
        if device_id == this_device {
            if let Ok(mut this_name) = self.this_name.lock() {
                *this_name = Some(name.to_owned());
            }
        } else if let Ok(mut others) = self.others.lock() {
            for session in others
                .iter_mut()
                .filter(|session| session.device_id == device_id)
            {
                session.name = Some(name.to_owned());
            }
        }
        self.bump();
    }

    pub fn remove(&self, device_id: &str) {
        if let Ok(mut others) = self.others.lock() {
            others.retain(|session| session.device_id != device_id);
        }
        self.bump();
    }

    pub fn remove_all_others(&self) {
        if let Ok(mut others) = self.others.lock() {
            others.clear();
        }
        self.bump();
    }

    pub fn logout_risks(&self, keys_backed_up: bool) -> LogoutRisks {
        let alone = self.others.lock().is_ok_and(|others| others.is_empty());
        LogoutRisks {
            keys_at_risk: alone && !keys_backed_up,
            unsent: if scenario().unsent { 2 } else { 0 },
        }
    }

    pub fn guard(&self, auth: Option<&AuthAnswer>, reset_on_oauth: bool) -> Guarded {
        let scenario = scenario();
        let flows = scenario.flows(reset_on_oauth);
        let Some(session) = auth.and_then(|auth| auth.session.clone()) else {
            return self.fresh_challenge(flows, reset_on_oauth);
        };
        let Ok(mut rounds) = self.rounds.lock() else {
            return self.fresh_challenge(flows, reset_on_oauth);
        };
        let round = rounds.entry(session.clone()).or_default();
        let flow = flows.first().cloned().unwrap_or_default();
        let next = flow.get(round.completed.len()).cloned();
        let mut refusal = None;
        match (auth.map(|auth| &auth.response), next.as_deref()) {
            (Some(AuthResponse::Password(password)), Some(PASSWORD_STAGE)) => {
                if accepts(password) {
                    round.completed.push(PASSWORD_STAGE.to_owned());
                } else {
                    refusal = Some(AuthRefusal {
                        code: WRONG_PASSWORD_CODE.to_owned(),
                        message: WRONG_PASSWORD_MESSAGE.to_owned(),
                    });
                }
            }
            (Some(AuthResponse::Dummy), Some(DUMMY_STAGE)) => {
                round.completed.push(DUMMY_STAGE.to_owned());
            }
            (Some(AuthResponse::Acknowledged), Some(stage)) if stage != PASSWORD_STAGE => {
                round.acknowledged += 1;
                if !(scenario.sso_unfinished && round.acknowledged == 1) {
                    round.completed.push(stage.to_owned());
                }
            }
            _ => {}
        }
        if round.completed.len() >= flow.len() {
            rounds.remove(&session);
            return Guarded::Done;
        }
        let completed = round.completed.clone();
        Guarded::NeedsAuth(Box::new(challenge(
            Some(session),
            flows,
            completed,
            reset_on_oauth,
            refusal,
        )))
    }

    fn fresh_challenge(&self, flows: Vec<Vec<String>>, reset_on_oauth: bool) -> Guarded {
        let session = if scenario().unsupported && !reset_on_oauth {
            None
        } else {
            let round = self.next_round.fetch_add(1, Ordering::Relaxed);
            Some(format!("demo-uia-{round}"))
        };
        Guarded::NeedsAuth(Box::new(challenge(
            session,
            flows,
            Vec::new(),
            reset_on_oauth,
            None,
        )))
    }

    fn bump(&self) {
        self.revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

fn accepts(password: &AccountPassword) -> bool {
    password.expose() == DEMO_ACCOUNT_PASSWORD
}

fn challenge(
    session: Option<String>,
    flows: Vec<Vec<String>>,
    completed: Vec<String>,
    reset_on_oauth: bool,
    refusal: Option<AuthRefusal>,
) -> AuthChallenge {
    AuthChallenge {
        session,
        flows,
        completed,
        oauth_url: reset_on_oauth.then(|| APPROVAL_PAGE.to_owned()),
        homeserver: DEMO_HOMESERVER.to_owned(),
        refusal,
    }
}

pub async fn signed_out_in_browser(book: &AccountBook, device_id: &str) {
    sleep(BROWSER_LAG).await;
    tracing::info!(%device_id, "demo: the session was signed out on the account page");
    book.remove(device_id);
}
