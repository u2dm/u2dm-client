use serde::Serialize;

use crate::adapters::demo::probe::names;
use crate::commands::view::SettingsView;
use crate::domain::settings::{Appearance, Settings};

#[derive(Serialize)]
pub(super) struct SettingsDto {
    values: SettingsValuesDto,
    dialog: Option<SettingsDialogDto>,
}

#[derive(Serialize)]
struct SettingsValuesDto {
    theme: &'static str,
    motion: &'static str,
}

#[derive(Serialize)]
struct SettingsDialogDto {
    page: &'static str,
}

pub(super) fn settings(source: &SettingsView) -> SettingsDto {
    let Settings {
        appearance: Appearance { theme, motion },
    } = source.values;
    SettingsDto {
        values: SettingsValuesDto {
            theme: names::theme_choice(theme),
            motion: names::motion_choice(motion),
        },
        dialog: source.dialog.map(|dialog| SettingsDialogDto {
            page: names::settings_page(dialog.page),
        }),
    }
}
