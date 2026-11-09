use crate::domain::call::devices::DeviceChoices;

pub(in crate::app) enum SettingsEvent {
    SaveFailed,
    DevicesChosen(Box<DeviceChoices>),
}

impl SettingsEvent {
    pub(in crate::app) const fn label(&self) -> &'static str {
        match self {
            Self::SaveFailed => "SettingsSaveFailed",
            Self::DevicesChosen(_) => "SettingsDevicesChosen",
        }
    }
}
