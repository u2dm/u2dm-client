use serde::Serialize;

use super::{CallDevicesDto, call_devices};
use crate::adapters::demo::probe::names;
use crate::commands::view::{
    CallSheetView, JoinOffer, JoinRefusal, PreviewTrouble, SheetPreview, TestPhase,
};
use crate::domain::member_name::NamedUser;

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct CallSheetDto {
    serial: i32,
    room_id: String,
    room_name: String,
    direct: bool,
    offer: &'static str,
    refusal: Option<&'static str>,
    video: bool,
    people: usize,
    others: Vec<String>,
    devices: Option<CallDevicesDto>,
    microphone: bool,
    camera: bool,
    preview: Option<SheetPreviewDto>,
    tests: bool,
    test: Option<DeviceTestDto>,
    leaving: Option<String>,
    leaving_name: Option<String>,
    ended: bool,
}

#[derive(Serialize)]
struct DeviceTestDto {
    kind: &'static str,
    phase: &'static str,
    level: u8,
}

#[derive(Serialize)]
struct SheetPreviewDto {
    state: &'static str,
    feed: Option<String>,
    call: Option<String>,
    trouble: Option<&'static str>,
}

#[derive(Serialize)]
pub(super) struct CallOfferDto {
    pub(super) room_id: String,
    pub(super) offer: &'static str,
    pub(super) refusal: Option<&'static str>,
}

pub(super) fn call_sheet(sheet: &CallSheetView) -> CallSheetDto {
    CallSheetDto {
        serial: sheet.serial,
        room_id: sheet.room_id.to_string(),
        room_name: sheet.room_name.clone(),
        direct: sheet.direct,
        offer: offer_word(sheet.offer),
        refusal: offer_refusal(sheet.offer),
        video: sheet.video,
        people: sheet.people,
        others: sheet.others.iter().map(NamedUser::shown).collect(),
        devices: sheet.devices.as_ref().map(call_devices),
        microphone: sheet.proposed.microphone,
        camera: sheet.proposed.camera,
        preview: sheet.preview.as_ref().map(sheet_preview),
        tests: sheet.tests.offered(),
        test: sheet.tests.running().map(|test| DeviceTestDto {
            kind: names::call_device_kind(test.kind),
            phase: match test.phase {
                TestPhase::Starting => "starting",
                TestPhase::Running => "running",
                TestPhase::Failed => "failed",
                TestPhase::Lost => "lost",
            },
            level: test.level,
        }),
        leaving: sheet
            .leaving
            .as_ref()
            .map(|leaving| leaving.room_id.to_string()),
        leaving_name: sheet
            .leaving
            .as_ref()
            .map(|leaving| leaving.room_name.clone()),
        ended: sheet.ended,
    }
}

fn sheet_preview(preview: &SheetPreview) -> SheetPreviewDto {
    let (state, call, trouble) = match preview {
        SheetPreview::Starting => ("starting", None, None),
        SheetPreview::Local(_) => ("local", None, None),
        SheetPreview::InCall { room_name, .. } => ("in-call", Some(room_name.clone()), None),
        SheetPreview::Elsewhere { room_name } => ("elsewhere", Some(room_name.clone()), None),
        SheetPreview::Failed(trouble) => ("failed", None, Some(preview_trouble_word(*trouble))),
    };
    SheetPreviewDto {
        state,
        feed: preview.feed().map(ToString::to_string),
        call,
        trouble,
    }
}

const fn preview_trouble_word(trouble: PreviewTrouble) -> &'static str {
    match trouble {
        PreviewTrouble::NoCamera => "no-camera",
        PreviewTrouble::NotStarted => "not-started",
        PreviewTrouble::Stopped => "stopped",
    }
}

pub(super) const fn offer_refusal(offer: JoinOffer) -> Option<&'static str> {
    match offer {
        JoinOffer::Refused(refusal) => Some(refusal_word(refusal)),
        JoinOffer::Start | JoinOffer::Join | JoinOffer::InCall => None,
    }
}

pub(super) const fn offer_word(offer: JoinOffer) -> &'static str {
    match offer {
        JoinOffer::Start => "start",
        JoinOffer::Join => "join",
        JoinOffer::InCall => "in-call",
        JoinOffer::Refused(_) => "refused",
    }
}

pub(super) const fn refusal_word(refusal: JoinRefusal) -> &'static str {
    match refusal {
        JoinRefusal::NotBuilt => "not-built",
        JoinRefusal::MediaBroken => "media-broken",
        JoinRefusal::NoServer => "no-server",
        JoinRefusal::ServerRefused => "server-refused",
        JoinRefusal::NotAllowed => "not-allowed",
        JoinRefusal::SessionUnverified => "session-unverified",
    }
}
