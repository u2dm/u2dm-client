use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use super::calls::{self, PLUGGED_BACK_AFTER, UNPLUGGED_AFTER};
use crate::domain::call::devices::{
    ByKind, DeviceChoice, DeviceChoices, DeviceKey, DeviceKind, DeviceLists, MediaDevice,
};
use crate::domain::call::media::MediaEvent;
use crate::domain::call::video::Camera;

const BUILT_IN: &str = "Built-in Audio Analog Stereo";
const HEADSET: &str = "USB Headset";
const HDMI: &str = "HDMI / DisplayPort";
const MICROPHONES: [&str; 2] = [BUILT_IN, HEADSET];
const SPEAKERS: [&str; 3] = [BUILT_IN, HEADSET, HDMI];
const INTEGRATED_CAMERA: DemoCamera = DemoCamera {
    key: "usb-Demo_Integrated_Camera-video-index0",
    name: "Integrated Camera",
};
const WEBCAM: DemoCamera = DemoCamera {
    key: "usb-Demo_USB_Webcam-video-index0",
    name: "USB Webcam",
};
const CAMERAS: [DemoCamera; 2] = [INTEGRATED_CAMERA, WEBCAM];

#[derive(Clone, Copy)]
struct DemoCamera {
    key: &'static str,
    name: &'static str,
}

type Unplugged = Arc<Mutex<HashSet<&'static str>>>;

pub(super) struct DemoDevices {
    choices: DeviceChoices,
    unplugged: Unplugged,
    unplugging: Vec<JoinHandle<()>>,
    armed: HashSet<&'static str>,
    failed: ByKind<Option<DeviceKey>>,
    routes: ByKind<Option<DeviceKey>>,
    reported: Option<DeviceLists>,
    camera_on: bool,
}

impl DemoDevices {
    pub(super) fn new(choices: DeviceChoices) -> Self {
        Self {
            choices,
            unplugged: Arc::default(),
            unplugging: Vec::new(),
            armed: HashSet::new(),
            failed: ByKind::default(),
            routes: ByKind::default(),
            reported: None,
            camera_on: false,
        }
    }

    fn lists(&self) -> DeviceLists {
        let unplugged = self
            .unplugged
            .lock()
            .map(|unplugged| unplugged.clone())
            .unwrap_or_default();
        lists_without(&unplugged)
    }

    pub(super) fn refresh(&mut self, events_tx: &mpsc::UnboundedSender<MediaEvent>) {
        let lists = self.lists();
        for kind in DeviceKind::ALL {
            let failed = self.failed.of_mut(kind);
            if failed
                .as_ref()
                .is_some_and(|key| !lists.is_connected(kind, key))
            {
                *failed = None;
            }
        }
        if self.reported.as_ref() != Some(&lists) {
            tracing::info!(
                microphones = lists.connected.microphone.len(),
                speakers = lists.connected.speaker.len(),
                cameras = lists.connected.camera.len(),
                "demo: the call devices changed"
            );
            send(events_tx, MediaEvent::Devices(lists.clone()));
            self.reported = Some(lists.clone());
        }
        self.route(DeviceKind::Microphone, &lists);
        self.route(DeviceKind::Speaker, &lists);
        self.follow_camera(&lists, events_tx);
    }

    fn route(&mut self, kind: DeviceKind, lists: &DeviceLists) {
        let used = self
            .choices
            .of(kind)
            .resolve(
                kind,
                lists.connected.of(kind),
                self.failed.of(kind).as_ref(),
            )
            .map(|device| device.key.clone());
        let route = self.routes.of_mut(kind);
        if *route == used {
            return;
        }
        if let Some(device) = &used {
            tracing::info!(%kind, %device, "demo: the call uses another device");
        } else {
            tracing::info!(%kind, "demo: the call uses the system default");
        }
        *route = used;
    }

    fn follow_camera(
        &mut self,
        lists: &DeviceLists,
        events_tx: &mpsc::UnboundedSender<MediaEvent>,
    ) {
        if !self.camera_on {
            return;
        }
        let capturing = self.routes.camera.clone();
        if capturing
            .as_ref()
            .is_some_and(|key| !lists.is_connected(DeviceKind::Camera, key))
        {
            tracing::info!("demo: the camera you use was unplugged");
            self.camera_on = false;
            self.routes.camera = None;
            send(
                events_tx,
                MediaEvent::Camera(Camera::Failed(
                    "demo: the camera stopped sending pictures".into(),
                )),
            );
            return;
        }
        let DeviceChoice::Device(chosen) = &self.choices.camera else {
            return;
        };
        if capturing.as_ref() == Some(&chosen.key)
            || self.failed.camera.as_ref() == Some(&chosen.key)
            || !lists.is_connected(DeviceKind::Camera, &chosen.key)
        {
            return;
        }
        tracing::info!(device = %chosen.key, "demo: the camera switches to the one you chose");
        self.routes.camera = Some(chosen.key.clone());
    }

