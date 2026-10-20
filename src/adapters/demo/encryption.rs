use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use crate::domain::encryption::{
    AccountEncryption, KeyImport, KeyPassphrase, KeyStorage, Recovery, RecoveryAttempt,
    RecoveryKey, SessionTrust,
};

const ENV_VAR: &str = "U2DM_DEMO_ENCRYPTION";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes this session's trust: unverified, an account without cross-signing, recovery off or on",
    combinable: true,
    flags: &[
        Flag {
            value: "unverified",
            effect: "this session starts unverified: another session can verify it, and recovery is set up on the account",
            note: "Use another session runs a scripted SAS; the recovery key is DEMO_RECOVERY_KEY, the passphrase correct horse",
        },
        Flag {
            value: "no-sessions",
            effect: "no other verified session exists, so only the recovery key can verify this one",
            note: "",
        },
        Flag {
            value: "no-recovery",
            effect: "the account has no recovery set up",
            note: "with unverified and no-sessions nothing can verify this session; verified, the dialog offers Set up recovery",
        },
        Flag {
            value: "new-account",
            effect: "the account has no cross-signing identity, so the session creates one 2s after login",
            note: "it then reads verified with recovery off",
        },
        Flag {
            value: "setup-refused",
            effect: "the homeserver wants a password before it takes the first cross-signing keys",
            note: "the banner reads encryption-not-set-up; Try again asks for the account password (U2DM_DEMO_ACCOUNT shapes the stages)",
        },
        Flag {
            value: "setup-fails",
            effect: "the first attempt to create the identity fails",
            note: "Try again succeeds",
        },
        Flag {
            value: "key-storage-off",
            effect: "the account turned key storage off (m.key_backup), so setting up recovery says it turns it back on",
            note: "",
        },
        Flag {
            value: "backup-unavailable",
            effect: "the right recovery key verifies the session but cannot restore the key backup",
            note: "",
        },
        Flag {
            value: "slow",
            effect: "every encryption action takes 1.5s, the first assessment arrives 1s after login",
            note: "the dialog's buttons show their busy state meanwhile",
        },
        Flag {
            value: "fails",
            effect: "asking another session, using a recovery key, setting up and changing recovery, resetting the identity and exporting or importing room keys all fail",
            note: "each failure shows in the dialog",
        },
        Flag {
            value: "not-a-key-file",
            effect: "the file picked to import room keys is not a key export",
            note: "",
        },
        Flag {
            value: "key-file-cancelled",
            effect: "the file chooser for exporting or importing room keys returns nothing, as if cancelled",
            note: "",
        },
    ],
    notes: &[
        "unset, this session is verified and recovery is on",
        "while the session is unverified or has no identity, texts and polls in encrypted rooms are not sent, as the identity-based key sharing refuses them",
        "an export writes no file and reports EXPORTED_KEYS keys; an import opens with the passphrase correct horse and reports IMPORTED_KEYS of KEY_FILE_KEYS",
    ],
};

pub const DEMO_RECOVERY_KEY: &str = "EsTk 7qXb 4Hsd N2Pw Lw3a mQ8R yZfJ 6uTe cHnV kB9o Xr5G wPq2";
const DEMO_PASSPHRASE: &str = "correct horse";
const CREATED_KEY: &str = "EsUa 9PcV Mz4H wT2b Rk7Q nL5d Gx8J hY3s Bf6W qN1c Td4K mZ8v";
const CHANGED_KEY: &str = "EsTz 2Kd8 Hq5W nV7c Lm3X bR9t Fy4P sJ6g Wc1N kQ8e Ud5A xG2h";
const SETUP_DELAY: Duration = Duration::from_secs(2);
const ACTION_DELAY: Duration = Duration::from_millis(1500);
const FIRST_ASSESSMENT_DELAY: Duration = Duration::from_secs(1);
const SAS_DELAY: Duration = Duration::from_secs(2);
pub const EXPORTED_KEYS: usize = 42;
const IMPORTED_KEYS: usize = 12;
const KEY_FILE_KEYS: usize = 15;

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    unverified: bool,
    no_sessions: bool,
    no_recovery: bool,
    new_account: bool,
    setup_refused: bool,
    setup_fails: bool,
    key_storage_off: bool,
    backup_unavailable: bool,
    slow: bool,
    pub fails: bool,
    not_a_key_file: bool,
    pub key_file_cancelled: bool,
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
        unverified = scenario.unverified,
        no_sessions = scenario.no_sessions,
        no_recovery = scenario.no_recovery,
        new_account = scenario.new_account,
        setup_refused = scenario.setup_refused,
        setup_fails = scenario.setup_fails,
        key_storage_off = scenario.key_storage_off,
        backup_unavailable = scenario.backup_unavailable,
        slow = scenario.slow,
        fails = scenario.fails,
        not_a_key_file = scenario.not_a_key_file,
        key_file_cancelled = scenario.key_file_cancelled,
        "demo mode: shaping this session's encryption"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "unverified" => scenario.unverified = true,
        "no-sessions" => scenario.no_sessions = true,
        "no-recovery" => scenario.no_recovery = true,
        "new-account" => scenario.new_account = true,
        "setup-refused" => scenario.setup_refused = true,
        "setup-fails" => scenario.setup_fails = true,
        "key-storage-off" => scenario.key_storage_off = true,
        "backup-unavailable" => scenario.backup_unavailable = true,
        "slow" => scenario.slow = true,
        "fails" => scenario.fails = true,
        "not-a-key-file" => scenario.not_a_key_file = true,
        "key-file-cancelled" => scenario.key_file_cancelled = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

