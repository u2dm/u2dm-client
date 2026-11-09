use serde::Serialize;

use crate::adapters::demo::probe::names;
use crate::commands::view::SettingsView;
use crate::domain::settings::{Appearance, Messaging, Settings};

#[derive(Serialize)]
pub(super) struct SettingsDto {
    values: SettingsValuesDto,
    dialog: Option<SettingsDialogDto>,
}

#[derive(Serialize)]
struct SettingsValuesDto {
    theme: &'static str,
    motion: &'static str,
    send_key: &'static str,
    receipts: &'static str,
    typing_notices: bool,
    share_presence: bool,
}

#[derive(Serialize)]
struct SettingsDialogDto {
    page: &'static str,
}

pub(super) fn settings(source: &SettingsView) -> SettingsDto {
    let Settings {
        appearance: Appearance { theme, motion },
        messaging:
            Messaging {
                send_key,
                receipts,
                typing_notices,
                share_presence,
            },
    } = source.values;
    SettingsDto {
        values: SettingsValuesDto {
            theme: names::theme_choice(theme),
            motion: names::motion_choice(motion),
            send_key: names::send_key(send_key),
            receipts: names::receipt_visibility(receipts),
            typing_notices,
            share_presence,
        },
        dialog: source.dialog.map(|dialog| SettingsDialogDto {
            page: names::settings_page(dialog.page),
        }),
    }
}
