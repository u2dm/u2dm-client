use std::path::{Path, PathBuf};
use std::{fs, io, result};

use async_trait::async_trait;
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::private_fs;
use crate::domain::settings::{
    Appearance, Messaging, MotionChoice, ReceiptVisibility, SendKey, Settings, ThemeChoice,
};
use crate::error::Result;
use crate::ports::settings::SettingsPort;

macro_rules! stored_choice {
    ($stored:ident $domain:ident; $($variant:ident $name:literal;)*) => {
        #[derive(Clone, Copy, Deserialize, Serialize)]
        enum $stored {
            $(#[serde(rename = $name)] $variant,)*
        }

        impl From<$domain> for $stored {
            fn from(choice: $domain) -> Self {
                match choice { $($domain::$variant => Self::$variant,)* }
            }
        }

        impl From<$stored> for $domain {
            fn from(choice: $stored) -> Self {
                match choice { $($stored::$variant => Self::$variant,)* }
            }
        }
    };
}

stored_choice!(StoredTheme ThemeChoice;
    System "system";
    Light "light";
    Dark "dark";
);

stored_choice!(StoredMotion MotionChoice;
    System "system";
    Reduced "reduced";
    Full "full";
);

stored_choice!(StoredSendKey SendKey;
    Enter "enter";
    CtrlEnter "ctrl-enter";
);

stored_choice!(StoredReceipts ReceiptVisibility;
    Public "public";
    Private "private";
);

#[derive(Default, Deserialize, Serialize)]
struct StoredAppearance {
    #[serde(default, deserialize_with = "lenient")]
    theme: Option<StoredTheme>,
    #[serde(default, deserialize_with = "lenient")]
    motion: Option<StoredMotion>,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredMessaging {
    #[serde(default, deserialize_with = "lenient")]
    send_key: Option<StoredSendKey>,
    #[serde(default, deserialize_with = "lenient")]
    receipts: Option<StoredReceipts>,
    #[serde(default, deserialize_with = "lenient")]
    typing_notices: Option<bool>,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredSettings {
    #[serde(default, deserialize_with = "lenient")]
    appearance: Option<StoredAppearance>,
    #[serde(default, deserialize_with = "lenient")]
    messaging: Option<StoredMessaging>,
}

fn lenient<'de, D, T>(deserializer: D) -> result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match serde_json::from_value(value) {
        Ok(read) => Some(read),
        Err(e) => {
            tracing::warn!("a setting could not be read, so it keeps its default: {e}");
            None
        }
    })
}

impl From<&Settings> for StoredSettings {
    fn from(settings: &Settings) -> Self {
        let Settings {
            appearance: Appearance { theme, motion },
            messaging:
                Messaging {
                    send_key,
                    receipts,
                    typing_notices,
                },
        } = *settings;
        Self {
            appearance: Some(StoredAppearance {
                theme: Some(theme.into()),
                motion: Some(motion.into()),
            }),
            messaging: Some(StoredMessaging {
                send_key: Some(send_key.into()),
                receipts: Some(receipts.into()),
                typing_notices: Some(typing_notices),
            }),
        }
    }
}

impl From<StoredSettings> for Settings {
    fn from(stored: StoredSettings) -> Self {
        let defaults = Self::default();
        let appearance = stored.appearance.unwrap_or_default();
        let messaging = stored.messaging.unwrap_or_default();
        Self {
            appearance: Appearance {
                theme: appearance
                    .theme
                    .map_or(defaults.appearance.theme, Into::into),
                motion: appearance
                    .motion
                    .map_or(defaults.appearance.motion, Into::into),
            },
            messaging: Messaging {
                send_key: messaging
                    .send_key
                    .map_or(defaults.messaging.send_key, Into::into),
                receipts: messaging
                    .receipts
                    .map_or(defaults.messaging.receipts, Into::into),
                typing_notices: messaging
                    .typing_notices
                    .unwrap_or(defaults.messaging.typing_notices),
            },
        }
    }
}

pub struct JsonSettings {
    path: PathBuf,
}

impl JsonSettings {
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl SettingsPort for JsonSettings {
    fn load(&self) -> Settings {
        match fs::read(&self.path) {
            Ok(bytes) => parsed(&bytes, &self.path),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Settings::default(),
            Err(e) => {
                tracing::warn!(
                    path = %self.path.display(),
                    "the settings file could not be read, so every setting keeps its default: {e}"
                );
                Settings::default()
            }
        }
    }

    async fn save(&self, settings: &Settings) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&StoredSettings::from(settings))?;
        if let Some(parent) = self.path.parent() {
            private_fs::create_dir(parent).await?;
        }
        private_fs::write_atomically(&self.path, &bytes).await?;
        tracing::debug!(path = %self.path.display(), "saved the settings");
        Ok(())
    }
}

fn parsed(bytes: &[u8], path: &Path) -> Settings {
    match serde_json::from_slice::<StoredSettings>(bytes) {
        Ok(stored) => stored.into(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                "the settings file is malformed, so every setting keeps its default: {e}"
            );
            Settings::default()
        }
    }
}
