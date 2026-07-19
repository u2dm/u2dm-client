use slint::SharedString;
use tokio::sync::{mpsc, watch};

use super::common::{Status, StringProp, UiProps, send_command};
use crate::commands::{UiCommand, ViewportChanged};
use crate::domain::models::{LoginCredentials, RoomId};

type Tx = mpsc::UnboundedSender<UiCommand>;

pub type RoomKey = Option<(RoomId, i32)>;

fn begin_login(props: Option<&dyn UiProps>, status: &Status) {
    if let Some(props) = props {
        props.set_string(StringProp::LoginStatus, SharedString::from(status.as_str()));
        props.set_string(StringProp::LoginError, SharedString::default());
    }
}

fn optional_room(id: String) -> Option<RoomId> {
    (!id.is_empty()).then(|| RoomId::new(id))
}

pub fn check_server(props: Option<&dyn UiProps>, tx: &Tx, homeserver: String) {
    begin_login(props, &Status::CheckingServer);
    send_command(tx, UiCommand::CheckServer(homeserver));
}

pub fn login_password(props: Option<&dyn UiProps>, tx: &Tx, creds: LoginCredentials) {
    begin_login(props, &Status::LoggingIn);
    send_command(tx, UiCommand::LoginPassword(creds));
}

pub fn login_oauth(props: Option<&dyn UiProps>, tx: &Tx) {
    begin_login(props, &Status::OpeningBrowser);
    send_command(tx, UiCommand::LoginOAuth);
}

pub fn cancel_oauth(tx: &Tx) {
    send_command(tx, UiCommand::CancelOAuth);
}

pub fn select_room(tx: &Tx, room_id: String) {
    send_command(tx, UiCommand::SelectRoom(RoomId::new(room_id)));
}

pub fn select_space(tx: &Tx, space_id: String) {
    send_command(tx, UiCommand::SelectSpace(optional_room(space_id)));
}

pub fn select_subspace(tx: &Tx, space_id: String) {
    send_command(tx, UiCommand::SelectSubspace(optional_room(space_id)));
}

pub fn move_space(tx: &Tx, from: usize, to: usize, reorder: impl FnOnce(usize, usize)) {
    if from == to {
        return;
    }
    reorder(from, to);
    send_command(tx, UiCommand::MoveSpace { from, to });
}

pub fn logout(tx: &Tx) {
    send_command(tx, UiCommand::Logout);
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

pub fn accept_verification(tx: &Tx) {
    send_command(tx, UiCommand::AcceptVerification);
}

pub fn confirm_verification(tx: &Tx) {
    send_command(tx, UiCommand::ConfirmVerification);
}

pub fn reject_verification(tx: &Tx) {
    send_command(tx, UiCommand::RejectVerification);
}

pub fn open_media(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::OpenMedia { event_id });
}

pub fn save_file(tx: &Tx, event_id: String, filename: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::SaveFile { event_id, filename });
}

pub fn scroll_position(
    scroll_tx: &watch::Sender<ViewportChanged>,
    key: RoomKey,
    at_top: bool,
    at_bottom: bool,
) {
    let Some((room_id, generation)) = key else {
        return;
    };
    let update = ViewportChanged {
        room_id,
        generation,
        at_top,
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

pub fn retry_timeline(tx: &Tx) {
    send_command(tx, UiCommand::RetryTimeline);
}
