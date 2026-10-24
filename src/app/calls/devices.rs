use std::sync::Arc;

use crate::commands::view::{CallDevices, DeviceNews, DeviceOption, DeviceStatus, DeviceTrouble};
use crate::domain::call::devices::{
    ByKind, DeviceChoice, DeviceChoices, DeviceKey, DeviceKind, DeviceLists, MediaDevice,
};

pub(super) struct Devices {
    choices: DeviceChoices,
    lists: DeviceLists,
    failed: ByKind<Option<DeviceKey>>,
    news: Option<DeviceNews>,
    serial: u64,
}

impl Devices {
    pub(super) fn new(choices: DeviceChoices) -> Self {
        Self {
            choices,
            lists: DeviceLists::default(),
            failed: ByKind::default(),
            news: None,
            serial: 0,
        }
    }

    pub(super) fn choice_for(
        &self,
        kind: DeviceKind,
        key: Option<&DeviceKey>,
    ) -> Option<DeviceChoice> {
        let Some(key) = key else {
            return Some(DeviceChoice::Default);
        };
        let listed = self
            .lists
            .connected
            .of(kind)
            .iter()
            .find(|device| device.key == *key)
            .cloned()
            .map(DeviceChoice::Device);
        let current = self.choices.of(kind);
        listed.or_else(|| (current.key() == Some(key)).then(|| current.clone()))
    }

    pub(super) fn choose(&mut self, kind: DeviceKind, choice: DeviceChoice) {
        self.retry(kind);
        *self.choices.of_mut(kind) = choice;
    }

    pub(super) fn retry(&mut self, kind: DeviceKind) {
        *self.failed.of_mut(kind) = None;
    }

    pub(super) fn listed(&mut self, lists: DeviceLists) {
        for kind in DeviceKind::ALL {
            if let Some(lost) = self.lost(kind, &lists) {
                tracing::info!(%kind, device = lost.name, "the device you chose was disconnected");
                self.tell(kind, lost.name, DeviceTrouble::Disconnected);
            }
            let failed = self.failed.of_mut(kind);
            if failed
                .as_ref()
                .is_some_and(|key| !lists.is_connected(kind, key))
            {
                *failed = None;
            }
        }
        self.lists = lists;
    }

    pub(super) const fn lists(&self) -> &DeviceLists {
        &self.lists
    }

    pub(super) fn uses_alike(&self, kind: DeviceKind, other: &DeviceChoice) -> bool {
        let connected = self.lists.connected.of(kind);
        let failed = self.failed.of(kind).as_ref();
        let used = self.choices.of(kind).resolve(kind, connected, failed);
        used.is_some() && used == other.resolve(kind, connected, failed)
    }

    fn lost(&self, kind: DeviceKind, now: &DeviceLists) -> Option<MediaDevice> {
        let DeviceChoice::Device(chosen) = self.choices.of(kind) else {
            return None;
        };
        let gone =
            self.lists.is_connected(kind, &chosen.key) && !now.is_connected(kind, &chosen.key);
        gone.then(|| chosen.clone())
    }

    pub(super) fn failed(&mut self, kind: DeviceKind, key: DeviceKey) {
        let name = self
            .lists
            .connected
            .of(kind)
            .iter()
            .find(|device| device.key == key)
            .map_or_else(|| key.to_string(), |device| device.name.clone());
        *self.failed.of_mut(kind) = Some(key);
        self.tell(kind, name, DeviceTrouble::Failed);
    }

    fn tell(&mut self, kind: DeviceKind, name: String, trouble: DeviceTrouble) {
        self.serial = self.serial.wrapping_add(1);
        self.news = Some(DeviceNews {
            serial: self.serial,
            kind,
            name,
            trouble,
        });
    }

    pub(super) fn view(&self) -> CallDevices {
        CallDevices {
            options: ByKind {
                microphone: self.options(DeviceKind::Microphone),
                speaker: self.options(DeviceKind::Speaker),
                camera: self.options(DeviceKind::Camera),
            },
            news: self.news.clone(),
        }
    }

    fn options(&self, kind: DeviceKind) -> Arc<[DeviceOption]> {
        let connected = self.lists.connected.of(kind);
        let choice = self.choices.of(kind);
        let failed = self.failed.of(kind).as_ref();
        let checked = match choice {
            DeviceChoice::Device(chosen) => Some(&chosen.key),
            DeviceChoice::Default => choice
                .resolve(kind, connected, failed)
                .map(|device| &device.key),
        };
        let mut options = Vec::with_capacity(connected.len() + 2);
        if kind.has_system_default() {
            options.push(DeviceOption {
                key: None,
                name: self.lists.defaults.of(kind).clone().unwrap_or_default(),
                status: DeviceStatus::SystemDefault,
                chosen: *choice == DeviceChoice::Default,
            });
        }
        options.extend(connected.iter().map(|device| DeviceOption {
            key: Some(device.key.clone()),
            name: device.name.clone(),
            status: if failed == Some(&device.key) {
                DeviceStatus::Failed
            } else {
                DeviceStatus::Connected
            },
            chosen: checked == Some(&device.key),
        }));
        if let DeviceChoice::Device(chosen) = choice
            && !self.lists.is_connected(kind, &chosen.key)
        {
            options.push(DeviceOption {
                key: Some(chosen.key.clone()),
                name: chosen.name.clone(),
                status: DeviceStatus::Missing,
                chosen: true,
            });
        }
        Arc::from(options)
    }
}
