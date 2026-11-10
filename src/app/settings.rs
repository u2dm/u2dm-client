mod rules;

use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::watch;

use self::rules::NotificationRules;
use super::event::{ActionOutcome, AppEvent, RuleSlot, SettingsEvent};
use super::input::EventSender;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::RuleCommand;
use crate::commands::view::{DownloadedMedia, SettingsDialog, SettingsPage, SettingsView, Toast};
use crate::domain::call::devices::DeviceChoices;
use crate::domain::media::MediaUsage;
use crate::domain::server_rules::ServerRules;
use crate::domain::settings::{SettingChange, Settings};
use crate::ports::matrix::{MediaPort, ServerRulesPort};
use crate::ports::output::AppOutputPort;
use crate::ports::settings::SettingsPort;

pub(super) struct PagePorts {
    pub(super) media: Option<Arc<dyn MediaPort>>,
    pub(super) rules: Option<Arc<dyn ServerRulesPort>>,
}

pub(super) struct SettingsController {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    values: Settings,
    lane: Option<watch::Sender<Settings>>,
    saves: TaskGroup,
    storage: TaskGroup,
    dialog: Option<SettingsDialog>,
    media_cleared: u64,
    rules: NotificationRules,
}

impl SettingsController {
    pub(super) fn new(
        output: Arc<dyn AppOutputPort>,
        events: EventSender,
        port: Arc<dyn SettingsPort>,
        loaded: Settings,
    ) -> Self {
        let (lane, changes) = watch::channel(loaded.clone());
        let mut saves = TaskGroup::new("settings-saves");
        saves.spawn(save_newest(port, events.clone(), changes));
        Self {
            output,
            rules: NotificationRules::new(events.clone()),
            events,
            values: loaded,
            lane: Some(lane),
            saves,
            storage: TaskGroup::new("settings-storage"),
            dialog: None,
            media_cleared: 0,
        }
    }

    pub(super) const fn values(&self) -> &Settings {
        &self.values
    }

    pub(super) fn open(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        tracing::debug!("opening the settings dialog");
        self.dialog = Some(SettingsDialog::default());
        self.publish();
    }

    pub(super) fn close(&mut self) {
        if self.dialog.take().is_none() {
            return;
        }
        tracing::debug!("closing the settings dialog");
        self.rules.stop();
        self.publish();
    }

