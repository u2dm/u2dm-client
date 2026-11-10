use crate::domain::media::MediaUsage;
use crate::domain::member_name::MemberName;
use crate::domain::server_rules::RoomDefault;
use crate::domain::settings::Settings;
use crate::domain::user_info::UserId;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub values: Settings,
    pub unsaved: bool,
    pub dialog: Option<SettingsDialog>,
    pub media_cleared: u64,
    pub rules: NotificationRulesView,
    pub ignored: IgnoredUsersView,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsDialog {
    pub page: SettingsPage,
    pub listing: bool,
    pub layout: SettingsLayout,
    pub storage: DownloadedMedia,
    pub clear_failed: bool,
}

impl SettingsDialog {
    pub fn shows(&self, page: SettingsPage) -> bool {
        !self.listing && self.page == page
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsLayout {
    #[default]
    Stacked,
    SideBySide,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPage {
    #[default]
    Appearance,
    Messages,
    Notifications,
    Calls,
    Media,
    Rules,
    Ignored,
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

#[derive(Clone, Default, PartialEq, Eq)]
pub struct IgnoredUsersView {
    pub known: bool,
    pub failed: Option<String>,
    pub people: Vec<IgnoredPerson>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IgnoredPerson {
    pub user_id: UserId,
    pub name: MemberName,
    pub busy: bool,
}
