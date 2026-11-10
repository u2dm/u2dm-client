use super::ActionOutcome;
use crate::commands::messages::UserMessageKind;
use crate::domain::call::devices::DeviceChoices;
use crate::domain::media::MediaUsage;
use crate::domain::member_name::MemberName;
use crate::domain::server_rules::{RoomClass, ServerRules};
use crate::domain::user_info::UserId;

pub(in crate::app) enum RuleSlot {
    Account,
    RoomDefault(RoomClass),
    Keyword(String),
}

pub(in crate::app) enum SettingsEvent {
    SaveFailed,
    SaveRecovered,
    DevicesChosen(Box<DeviceChoices>),
    MediaMeasured(Option<MediaUsage>),
    MediaCleared(Result<MediaUsage, UserMessageKind>),
    RulesChanged(Box<ServerRules>),
    RuleWritten {
        request: u64,
        slot: RuleSlot,
        outcome: ActionOutcome,
    },
    IgnoredListed(Vec<UserId>),
    IgnoredNamed {
        generation: u64,
        user_id: UserId,
        name: MemberName,
    },
    Unignored {
        request: u64,
        user_id: UserId,
        outcome: ActionOutcome,
    },
}

impl SettingsEvent {
    pub(in crate::app) const fn label(&self) -> &'static str {
        match self {
            Self::SaveFailed => "SettingsSaveFailed",
            Self::SaveRecovered => "SettingsSaveRecovered",
            Self::DevicesChosen(_) => "SettingsDevicesChosen",
            Self::MediaMeasured(_) => "SettingsMediaMeasured",
            Self::MediaCleared(_) => "SettingsMediaCleared",
            Self::RulesChanged(_) => "SettingsRulesChanged",
            Self::RuleWritten { .. } => "SettingsRuleWritten",
            Self::IgnoredListed(_) => "SettingsIgnoredListed",
            Self::IgnoredNamed { .. } => "SettingsIgnoredNamed",
            Self::Unignored { .. } => "SettingsUnignored",
        }
    }
}