    pub(super) fn show_page(&mut self, page: SettingsPage, ports: PagePorts) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if dialog.page == page {
            return;
        }
        tracing::debug!(?page, "showing a settings page");
        let left = mem::replace(&mut dialog.page, page);
        if left == SettingsPage::Rules {
            self.rules.stop();
        }
        if page == SettingsPage::Media {
            self.measure_media(ports.media);
        }
        if let (SettingsPage::Rules, Some(port)) = (page, ports.rules) {
            self.rules.start(port);
        }
        self.publish();
    }

    pub(super) fn change_rule(&mut self, port: Arc<dyn ServerRulesPort>, command: RuleCommand) {
        match command {
            RuleCommand::SetAccountNotifications(on) => self.rules.set_account(port, on),
            RuleCommand::SetRoomDefault(rooms, mode) => {
                self.rules.set_room_default(port, rooms, mode);
            }
            RuleCommand::AddKeyword(keyword) => self.rules.add_keyword(port, keyword),
            RuleCommand::RemoveKeyword(keyword) => self.rules.remove_keyword(port, keyword),
        }
        self.publish();
    }

    pub(super) fn rules_followed(&mut self, rules: ServerRules) {
        self.rules.followed(rules);
        self.publish();
    }

    pub(super) fn rule_written(&mut self, request: u64, slot: &RuleSlot, outcome: ActionOutcome) {
        self.rules.written(request, slot, outcome);
        if outcome == ActionOutcome::Failed && !self.rules.show_failure() {
            show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::new(
                    UserMessageKind::NotificationRulesNotChanged,
                )),
            );
        }
        self.publish();
    }

    fn measure_media(&mut self, media: Option<Arc<dyn MediaPort>>) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if matches!(dialog.storage, DownloadedMedia::Clearing(_)) {
            return;
        }
        let Some(media) = media else {
            dialog.storage = DownloadedMedia::Unknown;
            return;
        };
        self.storage.spawn_reporting(&self.events, async move {
            let usage = match media.cache_usage().await {
                Ok(usage) => Some(usage),
                Err(e) => {
                    tracing::warn!("the downloaded media could not be measured: {e}");
                    None
                }
            };
            AppEvent::Settings(SettingsEvent::MediaMeasured(usage))
        });
    }

    pub(super) fn media_measured(&mut self, usage: Option<MediaUsage>) {
        let Some(dialog) = self
            .dialog
            .as_mut()
            .filter(|dialog| !matches!(dialog.storage, DownloadedMedia::Clearing(_)))
        else {
            return;
        };
        dialog.storage = usage.map_or(DownloadedMedia::Unknown, DownloadedMedia::Measured);
        self.publish();
    }

    pub(super) fn clear_media(&mut self, media: Option<Arc<dyn MediaPort>>, keep: Vec<PathBuf>) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        let (DownloadedMedia::Measured(usage), Some(media)) = (dialog.storage, media) else {
            tracing::debug!("not clearing downloaded media that is not measured");
            return;
        };
        tracing::info!(keeping = keep.len(), "clearing downloaded media");
        dialog.storage = DownloadedMedia::Clearing(usage);
        self.storage.spawn_reporting(&self.events, async move {
            let outcome = media.clear_cache(keep).await.map_err(|e| {
                tracing::warn!("the downloaded media could not be cleared: {e}");
                UserMessageKind::DownloadedMediaNotCleared
            });
            AppEvent::Settings(SettingsEvent::MediaCleared(outcome))
        });
        self.publish();
    }

    pub(super) fn media_cleared(
        &mut self,
        outcome: Result<MediaUsage, UserMessageKind>,
        media: Option<Arc<dyn MediaPort>>,
    ) {
        self.media_cleared = self.media_cleared.wrapping_add(1);
        let clearing = self
            .dialog
            .as_mut()
            .filter(|dialog| matches!(dialog.storage, DownloadedMedia::Clearing(_)));
        match outcome {
            Ok(usage) => {
                if let Some(dialog) = clearing {
                    dialog.storage = DownloadedMedia::Measured(usage);
                }
            }
            Err(kind) => {
                if let Some(dialog) = clearing {
                    dialog.storage = DownloadedMedia::Measuring;
                }
                show_toast(self.output.as_ref(), Toast::Error(UserMessage::new(kind)));
                self.measure_media(media);
            }
        }
        self.publish();
    }

    pub(super) fn change(&mut self, change: SettingChange) {
        if !self.values.apply(change) {
            return;
        }
        tracing::info!(?change, "a setting changed");
        self.save_and_publish();
    }

    pub(super) fn remember_devices(&mut self, devices: DeviceChoices) {
        if self.values.remember_devices(devices) {
            tracing::debug!("remembering the call devices you chose");
            self.save_and_publish();
        }
    }

    pub(super) fn save_failed(&self) {
        show_toast(
            self.output.as_ref(),
            Toast::Error(UserMessage::new(UserMessageKind::SettingsNotSaved)),
        );
    }

    pub(super) fn close_with_session(&mut self) -> SettingsView {
        self.storage.cancel_and_detach();
        self.rules.forget_session();
        self.dialog = None;
        self.view()
    }

    pub(super) async fn shutdown(&mut self) {
        self.lane = None;
        self.saves.drain().await;
        self.storage.shutdown().await;
        self.rules.shutdown().await;
    }

    fn save_and_publish(&self) {
        if let Some(lane) = &self.lane {
            lane.send_replace(self.values.clone());
        }
        self.publish();
    }

    fn view(&self) -> SettingsView {
        SettingsView {
            values: self.values.clone(),
            dialog: self.dialog,
            media_cleared: self.media_cleared,
            rules: self.rules.view(),
        }
    }

    fn publish(&self) {
        let shown = self.view();
        self.output
            .publish(Box::new(move |view| view.settings = shown));
    }
}

async fn save_newest(
    port: Arc<dyn SettingsPort>,
    events: EventSender,
    mut changes: watch::Receiver<Settings>,
) {
    while changes.changed().await.is_ok() {
        let newest = changes.borrow_and_update().clone();
        if let Err(e) = port.save(&newest).await {
            tracing::warn!("the settings could not be saved: {e}");
            if events
                .send(AppEvent::Settings(SettingsEvent::SaveFailed))
                .is_err()
            {
                tracing::debug!("the app stopped before hearing that the settings were not saved");
            }
        }
    }
}
