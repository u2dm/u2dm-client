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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingFlag {
    TypingNotices,
    SharePresence,
    DesktopNotifications,
    HideMessageText,
    NotificationSound,
    Ringtone,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    pub theme: ThemeChoice,
    pub motion: MotionChoice,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub appearance: Appearance,
    pub messaging: Messaging,
    pub alerts: Alerts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingChange {
    Theme(ThemeChoice),
    Motion(MotionChoice),
    SendKey(SendKey),
    Receipts(ReceiptVisibility),
    Flag(SettingFlag, bool),
}

impl Settings {
    pub fn apply(&mut self, change: SettingChange) -> bool {
        match change {
            SettingChange::Theme(theme) => replaced(&mut self.appearance.theme, theme),
            SettingChange::Motion(motion) => replaced(&mut self.appearance.motion, motion),
            SettingChange::SendKey(key) => replaced(&mut self.messaging.send_key, key),
            SettingChange::Receipts(receipts) => replaced(&mut self.messaging.receipts, receipts),
            SettingChange::Flag(flag, on) => replaced(self.flag_mut(flag), on),
        }
    }

    const fn flag_mut(&mut self, flag: SettingFlag) -> &mut bool {
        match flag {
            SettingFlag::TypingNotices => &mut self.messaging.typing_notices,
            SettingFlag::SharePresence => &mut self.messaging.share_presence,
            SettingFlag::DesktopNotifications => &mut self.alerts.desktop_notifications,
            SettingFlag::HideMessageText => &mut self.alerts.hide_message_text,
            SettingFlag::NotificationSound => &mut self.alerts.notification_sound,
            SettingFlag::Ringtone => &mut self.alerts.ringtone,
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
