use super::{Tx, optional_room};
use crate::adapters::ui::props::send_command;
use crate::adapters::ui::schema::{call_device_kinds, variant_named};
use crate::commands::ui::CallCommand;
use crate::domain::call::devices::{DeviceKey, DeviceKind};
use crate::domain::call::media::MediaChoice;
use crate::domain::room::RoomId;

pub fn open_call_sheet(tx: &Tx, room_id: String, video: bool) {
    if room_id.is_empty() {
        return;
    }
    send_command(
        tx,
        CallCommand::OpenCallSheet {
            room_id: RoomId::new(room_id),
            video,
        },
    );
}

pub fn start_call(
    tx: &Tx,
    room_id: String,
    video: bool,
    microphone: bool,
    camera: bool,
    leaving: String,
) {
    if room_id.is_empty() {
        return;
    }
    send_command(
        tx,
        CallCommand::StartCall {
            room_id: RoomId::new(room_id),
            video,
            media: MediaChoice { microphone, camera },
            leaving: optional_room(leaving),
        },
    );
}

pub fn join_call(tx: &Tx, room_id: String, microphone: bool, camera: bool, leaving: String) {
    if room_id.is_empty() {
        return;
    }
    send_command(
        tx,
        CallCommand::JoinCall {
            room_id: RoomId::new(room_id),
            media: MediaChoice { microphone, camera },
            leaving: optional_room(leaving),
        },
    );
}

pub fn answer_call(tx: &Tx, room_id: String, camera: bool, leaving: String) {
    if room_id.is_empty() {
        return;
    }
    send_command(
        tx,
        CallCommand::AnswerCall {
            room_id: RoomId::new(room_id),
            camera,
            leaving: optional_room(leaving),
        },
    );
}

pub fn watch_connections(tx: &Tx, on: bool) {
    send_command(tx, CallCommand::WatchConnections(on));
}

pub fn watch_microphone(tx: &Tx, on: bool) {
    send_command(tx, CallCommand::WatchMicrophone(on));
}

pub fn send_call_reaction(tx: &Tx, name: String) {
    if name.is_empty() {
        return;
    }
    send_command(tx, CallCommand::SendCallReaction(name));
}

pub fn mute_for_me(tx: &Tx, user_id: String, on: bool) {
    if user_id.is_empty() {
        return;
    }
    send_command(tx, CallCommand::MuteForMe { user_id, on });
}

call_device_kinds!(variant_named call_device_kind_named DeviceKind;);

pub fn test_call_device(tx: &Tx, kind: &str, on: bool) {
    let Some(kind) = call_device_kind_named(kind) else {
        tracing::warn!(kind, "the call sheet asked to test an unknown kind");
        return;
    };
    send_command(tx, CallCommand::TestCallDevice { kind, on });
}

pub fn use_call_device(tx: &Tx, kind: &str, key: String) {
    let Some(kind) = call_device_kind_named(kind) else {
        tracing::warn!(kind, "the device menu sent an unknown kind");
        return;
    };
    let key = (!key.is_empty()).then(|| DeviceKey::new(key));
    send_command(tx, CallCommand::UseCallDevice { kind, key });
}
