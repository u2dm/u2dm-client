use serde::Serialize;

use crate::adapters::demo::probe::names;
use crate::commands::view::{DownloadedMedia, SettingsView};
use crate::domain::call::devices::{ByKind, DeviceChoice};
use crate::domain::settings::{
    Alerts, Appearance, CallPreferences, MediaPreferences, Messaging, Settings,
};

#[derive(Serialize)]
pub(super) struct SettingsDto {
    values: SettingsValuesDto,
    dialog: Option<SettingsDialogDto>,
    media_cleared: u64,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct SettingsValuesDto {
    theme: &'static str,
    motion: &'static str,
    send_key: &'static str,
    receipts: &'static str,
    typing_notices: bool,
    share_presence: bool,
    desktop_notifications: bool,
    hide_message_text: bool,
    notification_sound: bool,
    ringtone: bool,
    remember_devices: bool,
    camera_on_join: bool,
    call_layout: &'static str,
    follow_speaker: bool,
    remembered_devices: RememberedDevicesDto,
    auto_load_media: bool,
    play_animations: bool,
}

#[derive(Serialize)]
struct RememberedDevicesDto {
    microphone: Option<String>,
    speaker: Option<String>,
    camera: Option<String>,
}

impl From<&ByKind<DeviceChoice>> for RememberedDevicesDto {
    fn from(devices: &ByKind<DeviceChoice>) -> Self {
        let key_of = |choice: &DeviceChoice| choice.key().map(ToString::to_string);
        Self {
            microphone: key_of(&devices.microphone),
            speaker: key_of(&devices.speaker),
            camera: key_of(&devices.camera),
        }
    }
}

#[derive(Serialize)]
struct SettingsDialogDto {
    page: &'static str,
    storage: StorageDto,
}

#[derive(Serialize)]
struct StorageDto {
    state: &'static str,
    clearable_bytes: Option<u64>,
    kept_bytes: Option<u64>,
}

impl From<DownloadedMedia> for StorageDto {
    fn from(storage: DownloadedMedia) -> Self {
        let usage = storage.usage();
        Self {
            state: names::downloaded_media_state(storage),
            clearable_bytes: usage.map(|usage| usage.clearable),
            kept_bytes: usage.map(|usage| usage.kept),
        }
    }
}

pub(super) fn settings(source: &SettingsView) -> SettingsDto {
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
        media: MediaPreferences {
            auto_load,
            play_animations,
        },
    } = source.values;
    SettingsDto {
        values: SettingsValuesDto {
            theme: names::theme_choice(theme),
            motion: names::motion_choice(motion),
            send_key: names::send_key(send_key),
            receipts: names::receipt_visibility(receipts),
            typing_notices,
            share_presence,
            desktop_notifications,
            hide_message_text,
            notification_sound,
            ringtone,
            remember_devices,
            camera_on_join,
            call_layout: names::call_layout(layout),
            follow_speaker,
            remembered_devices: remembered_devices.into(),
            auto_load_media: auto_load,
            play_animations,
        },
        dialog: source.dialog.map(|dialog| SettingsDialogDto {
            page: names::settings_page(dialog.page),
            storage: dialog.storage.into(),
        }),
        media_cleared: source.media_cleared,
    }
}
