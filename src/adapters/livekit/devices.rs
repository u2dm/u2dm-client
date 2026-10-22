use livekit::prelude::{PlatformAudio, PlayoutDeviceId, RecordingDeviceId};

use crate::adapters::video::camera::{CameraDevice, cameras};
use crate::domain::call::devices::{
    ByKind, DeviceKey, DeviceKind, DeviceLists, MediaDevice, numbered,
};

const SYSTEM_DEFAULT: u16 = 0;
const DEFAULT_PREFIX: &str = "default: ";
const UNNAMED_DEFAULT: &str = "default";

struct Indexed {
    index: u16,
    device: MediaDevice,
}

#[derive(Default)]
pub(super) struct Listing {
    microphones: Vec<Indexed>,
    speakers: Vec<Indexed>,
    cameras: Vec<CameraDevice>,
    lists: DeviceLists,
}

impl Listing {
    pub(super) fn read(audio: &PlatformAudio) -> Self {
        let (default_microphone, microphones) = indexed(
            audio
                .recording_devices()
                .map(|device| (device.index, device.name)),
        );
        let (default_speaker, speakers) = indexed(
            audio
                .playout_devices()
                .map(|device| (device.index, device.name)),
        );
        let cameras = cameras();
        let camera_names = numbered(cameras.iter().map(|camera| camera.name.clone()));
        let lists = DeviceLists {
            connected: ByKind {
                microphone: microphones
                    .iter()
                    .map(|entry| entry.device.clone())
                    .collect(),
                speaker: speakers.iter().map(|entry| entry.device.clone()).collect(),
                camera: cameras
                    .iter()
                    .zip(camera_names)
                    .map(|(camera, name)| MediaDevice {
                        key: DeviceKey::new(camera.key.clone()),
                        name,
                    })
                    .collect(),
            },
            defaults: ByKind {
                microphone: default_microphone,
                speaker: default_speaker,
                camera: None,
            },
        };
        Self {
            microphones,
            speakers,
            cameras,
            lists,
        }
    }

    pub(super) const fn lists(&self) -> &DeviceLists {
        &self.lists
    }

    pub(super) fn index_of(&self, kind: DeviceKind, device: Option<&MediaDevice>) -> u16 {
        let indexed = match kind {
            DeviceKind::Microphone => &self.microphones,
            DeviceKind::Speaker => &self.speakers,
            DeviceKind::Camera => return SYSTEM_DEFAULT,
        };
        device
            .and_then(|device| indexed.iter().find(|entry| entry.device.key == device.key))
            .map_or(SYSTEM_DEFAULT, |entry| entry.index)
    }

    pub(super) fn camera(&self, device: &MediaDevice) -> Option<CameraDevice> {
        self.cameras
            .iter()
            .find(|camera| camera.key == device.key.as_ref())
            .cloned()
    }
}

fn indexed(listed: impl Iterator<Item = (usize, String)>) -> (Option<String>, Vec<Indexed>) {
    let mut default = None;
    let mut devices = Vec::new();
    for (index, name) in listed {
        let Ok(index) = u16::try_from(index) else {
            continue;
        };
        if index == SYSTEM_DEFAULT {
            default = default_name(&name);
        } else if !name.trim().is_empty() {
            devices.push((index, name));
        }
    }
    let names = numbered(devices.iter().map(|(_, name)| name.clone()));
    let devices = devices
        .into_iter()
        .zip(names)
        .map(|((index, _), name)| Indexed {
            index,
            device: MediaDevice {
                key: DeviceKey::new(name.clone()),
                name,
            },
        })
        .collect();
    (default, devices)
}

fn default_name(listed: &str) -> Option<String> {
    let name = listed.strip_prefix(DEFAULT_PREFIX).unwrap_or(listed).trim();
    (!name.is_empty() && name != UNNAMED_DEFAULT).then(|| name.to_owned())
}

pub(super) fn select_microphone(
    audio: &PlatformAudio,
    index: u16,
    live: bool,
) -> Result<(), String> {
    let device = RecordingDeviceId::from_index(usize::from(index));
    if live {
        audio
            .switch_recording_device(&device)
            .map_err(|e| e.to_string())?;
        if audio.is_recording_initialized() {
            return Ok(());
        }
        return audio.start_recording().map_err(|e| e.to_string());
    }
    if audio.is_recording_initialized() {
        audio.stop_recording().map_err(|e| e.to_string())?;
    }
    audio
        .set_recording_device(&device)
        .map_err(|e| e.to_string())
}

pub(super) fn select_speaker(audio: &PlatformAudio, index: u16) -> Result<(), String> {
    if !audio
        .playout_devices()
        .any(|device| device.index == usize::from(index))
    {
        return Err(format!("no speaker is listed at {index} any more"));
    }
    audio
        .switch_playout_device(&PlayoutDeviceId::from_index(usize::from(index)))
        .map_err(|e| e.to_string())
}
