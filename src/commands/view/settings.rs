use crate::domain::media::MediaUsage;
use crate::domain::server_rules::RoomDefault;
use crate::domain::settings::Settings;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub values: Settings,
    pub dialog: Option<SettingsDialog>,
    pub media_cleared: u64,
    pub rules: NotificationRulesView,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsDialog {
    pub page: SettingsPage,
    pub storage: DownloadedMedia,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPage {
    #[default]
    Overview,
    Appearance,
    Messages,
    Notifications,
    Calls,
    Media,
    Rules,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DownloadedMedia {
    #[default]
    Measuring,
    Measured(MediaUsage),
    Clearing(MediaUsage),
    Unknown,
}

impl DownloadedMedia {
    pub const fn usage(self) -> Option<MediaUsage> {
        match self {
            Self::Measured(usage) | Self::Clearing(usage) => Some(usage),
            Self::Measuring | Self::Unknown => None,
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct NotificationRulesView {
    pub known: bool,
    pub failed: bool,
    pub account: AccountRule,
    pub direct: RoomDefault,
    pub group: RoomDefault,
    pub keywords: Vec<KeywordRow>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccountRule {
    #[default]
    Missing,
    On,
    Off,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeywordRow {
    pub keyword: String,
    pub busy: bool,
}
