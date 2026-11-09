use std::sync::Arc;

use tokio::sync::watch;

use super::event::{AppEvent, SettingsEvent};
use super::input::EventSender;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{SettingsDialog, SettingsPage, SettingsView, Toast};
use crate::domain::settings::{SettingChange, Settings};
use crate::ports::output::AppOutputPort;
use crate::ports::settings::SettingsPort;

pub(super) struct SettingsController {
    output: Arc<dyn AppOutputPort>,
    values: Settings,
    lane: Option<watch::Sender<Settings>>,
    saves: TaskGroup,
    dialog: Option<SettingsDialog>,
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
        saves.spawn(save_newest(port, events, changes));
        Self {
            output,
            values: loaded,
            lane: Some(lane),
            saves,
            dialog: None,
        }
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
        self.publish();
    }

    pub(super) fn show_page(&mut self, page: SettingsPage) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if dialog.page == page {
            return;
        }
        tracing::debug!(?page, "showing a settings page");
        dialog.page = page;
        self.publish();
    }

    pub(super) fn change(&mut self, change: SettingChange) {
        if !self.values.apply(change) {
            return;
        }
        tracing::info!(?change, "a setting changed");
        if let Some(lane) = &self.lane {
            lane.send_replace(self.values.clone());
        }
        self.publish();
    }

    pub(super) fn save_failed(&self) {
        show_toast(
            self.output.as_ref(),
            Toast::Error(UserMessage::new(UserMessageKind::SettingsNotSaved)),
        );
    }

    pub(super) fn close_with_session(&mut self) -> SettingsView {
        self.dialog = None;
        self.view()
    }

    pub(super) async fn shutdown(&mut self) {
        self.lane = None;
        self.saves.drain().await;
    }

    fn view(&self) -> SettingsView {
        SettingsView {
            values: self.values.clone(),
            dialog: self.dialog,
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
