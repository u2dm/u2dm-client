use slint::ComponentHandle;

use super::run_change_handlers_next_frame;
use crate::adapters::ui::props::{BoolProp, UiProps};
use crate::commands::view::{SettingsPage, SettingsView};
use crate::domain::settings::{Appearance, Settings};

pub(in crate::adapters::ui) fn seed_settings<W: UiProps>(w: &W, values: &Settings) {
    apply_values(w, values);
}

pub(super) fn apply_settings<W: UiProps + ComponentHandle>(
    w: &W,
    last: Option<&SettingsView>,
    view: &SettingsView,
) {
    if last == Some(view) {
        return;
    }
    let SettingsView { values, dialog } = view;
    w.set_bool(BoolProp::SettingsVisible, dialog.is_some());
    w.set_settings_page(dialog.map_or(SettingsPage::default(), |dialog| dialog.page));
    apply_values(w, values);
    if last.is_none_or(|last| last.values.appearance.theme != values.appearance.theme) {
        run_change_handlers_next_frame(w);
    }
}

fn apply_values<W: UiProps>(w: &W, values: &Settings) {
    let Settings {
        appearance: Appearance { theme, motion },
    } = *values;
    w.set_appearance_theme(theme);
    w.set_motion_choice(motion);
}
