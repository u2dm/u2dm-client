use super::AppService;
use crate::app::event::SettingsEvent;
use crate::commands::ui::SettingsCommand;
use crate::domain::settings::{Appearance, Messaging, Settings};

impl AppService {
    pub(super) fn spread_settings(&mut self) {
        let Settings {
            appearance:
                Appearance {
                    theme: _,
                    motion: _,
                },
            messaging:
                Messaging {
                    send_key: _,
                    receipts,
                    typing_notices,
                    share_presence,
                },
            alerts,
        } = *self.settings.values();
        self.active_timeline.set_receipts(receipts);
        self.room_info.set_receipts(receipts);
        self.typing.set_notices(typing_notices);
        self.presence.set_sharing(share_presence);
        self.notifications.set_alerts(alerts);
        self.calls.set_alerts(alerts, &self.room_directory);
    }

    pub(super) fn handle_settings_command(&mut self, command: SettingsCommand) {
        match command {
            SettingsCommand::OpenSettings => self.settings.open(),
            SettingsCommand::CloseSettings => self.settings.close(),
            SettingsCommand::ShowPage(page) => self.settings.show_page(page),
            SettingsCommand::Change(change) => {
                self.settings.change(change);
                self.spread_settings();
            }
        }
    }

    pub(super) fn handle_settings_event(&self, event: SettingsEvent) {
        match event {
            SettingsEvent::SaveFailed => self.settings.save_failed(),
        }
    }
}
