use super::Tx;
use crate::adapters::ui::props::send_command;
use crate::adapters::ui::schema::{
    motion_choices, receipt_visibilities, send_keys, setting_flags, settings_pages, theme_choices,
    variant_named,
};
use crate::commands::ui::SettingsCommand;
use crate::commands::view::SettingsPage;
use crate::domain::settings::{
    MotionChoice, ReceiptVisibility, SendKey, SettingChange, SettingFlag, ThemeChoice,
};

settings_pages!(variant_named settings_page_named SettingsPage;);
theme_choices!(variant_named theme_choice_named ThemeChoice;);
motion_choices!(variant_named motion_choice_named MotionChoice;);
send_keys!(variant_named send_key_named SendKey;);
receipt_visibilities!(variant_named receipt_visibility_named ReceiptVisibility;);
setting_flags!(variant_named setting_flag_named SettingFlag;);

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

pub fn set_send_key(tx: &Tx, choice: &str) {
    let Some(key) = send_key_named(choice) else {
        tracing::warn!(choice, "ignoring an unknown send key");
        return;
    };
    send_command(tx, SettingsCommand::Change(SettingChange::SendKey(key)));
}

pub fn set_receipts(tx: &Tx, choice: &str) {
    let Some(receipts) = receipt_visibility_named(choice) else {
        tracing::warn!(choice, "ignoring an unknown receipt visibility");
        return;
    };
    send_command(
        tx,
        SettingsCommand::Change(SettingChange::Receipts(receipts)),
    );
}

pub fn set_setting_flag(tx: &Tx, setting: &str, on: bool) {
    let Some(flag) = setting_flag_named(setting) else {
        tracing::warn!(setting, "ignoring an unknown setting");
        return;
    };
    send_command(tx, SettingsCommand::Change(SettingChange::Flag(flag, on)));
}
