use crate::domain::settings::Settings;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub values: Settings,
    pub dialog: Option<SettingsDialog>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsDialog {
    pub page: SettingsPage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPage {
    #[default]
    Overview,
    Appearance,
    Messages,
    Notifications,
}
