#[derive(Clone, Copy)]
pub(in crate::app) enum SettingsEvent {
    SaveFailed,
}

impl SettingsEvent {
    pub(in crate::app) const fn label(self) -> &'static str {
        match self {
            Self::SaveFailed => "SettingsSaveFailed",
        }
    }
}
