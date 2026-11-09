use super::AppService;
use crate::app::event::SettingsEvent;
use crate::commands::ui::SettingsCommand;
use crate::domain::settings::{Appearance, MediaPreferences, Messaging, Settings};

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
            calls,
            remembered_devices: _,
            media:
                MediaPreferences {
                    auto_load: _,
                    play_animations: _,
                },
        } = *self.settings.values();
        self.active_timeline.set_receipts(receipts);
        self.room_info.set_receipts(receipts);
        self.typing.set_notices(typing_notices);
        self.presence.set_sharing(share_presence);
        self.notifications.set_alerts(alerts);
        self.calls.set_alerts(alerts, &self.room_directory);
        self.calls.set_preferences(calls);
        self.tell_session_media_loading();
    }

    pub(super) fn tell_session_media_loading(&self) {
        if let Some(session) = self.held_session.running() {
            session
                .media
                .set_auto_load(self.settings.values().media.auto_load);
        }
    }

    pub(super) fn handle_settings_command(&mut self, command: SettingsCommand) {
        match command {
            SettingsCommand::OpenSettings => self.settings.open(),
            SettingsCommand::CloseSettings => self.settings.close(),
            SettingsCommand::ShowPage(page) => {
                let media = self.port(|session| &session.media);
                self.settings.show_page(page, media);
            }
            SettingsCommand::ClearDownloadedMedia => {
                let media = self.port(|session| &session.media);
                let playing = [self.audio.playing_file(), self.video.playing_file()];
                self.settings
                    .clear_media(media, playing.into_iter().flatten().collect());
            }
            SettingsCommand::Change(change) => {
                let loaded_automatically = self.settings.values().media.auto_load;
                self.settings.change(change);
                self.spread_settings();
                if self.settings.values().media.auto_load != loaded_automatically {
                    self.active_timeline.recheck_media();
                }
            }
        }
    }

    pub(super) fn handle_settings_event(&mut self, event: SettingsEvent) {
        match event {
            SettingsEvent::SaveFailed => self.settings.save_failed(),
            SettingsEvent::DevicesChosen(devices) => self.settings.remember_devices(*devices),
            SettingsEvent::MediaMeasured(usage) => self.settings.media_measured(usage),
            SettingsEvent::MediaCleared(outcome) => {
                let media = self.port(|session| &session.media);
                self.settings.media_cleared(outcome, media);
                self.active_timeline.recheck_media();
            }
        }
    }
}
