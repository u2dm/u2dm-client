use std::path::{Path, PathBuf};
use std::{fs, io, result};

use async_trait::async_trait;
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::private_fs;
use crate::domain::call::devices::{ByKind, DeviceChoice, DeviceKey, MediaDevice};
use crate::domain::settings::{
    Alerts, Appearance, CallLayout, CallPreferences, MediaPreferences, Messaging, MotionChoice,
    ReceiptVisibility, SendKey, Settings, ThemeChoice,
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

stored_choice!(StoredCallLayout CallLayout;
    Automatic "automatic";
    CallAndChat "call-and-chat";
    CallOnly "call-only";
    ChatOnly "chat-only";
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
    #[serde(default, deserialize_with = "lenient")]
    share_presence: Option<bool>,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredAlerts {
    #[serde(default, deserialize_with = "lenient")]
    desktop_notifications: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    hide_message_text: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    notification_sound: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    ringtone: Option<bool>,
}

#[derive(Deserialize, Serialize)]
struct StoredDevice {
    key: String,
    name: String,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredDevices {
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    microphone: Option<StoredDevice>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    speaker: Option<StoredDevice>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    camera: Option<StoredDevice>,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredCalls {
    #[serde(default, deserialize_with = "lenient")]
    remember_devices: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    camera_on_join: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    layout: Option<StoredCallLayout>,
    #[serde(default, deserialize_with = "lenient")]
    follow_speaker: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    devices: Option<StoredDevices>,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredMedia {
    #[serde(default, deserialize_with = "lenient")]
    auto_load: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    play_animations: Option<bool>,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredSettings {
    #[serde(default, deserialize_with = "lenient")]
    appearance: Option<StoredAppearance>,
    #[serde(default, deserialize_with = "lenient")]
    messaging: Option<StoredMessaging>,
    #[serde(default, deserialize_with = "lenient")]
    alerts: Option<StoredAlerts>,
    #[serde(default, deserialize_with = "lenient")]
    calls: Option<StoredCalls>,
    #[serde(default, deserialize_with = "lenient")]
    media: Option<StoredMedia>,
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

fn stored_device(choice: &DeviceChoice) -> Option<StoredDevice> {
    match choice {
        DeviceChoice::Default => None,
        DeviceChoice::Device(device) => Some(StoredDevice {
            key: device.key.to_string(),
            name: device.name.clone(),
        }),
    }
}

fn device_choice(stored: Option<StoredDevice>) -> DeviceChoice {
    stored.map_or(DeviceChoice::Default, |device| {
        DeviceChoice::Device(MediaDevice {
            key: DeviceKey::new(device.key),
            name: device.name,
        })
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
                    share_presence,
                },
            alerts:
                Alerts {
                    desktop_notifications,
                    hide_message_text,
                    notification_sound,
                    ringtone,
                },
            calls:
                CallPreferences {
                    remember_devices,
                    camera_on_join,
                    layout,
                    follow_speaker,
                },
            ref remembered_devices,
            media:
                MediaPreferences {
                    auto_load,
                    play_animations,
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
                share_presence: Some(share_presence),
            }),
            alerts: Some(StoredAlerts {
                desktop_notifications: Some(desktop_notifications),
                hide_message_text: Some(hide_message_text),
                notification_sound: Some(notification_sound),
                ringtone: Some(ringtone),
            }),
            calls: Some(StoredCalls {
                remember_devices: Some(remember_devices),
                camera_on_join: Some(camera_on_join),
                layout: Some(layout.into()),
                follow_speaker: Some(follow_speaker),
                devices: Some(StoredDevices {
                    microphone: stored_device(&remembered_devices.microphone),
                    speaker: stored_device(&remembered_devices.speaker),
                    camera: stored_device(&remembered_devices.camera),
                }),
            }),
            media: Some(StoredMedia {
                auto_load: Some(auto_load),
                play_animations: Some(play_animations),
            }),
        }
    }
}

impl From<StoredSettings> for Settings {
    fn from(stored: StoredSettings) -> Self {
        let defaults = Self::default();
        let appearance = stored.appearance.unwrap_or_default();
        let messaging = stored.messaging.unwrap_or_default();
        let alerts = stored.alerts.unwrap_or_default();
        let calls = stored.calls.unwrap_or_default();
        let remember_devices = calls
            .remember_devices
            .unwrap_or(defaults.calls.remember_devices);
        let devices = calls
            .devices
            .filter(|_| remember_devices)
            .unwrap_or_default();
        let media = stored.media.unwrap_or_default();
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
                share_presence: messaging
                    .share_presence
                    .unwrap_or(defaults.messaging.share_presence),
            },
            alerts: Alerts {
                desktop_notifications: alerts
                    .desktop_notifications
                    .unwrap_or(defaults.alerts.desktop_notifications),
                hide_message_text: alerts
                    .hide_message_text
                    .unwrap_or(defaults.alerts.hide_message_text),
                notification_sound: alerts
                    .notification_sound
                    .unwrap_or(defaults.alerts.notification_sound),
                ringtone: alerts.ringtone.unwrap_or(defaults.alerts.ringtone),
            },
            calls: CallPreferences {
                remember_devices,
                camera_on_join: calls
                    .camera_on_join
                    .unwrap_or(defaults.calls.camera_on_join),
                layout: calls.layout.map_or(defaults.calls.layout, Into::into),
                follow_speaker: calls
                    .follow_speaker
                    .unwrap_or(defaults.calls.follow_speaker),
            },
            remembered_devices: ByKind {
                microphone: device_choice(devices.microphone),
                speaker: device_choice(devices.speaker),
                camera: device_choice(devices.camera),
            },
            media: MediaPreferences {
                auto_load: media.auto_load.unwrap_or(defaults.media.auto_load),
                play_animations: media
                    .play_animations
                    .unwrap_or(defaults.media.play_animations),
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
