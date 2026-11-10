use super::AppService;
use crate::app::event::{ActionOutcome, SettingsEvent};
use crate::app::settings::PagePorts;
use crate::commands::ui::SettingsCommand;
use crate::domain::settings::{Appearance, MediaPreferences, Messaging, Settings};
use crate::domain::user_info::IgnoreChange;

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

    fn page_ports(&self) -> PagePorts {
        PagePorts {
            media: self.port(|session| &session.media),
            rules: self.port(|session| &session.server_rules),
            people: self.port(|session| &session.user_info),
        }
    }

    pub(super) fn handle_settings_command(&mut self, command: SettingsCommand) {
        match command {
            SettingsCommand::Open(page) => {
                let ports = self.page_ports();
                self.settings.open(page, ports);
            }
            SettingsCommand::CloseSettings => self.settings.close(),
            SettingsCommand::ShowPage(page) => {
                let ports = self.page_ports();
                self.settings.show_page(page, ports);
            }
            SettingsCommand::ShowList => self.settings.show_list(),
            SettingsCommand::Layout(layout) => {
                let ports = self.page_ports();
                self.settings.set_layout(layout, ports);
            }
            SettingsCommand::Rule(command) => {
                if let Some(port) = self.port(|session| &session.server_rules) {
                    self.settings.change_rule(port, command);
                }
            }
            SettingsCommand::StopIgnoring(user_id) => {
                if let Some(port) = self.port(|session| &session.user_info) {
                    self.settings.stop_ignoring(port, user_id);
                }
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
            SettingsEvent::SaveRecovered => self.settings.save_recovered(),
            SettingsEvent::DevicesChosen(devices) => self.settings.remember_devices(*devices),
            SettingsEvent::MediaMeasured(usage) => self.settings.media_measured(usage),
            SettingsEvent::RulesChanged(rules) => self.settings.rules_followed(*rules),
            SettingsEvent::RuleWritten {
                request,
                slot,
                outcome,
            } => self.settings.rule_written(request, &slot, outcome),
            SettingsEvent::IgnoredListed(listed) => self.settings.ignored_listed(listed),
            SettingsEvent::IgnoredNamed {
                generation,
                user_id,
                name,
            } => self.settings.ignored_named(generation, user_id, name),
            SettingsEvent::Unignored {
                request,
                user_id,
                outcome,
            } => {
                self.settings.unignored(request, &user_id, outcome);
                if outcome == ActionOutcome::Done {
                    self.user_info
                        .ignore_changed_elsewhere(&user_id, IgnoreChange::Unignore);
                }
            }
            SettingsEvent::MediaCleared(outcome) => {
                let media = self.port(|session| &session.media);
                self.settings.media_cleared(outcome, media);
                self.active_timeline.recheck_media();
            }
        }
    }
}
