use tokio::sync::watch;
use url::Url;

use super::clipboard;
use super::props::send_command;
use super::schema::simple_callbacks;
use crate::app::input::CommandSender;
use crate::commands::ui::{
    MessageDraft, ReplyDraft, TimelineVisibility, UiCommand, ViewportChanged,
};
use crate::domain::auth::LoginCredentials;
use crate::domain::media::AttachmentPick;
use crate::domain::room::RoomId;
use crate::domain::sticker::PackId;

type Tx = CommandSender;

pub type RoomKey = Option<(RoomId, i32)>;

fn optional_room(id: String) -> Option<RoomId> {
    (!id.is_empty()).then(|| RoomId::new(id))
}

macro_rules! gen_router_fns {
    ($($on:ident $lit:literal $fn:ident $kind:ident $(($($arg:tt)*))? $cmd:ident;)*) => {
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
    (@one $fn:ident room_key $cmd:ident) => {
        pub fn $fn(tx: &Tx, key: RoomKey) {
            if let Some((room_id, generation)) = key {
                send_command(tx, UiCommand::$cmd { room_id, generation });
            }
        }
    };
    (@one $fn:ident manual_string $cmd:ident) => {};
    (@one $fn:ident request $cmd:ident) => {};
}

simple_callbacks!(gen_router_fns);

pub fn login_password(tx: &Tx, username: String, password: String) {
    send_command(
        tx,
        UiCommand::LoginPassword(LoginCredentials { username, password }),
    );
}

pub fn move_space(tx: &Tx, from: usize, to: usize, reorder: impl FnOnce(usize, usize)) {
    if from == to {
        return;
    }
    reorder(from, to);
    send_command(tx, UiCommand::MoveSpace { from, to });
}

pub fn send_message(
    tx: &Tx,
    room_id: String,
    body: String,
    reply_to: String,
    reply_sender: String,
    reply_preview: String,
) {
    if room_id.is_empty() || body.is_empty() {
        return;
    }
    let reply = (!reply_to.is_empty()).then_some(ReplyDraft {
        event_id: reply_to,
        sender: reply_sender,
        preview: reply_preview,
    });
    send_command(
        tx,
        UiCommand::SendMessage {
            room_id: RoomId::new(room_id),
            draft: MessageDraft { body, reply },
        },
    );
}

pub fn dismiss_unsent(tx: &Tx, submission: i32) {
    send_command(tx, UiCommand::DismissUnsent { submission });
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

pub fn pick_photo(tx: &Tx, room_id: String) {
    pick_attachment(tx, room_id, AttachmentPick::Media);
}

pub fn pick_document(tx: &Tx, room_id: String) {
    pick_attachment(tx, room_id, AttachmentPick::Document);
}

pub fn paste_attachment(tx: &Tx, room_id: String) -> bool {
    if room_id.is_empty() {
        return false;
    }
    let Some(media) = clipboard::pasted_media() else {
        return false;
    };
    pick_attachment(tx, room_id, AttachmentPick::Pasted(media));
    true
}

fn pick_attachment(tx: &Tx, room_id: String, pick: AttachmentPick) {
    if room_id.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::PickAttachment {
            room_id: RoomId::new(room_id),
            pick,
        },
    );
}

pub fn send_attachment(
    tx: &Tx,
    room_id: String,
    caption: String,
    as_document: bool,
    reply_to: String,
) {
    if room_id.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::SendAttachment {
            room_id: RoomId::new(room_id),
            caption,
            as_document,
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

pub fn open_video(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::OpenVideo { event_id });
}

pub fn play_audio(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::PlayAudio { event_id });
}

const OPENABLE_SCHEMES: &[&str] = &["http", "https", "mailto"];

pub fn open_link(tx: &Tx, url: String) {
    let Ok(parsed) = Url::parse(&url) else {
        tracing::debug!("ignoring a message link that is not a URL");
        return;
    };
    if !OPENABLE_SCHEMES.contains(&parsed.scheme()) {
        tracing::debug!(scheme = parsed.scheme(), "ignoring a message link");
        return;
    }
    send_command(tx, UiCommand::OpenLink { url });
}

pub fn jump_to_event(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::JumpToEvent { event_id });
}

pub fn retry_send(tx: &Tx, local_id: String) {
    if local_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::RetrySend { local_id });
}

pub fn discard_send(tx: &Tx, local_id: String) {
    if local_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::DiscardSend { local_id });
}

pub fn save_file(tx: &Tx, event_id: String, filename: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::SaveFile { event_id, filename });
}

pub fn toggle_reaction(tx: &Tx, event_id: String, key: String) {
    if event_id.is_empty() || key.is_empty() {
        return;
    }
    send_command(tx, UiCommand::ToggleReaction { event_id, key });
}

pub fn scroll_position(
    scroll_tx: &watch::Sender<ViewportChanged>,
    key: RoomKey,
    at_bottom: bool,
    unread_below: u32,
) {
    let Some((room_id, generation)) = key else {
        return;
    };
    let update = ViewportChanged {
        room_id,
        generation,
        at_bottom,
        unread_below,
    };
    if scroll_tx.send(update).is_err() {
        tracing::debug!("scroll position receiver closed");
    }
}

pub fn timeline_visibility(visibility_tx: &watch::Sender<TimelineVisibility>, visible: bool) {
    let visibility = if visible {
        TimelineVisibility::Visible
    } else {
        TimelineVisibility::Hidden
    };
    if visibility_tx.send(visibility).is_err() {
        tracing::debug!("timeline visibility receiver closed");
    }
}
