use std::sync::Arc;

use crate::commands::view::{
    CallDevices, CallSheetView, JoinOffer, LeavingCall, SheetPreview, SheetTests,
};
use crate::domain::call::RoomCall;
use crate::domain::call::devices::DeviceChoices;
use crate::domain::call::media::MediaChoice;
use crate::domain::room::{Room, RoomId};

pub(super) struct Sheet {
    pub(super) room_id: RoomId,
    serial: i32,
    video: bool,
    proposed: MediaChoice,
    ended: bool,
    pub(super) camera: bool,
    pub(super) draft: Option<DeviceChoices>,
}

pub(super) struct SheetFacts<'a> {
    pub(super) offer: JoinOffer,
    pub(super) leaving: Option<LeavingCall>,
    pub(super) me: Option<&'a str>,
    pub(super) devices: Option<CallDevices>,
    pub(super) preview: Option<SheetPreview>,
    pub(super) tests: SheetTests,
}

impl Sheet {
    pub(super) fn open(
        room: &Room,
        video: bool,
        serial: i32,
        draft: Option<DeviceChoices>,
    ) -> Self {
        let proposed = proposed_media(room);
        Self {
            room_id: room.id.clone(),
            serial,
            video,
            proposed,
            ended: false,
            camera: proposed.camera,
            draft,
        }
    }

    pub(super) fn call_ended(&mut self) {
        self.ended = true;
    }

    pub(super) fn view(&self, room: &Room, facts: SheetFacts<'_>) -> CallSheetView {
        let call = room.call.as_deref();
        CallSheetView {
            serial: self.serial,
            room_id: room.id.clone(),
            room_name: room.display_name.clone(),
            direct: room.is_direct,
            offer: facts.offer,
            video: call.map_or(self.video, RoomCall::is_video),
            people: call.map_or(0, RoomCall::people),
            others: call.map_or_else(
                || Arc::from(Vec::new()),
                |call| {
                    call.people_in_order()
                        .filter(|user| Some(user.user_id.as_str()) != facts.me)
                        .cloned()
                        .collect()
                },
            ),
            devices: facts.devices,
            proposed: self.proposed,
            preview: facts.preview,
            tests: facts.tests,
            leaving: facts.leaving,
            ended: self.ended && call.is_none(),
        }
    }
}

fn proposed_media(room: &Room) -> MediaChoice {
    MediaChoice {
        microphone: room.is_direct || room.call.is_none(),
        camera: false,
    }
}
