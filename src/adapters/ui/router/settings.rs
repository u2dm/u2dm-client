use super::Tx;
use crate::adapters::ui::props::send_command;
use crate::adapters::ui::schema::{motion_choices, settings_pages, theme_choices, variant_named};
use crate::commands::ui::SettingsCommand;
use crate::commands::view::SettingsPage;
use crate::domain::settings::{MotionChoice, SettingChange, ThemeChoice};

settings_pages!(variant_named settings_page_named SettingsPage;);
theme_choices!(variant_named theme_choice_named ThemeChoice;);
motion_choices!(variant_named motion_choice_named MotionChoice;);

pub fn show_settings_page(tx: &Tx, page: &str) {
    let Some(page) = settings_page_named(page) else {
        tracing::warn!(page, "ignoring an unknown settings page");
        return;
    };
    send_command(tx, SettingsCommand::ShowPage(page));
}

pub fn set_theme(tx: &Tx, choice: &str) {
    let Some(theme) = theme_choice_named(choice) else {
        tracing::warn!(choice, "ignoring an unknown theme");
        return;
    };
    send_command(tx, SettingsCommand::Change(SettingChange::Theme(theme)));
}

pub fn set_motion(tx: &Tx, choice: &str) {
    let Some(motion) = motion_choice_named(choice) else {
        tracing::warn!(choice, "ignoring an unknown motion choice");
        return;
    };
    send_command(tx, SettingsCommand::Change(SettingChange::Motion(motion)));
}
