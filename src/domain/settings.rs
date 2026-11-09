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
pub struct Appearance {
    pub theme: ThemeChoice,
    pub motion: MotionChoice,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub appearance: Appearance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingChange {
    Theme(ThemeChoice),
    Motion(MotionChoice),
}

impl Settings {
    pub fn apply(&mut self, change: SettingChange) -> bool {
        match change {
            SettingChange::Theme(theme) => replaced(&mut self.appearance.theme, theme),
            SettingChange::Motion(motion) => replaced(&mut self.appearance.motion, motion),
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