impl Scenario {
    fn lacks_identity(self) -> bool {
        self.new_account || self.setup_refused || self.setup_fails
    }

    fn opening_state(self) -> AccountEncryption {
        let trust = if self.lacks_identity() {
            SessionTrust::NoIdentity
        } else if self.unverified {
            SessionTrust::Unverified
        } else {
            SessionTrust::Verified
        };
        let recovery = if self.no_recovery || self.lacks_identity() {
            Recovery::Off
        } else if trust == SessionTrust::Unverified {
            Recovery::Incomplete
        } else {
            Recovery::On
        };
        AccountEncryption {
            trust,
            verified_sessions: trust == SessionTrust::Unverified && !self.no_sessions,
            recovery,
            key_storage: if self.key_storage_off {
                KeyStorage::TurnedOff
            } else {
                KeyStorage::Allowed
            },
        }
    }
}

pub async fn pause_first_assessment() {
    if scenario().slow {
        sleep(FIRST_ASSESSMENT_DELAY).await;
    }
}

pub async fn pause_action() {
    if scenario().slow {
        sleep(ACTION_DELAY).await;
    }
}

pub async fn pause_setup() {
    sleep(SETUP_DELAY).await;
}

pub async fn pause_sas() {
    sleep(SAS_DELAY).await;
}

pub fn unlock_attempt() -> RecoveryAttempt {
    if scenario().backup_unavailable {
        RecoveryAttempt::UnlockedWithoutBackup
    } else {
        RecoveryAttempt::Unlocked
    }
}

pub fn identity_refused() -> bool {
    scenario().setup_refused
}

pub fn first_setup_fails(attempt: u32) -> bool {
    scenario().setup_fails && attempt == 1
}

pub fn opens(key: &RecoveryKey) -> bool {
    let typed = key.expose();
    let squeezed: String = typed.split_whitespace().collect();
    let expected: String = DEMO_RECOVERY_KEY.split_whitespace().collect();
    squeezed == expected || typed == DEMO_PASSPHRASE
}

pub fn import_from(passphrase: &KeyPassphrase) -> KeyImport {
    if scenario().not_a_key_file {
        KeyImport::NotAKeyFile
    } else if passphrase.expose() == DEMO_PASSPHRASE {
        KeyImport::Imported {
            imported: IMPORTED_KEYS,
            total: KEY_FILE_KEYS,
        }
    } else {
        KeyImport::WrongPassphrase
    }
}

pub fn created_key() -> RecoveryKey {
    RecoveryKey::new(CREATED_KEY)
}

pub fn changed_key() -> RecoveryKey {
    RecoveryKey::new(CHANGED_KEY)
}

pub struct EncryptionBook {
    state: Mutex<AccountEncryption>,
    setup_attempts: Mutex<u32>,
    revision: watch::Sender<u64>,
}

impl Default for EncryptionBook {
    fn default() -> Self {
        Self {
            state: Mutex::new(scenario().opening_state()),
            setup_attempts: Mutex::new(0),
            revision: watch::Sender::new(0),
        }
    }
}

impl EncryptionBook {
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    pub fn state(&self) -> AccountEncryption {
        self.state.lock().map(|state| *state).unwrap_or_default()
    }

    pub fn holds_encrypted_sends(&self) -> bool {
        self.state().trust != SessionTrust::Verified
    }

    pub fn has_identity(&self) -> bool {
        self.state().trust != SessionTrust::NoIdentity
    }

    pub fn count_setup_attempt(&self) -> u32 {
        self.setup_attempts.lock().map_or(1, |mut attempts| {
            *attempts += 1;
            *attempts
        })
    }

    pub fn create_identity(&self) {
        self.change(|state| {
            state.trust = SessionTrust::Verified;
            state.verified_sessions = false;
        });
    }

    pub fn verified_by_another_session(&self) {
        self.change(|state| {
            state.trust = SessionTrust::Verified;
            state.verified_sessions = false;
            if state.recovery == Recovery::Incomplete {
                state.recovery = Recovery::On;
            }
        });
    }

    pub fn unlocked(&self, attempt: RecoveryAttempt) {
        self.change(|state| {
            state.trust = SessionTrust::Verified;
            state.verified_sessions = false;
            state.recovery = if attempt == RecoveryAttempt::Unlocked {
                Recovery::On
            } else {
                Recovery::Incomplete
            };
        });
    }

    pub fn identity_reset(&self) {
        self.change(|state| {
            state.trust = SessionTrust::Verified;
            state.verified_sessions = false;
            state.recovery = Recovery::Off;
        });
    }

    pub fn recovery_enabled(&self) {
        self.change(|state| {
            state.recovery = Recovery::On;
            state.key_storage = KeyStorage::Allowed;
        });
    }

    fn change(&self, change: impl FnOnce(&mut AccountEncryption)) {
        if let Ok(mut state) = self.state.lock() {
            change(&mut state);
        }
        self.revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}
