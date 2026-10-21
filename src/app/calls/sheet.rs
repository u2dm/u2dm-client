use crate::commands::view::{CallSheetView, JoinOffer, LeavingCall};
use crate::domain::call::RoomCall;
use crate::domain::call::media::MediaChoice;
use crate::domain::room::{Room, RoomId};

pub(super) struct Sheet {
    pub(super) room_id: RoomId,
    serial: i32,
    video: bool,
    proposed: MediaChoice,
    ended: bool,
}

impl Sheet {
    pub(super) fn open(room: &Room, video: bool, serial: i32) -> Self {
        Self {
            room_id: room.id.clone(),
            serial,
            video,
            proposed: proposed_media(room),
            ended: false,
        }
    }

    pub(super) fn call_ended(&mut self) {
        self.ended = true;
    }

    pub(super) fn view(
        &self,
        room: &Room,
        offer: JoinOffer,
        leaving: Option<LeavingCall>,
    ) -> CallSheetView {
        let call = room.call.as_deref();
        CallSheetView {
            serial: self.serial,
            room_id: room.id.clone(),
            room_name: room.display_name.clone(),
            direct: room.is_direct,
            offer,
            video: call.map_or(self.video, RoomCall::is_video),
            people: call.map_or(0, RoomCall::people),
            proposed: self.proposed,
            leaving,
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
