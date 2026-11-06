use slint::VecModel;

use crate::adapters::ui::backend::{Models, UiBackend};
use crate::adapters::ui::dto::call_device_to_dto;
use crate::adapters::ui::props::{IntProp, StringProp, UiProps};
use crate::adapters::ui::rows::row_index;
use crate::commands::view::{CallDevices, CallView, DeviceNews};
use crate::domain::call::devices::DeviceKind;
use crate::util::saturating_i32;

pub(super) fn shown_devices(call: &CallView) -> Option<&CallDevices> {
    if let Some(devices) = call.ring.as_ref().and_then(|ring| ring.devices.as_ref()) {
        return Some(devices);
    }
    match &call.sheet {
        Some(sheet) => sheet.devices.as_ref(),
        None => call.active.as_ref().map(|active| &active.devices),
    }
}

pub(super) fn apply_device_rows<B: UiBackend>(
    w: &B::Window,
    last: Option<&CallDevices>,
    next: Option<&CallDevices>,
) {
    let models = B::models();
    for kind in DeviceKind::ALL {
        let options = next.map(|devices| devices.options.of(kind));
        if last.map(|last| last.options.of(kind)) == options {
            continue;
        }
        let rows: Vec<B::CallDevice> = options.map_or_else(Vec::new, |options| {
            options
                .iter()
                .map(call_device_to_dto)
                .map(B::CallDevice::from)
                .collect()
        });
        device_model::<B>(&models, kind).set_vec(rows);
    }
    w.set_int(
        IntProp::MicrophoneChosen,
        chosen_index(next, DeviceKind::Microphone),
    );
    w.set_int(
        IntProp::CameraChosen,
        chosen_index(next, DeviceKind::Camera),
    );
    w.set_int(
        IntProp::SpeakerChosen,
        chosen_index(next, DeviceKind::Speaker),
    );
}

fn chosen_index(devices: Option<&CallDevices>, kind: DeviceKind) -> i32 {
    row_index(devices.and_then(|devices| {
        devices
            .options
            .of(kind)
            .iter()
            .position(|option| option.chosen)
    }))
}

pub(super) fn apply_device_news(w: &impl UiProps, news: Option<&DeviceNews>) {
    let Some(news) = news else {
        w.set_int(IntProp::DeviceNewsSerial, 0);
        return;
    };
    w.set_device_news_kind(news.kind);
    w.set_device_news_trouble(news.trouble);
    w.set_text(StringProp::DeviceNewsName, &news.name);
    w.set_int(IntProp::DeviceNewsSerial, saturating_i32(news.serial));
}

fn device_model<B: UiBackend>(models: &Models<B>, kind: DeviceKind) -> &VecModel<B::CallDevice> {
    match kind {
        DeviceKind::Microphone => &models.call_microphones,
        DeviceKind::Speaker => &models.call_speakers,
        DeviceKind::Camera => &models.call_cameras,
    }
}
