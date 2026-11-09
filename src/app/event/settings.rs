use crate::commands::messages::UserMessageKind;
use crate::domain::call::devices::DeviceChoices;
use crate::domain::media::MediaUsage;

pub(in crate::app) enum SettingsEvent {
    SaveFailed,
    DevicesChosen(Box<DeviceChoices>),
    MediaMeasured(Option<MediaUsage>),
    MediaCleared(Result<MediaUsage, UserMessageKind>),
}

impl SettingsEvent {
    pub(in crate::app) const fn label(&self) -> &'static str {
        match self {
            Self::SaveFailed => "SettingsSaveFailed",
            Self::DevicesChosen(_) => "SettingsDevicesChosen",
            Self::MediaMeasured(_) => "SettingsMediaMeasured",
            Self::MediaCleared(_) => "SettingsMediaCleared",
        }
    }
}
