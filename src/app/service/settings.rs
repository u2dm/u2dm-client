use super::AppService;
use crate::app::event::SettingsEvent;
use crate::commands::ui::SettingsCommand;

impl AppService {
    pub(super) fn handle_settings_command(&mut self, command: SettingsCommand) {
        match command {
            SettingsCommand::OpenSettings => self.settings.open(),
            SettingsCommand::CloseSettings => self.settings.close(),
            SettingsCommand::ShowPage(page) => self.settings.show_page(page),
            SettingsCommand::Change(change) => self.settings.change(change),
        }
    }

    pub(super) fn handle_settings_event(&self, event: SettingsEvent) {
        match event {
            SettingsEvent::SaveFailed => self.settings.save_failed(),
        }
    }
}
