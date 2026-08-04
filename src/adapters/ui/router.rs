use tokio::sync::{mpsc, watch};

use super::props::send_command;
use super::schema::simple_callbacks;
use crate::commands::ui::{UiCommand, ViewportChanged};
use crate::domain::auth::LoginCredentials;
use crate::domain::room::RoomId;
use crate::domain::sticker::PackId;

type Tx = mpsc::UnboundedSender<UiCommand>;

pub type RoomKey = Option<(RoomId, i32)>;

fn optional_room(id: String) -> Option<RoomId> {
    (!id.is_empty()).then(|| RoomId::new(id))
}

macro_rules! gen_router_fns {
    ($($on:ident $lit:literal $fn:ident $kind:ident $cmd:ident;)*) => {
        $( gen_router_fns!(@one $fn $kind $cmd); )*
    };
    (@one $fn:ident plain $cmd:ident) => {
        pub fn $fn(tx: &Tx) {
            send_command(tx, UiCommand::$cmd);
        }
    };
    (@one $fn:ident pass $cmd:ident) => {
        pub fn $fn(tx: &Tx, arg: String) {
            send_command(tx, UiCommand::$cmd(arg));
        }
    };
    (@one $fn:ident room $cmd:ident) => {
        pub fn $fn(tx: &Tx, arg: String) {
            send_command(tx, UiCommand::$cmd(RoomId::new(arg)));
        }
    };
    (@one $fn:ident opt_room $cmd:ident) => {
        pub fn $fn(tx: &Tx, arg: String) {
            send_command(tx, UiCommand::$cmd(optional_room(arg)));
        }
    };
    (@one $fn:ident manual_string $cmd:ident) => {};
}

simple_callbacks!(gen_router_fns);

pub fn login_password(tx: &Tx, creds: LoginCredentials) {
    send_command(tx, UiCommand::LoginPassword(creds));
}

pub fn move_space(tx: &Tx, from: usize, to: usize, reorder: impl FnOnce(usize, usize)) {
    if from == to {
        return;
    }
    reorder(from, to);
    send_command(tx, UiCommand::MoveSpace { from, to });
}

pub fn send_message(tx: &Tx, room_id: String, body: String, reply_to: String) {
    if room_id.is_empty() || body.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::SendMessage {
            room_id: RoomId::new(room_id),
            body,
            reply_to: (!reply_to.is_empty()).then_some(reply_to),
        },
    );
}

pub fn send_sticker(
    tx: &Tx,
    room_id: String,
    pack_id: String,
    shortcode: String,
    reply_to: String,
) {
    if room_id.is_empty() || pack_id.is_empty() || shortcode.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::SendSticker {
            room_id: RoomId::new(room_id),
            pack: PackId::new(pack_id),
            shortcode,
            reply_to: (!reply_to.is_empty()).then_some(reply_to),
        },
    );
}

pub fn open_media(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::OpenMedia { event_id });
}

pub fn jump_to_event(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::JumpToEvent { event_id });
}

pub fn save_file(tx: &Tx, event_id: String, filename: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::SaveFile { event_id, filename });
}

pub fn scroll_position(scroll_tx: &watch::Sender<ViewportChanged>, key: RoomKey, at_bottom: bool) {
    let Some((room_id, generation)) = key else {
        return;
    };
    let update = ViewportChanged {
        room_id,
        generation,
        at_bottom,
    };
    if scroll_tx.send(update).is_err() {
        tracing::debug!("scroll position receiver closed");
    }
}

pub fn paginate_backwards(tx: &Tx, key: RoomKey) {
    if let Some((room_id, generation)) = key {
        send_command(
            tx,
            UiCommand::PaginateBackwards {
                room_id,
                generation,
            },
        );
    }
}

pub fn paginate_forwards(tx: &Tx, key: RoomKey) {
    if let Some((room_id, generation)) = key {
        send_command(
            tx,
            UiCommand::PaginateForwards {
                room_id,
                generation,
            },
        );
    }
}

pub fn jump_to_latest(tx: &Tx, key: RoomKey) {
    if let Some((room_id, generation)) = key {
        send_command(
            tx,
            UiCommand::JumpToLatest {
                room_id,
                generation,
            },
        );
    }
}