    pub(super) fn shape_chosen(&mut self, events_tx: &mpsc::UnboundedSender<MediaEvent>) {
        for kind in DeviceKind::ALL {
            if let Some(key) = self.choices.of(kind).key().cloned() {
                self.shape_choice(kind, key, events_tx);
            }
        }
    }

    pub(super) fn choose(
        &mut self,
        kind: DeviceKind,
        choice: DeviceChoice,
        events_tx: &mpsc::UnboundedSender<MediaEvent>,
    ) {
        *self.failed.of_mut(kind) = None;
        *self.choices.of_mut(kind) = choice;
        if let Some(key) = self.choices.of(kind).key().cloned() {
            tracing::info!(%kind, device = %key, "demo: you choose a call device");
            self.shape_choice(kind, key, events_tx);
        } else {
            tracing::info!(%kind, "demo: you choose the system default");
        }
        self.refresh(events_tx);
    }

    fn shape_choice(
        &mut self,
        kind: DeviceKind,
        key: DeviceKey,
        events_tx: &mpsc::UnboundedSender<MediaEvent>,
    ) {
        let shaped = calls::scenario();
        let headset = kind != DeviceKind::Camera && key.as_ref() == HEADSET;
        if headset && shaped.headset_fails {
            self.refuse(kind, key, events_tx);
        } else if headset && shaped.headset_unplugs {
            self.arm_unplug(HEADSET);
        } else if kind == DeviceKind::Camera && key.as_ref() == WEBCAM.key && shaped.webcam_unplugs
        {
            self.arm_unplug(WEBCAM.key);
        }
    }

    fn refuse(
        &mut self,
        kind: DeviceKind,
        key: DeviceKey,
        events_tx: &mpsc::UnboundedSender<MediaEvent>,
    ) {
        tracing::info!(%kind, "demo: the USB Headset cannot be opened");
        send(
            events_tx,
            MediaEvent::DeviceFailed {
                kind,
                device: key.clone(),
                why: "demo: the USB Headset is busy".into(),
            },
        );
        *self.failed.of_mut(kind) = Some(key);
    }

    fn arm_unplug(&mut self, device: &'static str) {
        if !self.armed.insert(device) {
            return;
        }
        let unplugged = Arc::clone(&self.unplugged);
        self.unplugging.push(tokio::spawn(async move {
            sleep(UNPLUGGED_AFTER).await;
            tracing::info!(device, "demo: a call device is unplugged");
            if let Ok(mut unplugged) = unplugged.lock() {
                unplugged.insert(device);
            }
            sleep(PLUGGED_BACK_AFTER).await;
            tracing::info!(device, "demo: a call device is plugged back in");
            if let Ok(mut unplugged) = unplugged.lock() {
                unplugged.remove(device);
            }
        }));
    }

    pub(super) fn start_camera(&mut self) -> Result<String, String> {
        self.failed.camera = None;
        let lists = self.lists();
        let used = self
            .choices
            .camera
            .resolve(
                DeviceKind::Camera,
                &lists.connected.camera,
                self.failed.camera.as_ref(),
            )
            .ok_or_else(|| "demo: no camera was found".to_owned())?;
        self.routes.camera = Some(used.key.clone());
        self.camera_on = true;
        Ok(used.name.clone())
    }

    pub(super) fn stop_camera(&mut self) {
        self.camera_on = false;
        self.routes.camera = None;
    }

    pub(super) fn close(&self) {
        for task in &self.unplugging {
            task.abort();
        }
    }
}

pub(super) fn plugged_in() -> DeviceLists {
    lists_without(&HashSet::new())
}

fn lists_without(unplugged: &HashSet<&'static str>) -> DeviceLists {
    let audio = |names: &[&'static str]| -> Vec<MediaDevice> {
        names
            .iter()
            .filter(|name| !unplugged.contains(*name))
            .map(|name| MediaDevice {
                key: DeviceKey::new(*name),
                name: (*name).to_owned(),
            })
            .collect()
    };
    let cameras = if calls::scenario().no_camera {
        Vec::new()
    } else {
        CAMERAS
            .iter()
            .filter(|camera| !unplugged.contains(camera.key))
            .map(|camera| MediaDevice {
                key: DeviceKey::new(camera.key),
                name: camera.name.to_owned(),
            })
            .collect()
    };
    DeviceLists {
        connected: ByKind {
            microphone: audio(&MICROPHONES),
            speaker: audio(&SPEAKERS),
            camera: cameras,
        },
        defaults: ByKind {
            microphone: Some(BUILT_IN.to_owned()),
            speaker: Some(BUILT_IN.to_owned()),
            camera: None,
        },
    }
}

fn send(events_tx: &mpsc::UnboundedSender<MediaEvent>, event: MediaEvent) {
    if events_tx.send(event).is_err() {
        tracing::debug!("demo: the app stopped listening to the call media");
    }
}
