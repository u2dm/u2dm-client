use crate::domain::media::MediaUsage;
use crate::domain::settings::Settings;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub values: Settings,
    pub dialog: Option<SettingsDialog>,
    pub media_cleared: u64,
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
