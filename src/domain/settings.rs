pub mod quick_reactions;

use self::quick_reactions::{QuickReactionChange, QuickReactions};
use crate::domain::call::devices::DeviceChoices;
use crate::domain::message::ServiceEvent;

pub const SKIN_TONE_COUNT: u8 = 6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MotionChoice {
    #[default]
    System,
    Reduced,
    Full,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClockChoice {
    #[default]
    System,
    TwelveHour,
    TwentyFourHour,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SendKey {
    #[default]
    Enter,
    CtrlEnter,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReceiptVisibility {
    #[default]
    Public,
    Private,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CallLayout {
    #[default]
    Automatic,
    CallAndChat,
    CallOnly,
    ChatOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkinTone(u8);

impl SkinTone {
    pub const fn new(index: u8) -> Option<Self> {
        if index < SKIN_TONE_COUNT {
            Some(Self(index))
        } else {
            None
        }
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingFlag {
    TypingNotices,
    SharePresence,
    DesktopNotifications,
    HideMessageText,
    NotificationSound,
    Ringtone,
    RememberDevices,
    CameraOnJoin,
    FollowSpeaker,
    AutoLoadMedia,
    PlayAnimations,
    VoiceInARow,
    JoinsAndLeaves,
    ProfileChanges,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    pub theme: ThemeChoice,
    pub motion: MotionChoice,
    pub clock: ClockChoice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Messaging {
    pub send_key: SendKey,
    pub receipts: ReceiptVisibility,
    pub typing_notices: bool,
    pub share_presence: bool,
}

impl Default for Messaging {
    fn default() -> Self {
        Self {
            send_key: SendKey::default(),
            receipts: ReceiptVisibility::default(),
            typing_notices: true,
            share_presence: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowFilter {
    pub joins_and_leaves: bool,
    pub profile_changes: bool,
}

impl Default for RowFilter {
    fn default() -> Self {
        Self {
            joins_and_leaves: true,
            profile_changes: true,
        }
    }
}

impl RowFilter {
    pub const fn shows(self, service: &ServiceEvent) -> bool {
        match service {
            ServiceEvent::Joined
            | ServiceEvent::Left
            | ServiceEvent::InvitationAccepted
            | ServiceEvent::InvitationRejected => self.joins_and_leaves,
            ServiceEvent::DisplayNameSet { .. }
            | ServiceEvent::DisplayNameChanged { .. }
            | ServiceEvent::DisplayNameRemoved
            | ServiceEvent::AvatarChanged => self.profile_changes,
            _ => true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReactionPreferences {
    pub quick: QuickReactions,
    pub skin_tone: SkinTone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Alerts {
    pub desktop_notifications: bool,
    pub hide_message_text: bool,
    pub notification_sound: bool,
    pub ringtone: bool,
}

impl Default for Alerts {
    fn default() -> Self {
        Self {
            desktop_notifications: true,
            hide_message_text: false,
            notification_sound: true,
            ringtone: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallPreferences {
    pub remember_devices: bool,
    pub camera_on_join: bool,
    pub layout: CallLayout,
    pub follow_speaker: bool,
}

impl Default for CallPreferences {
    fn default() -> Self {
        Self {
            remember_devices: true,
            camera_on_join: false,
            layout: CallLayout::default(),
            follow_speaker: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaPreferences {
    pub auto_load: bool,
    pub play_animations: bool,
    pub voice_in_a_row: bool,
}

impl Default for MediaPreferences {
    fn default() -> Self {
        Self {
            auto_load: true,
            play_animations: true,
            voice_in_a_row: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub appearance: Appearance,
    pub messaging: Messaging,
    pub timeline: RowFilter,
    pub reactions: ReactionPreferences,
    pub alerts: Alerts,
    pub calls: CallPreferences,
    pub remembered_devices: DeviceChoices,
    pub media: MediaPreferences,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingChange {
    Theme(ThemeChoice),
    Motion(MotionChoice),
    Clock(ClockChoice),
    SendKey(SendKey),
    Receipts(ReceiptVisibility),
    CallLayout(CallLayout),
    SkinTone(SkinTone),
    Flag(SettingFlag, bool),
}

impl Settings {
    pub fn apply(&mut self, change: SettingChange) -> bool {
        match change {
            SettingChange::Theme(theme) => replaced(&mut self.appearance.theme, theme),
            SettingChange::Motion(motion) => replaced(&mut self.appearance.motion, motion),
            SettingChange::Clock(clock) => replaced(&mut self.appearance.clock, clock),
            SettingChange::SendKey(key) => replaced(&mut self.messaging.send_key, key),
            SettingChange::Receipts(receipts) => replaced(&mut self.messaging.receipts, receipts),
            SettingChange::CallLayout(layout) => replaced(&mut self.calls.layout, layout),
            SettingChange::SkinTone(tone) => replaced(&mut self.reactions.skin_tone, tone),
            SettingChange::Flag(flag, on) => {
                let changed = replaced(self.flag_mut(flag), on);
                if !self.calls.remember_devices {
                    self.remembered_devices = DeviceChoices::default();
                }
                changed
            }
        }
    }

    pub fn change_quick_reactions(&mut self, change: QuickReactionChange) -> bool {
        self.reactions.quick.apply(change)
    }

    pub fn remember_devices(&mut self, devices: DeviceChoices) -> bool {
        self.calls.remember_devices && replaced(&mut self.remembered_devices, devices)
    }

    const fn flag_mut(&mut self, flag: SettingFlag) -> &mut bool {
        match flag {
            SettingFlag::TypingNotices => &mut self.messaging.typing_notices,
            SettingFlag::SharePresence => &mut self.messaging.share_presence,
            SettingFlag::DesktopNotifications => &mut self.alerts.desktop_notifications,
            SettingFlag::HideMessageText => &mut self.alerts.hide_message_text,
            SettingFlag::NotificationSound => &mut self.alerts.notification_sound,
            SettingFlag::Ringtone => &mut self.alerts.ringtone,
            SettingFlag::RememberDevices => &mut self.calls.remember_devices,
            SettingFlag::CameraOnJoin => &mut self.calls.camera_on_join,
            SettingFlag::FollowSpeaker => &mut self.calls.follow_speaker,
            SettingFlag::AutoLoadMedia => &mut self.media.auto_load,
            SettingFlag::PlayAnimations => &mut self.media.play_animations,
            SettingFlag::VoiceInARow => &mut self.media.voice_in_a_row,
            SettingFlag::JoinsAndLeaves => &mut self.timeline.joins_and_leaves,
            SettingFlag::ProfileChanges => &mut self.timeline.profile_changes,
        }
    }
}

fn replaced<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}
