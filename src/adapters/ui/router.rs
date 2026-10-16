use tokio::sync::watch;

use super::clipboard;
use super::emote;
use super::permalink;
use super::props::send_command;
use super::schema::{notify_modes, room_accesses, room_kinds, simple_callbacks, variant_named};
use crate::app::input::CommandSender;
use crate::commands::ui::{
    ComposerActivity, MessageDraft, ReplyDraft, TimelineVisibility, UiCommand, UserActivity,
    ViewportChanged, WindowAttention, WindowFocus,
};
use crate::domain::auth::LoginCredentials;
use crate::domain::emote::ComposerText;
use crate::domain::encryption::RecoveryKey;
use crate::domain::image_pack::PackId;
use crate::domain::link::LauncherSafeUrl;
use crate::domain::matrix_link::MatrixLink;
use crate::domain::media::AttachmentPick;
use crate::domain::message::{EditKind, EditTarget, MessageEdit, TextRevision};
use crate::domain::poll::{ChoiceMode, PollDisclosure, PollDraft};
use crate::domain::room::{NotifyMode, RoomId};
use crate::domain::room_creation::{RoomAccess, RoomDraft, RoomKind};
use crate::domain::user_info::UserId;

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
    (@one $fn:ident user $cmd:ident) => {
        pub fn $fn(tx: &Tx, arg: String) {
            if arg.is_empty() {
                return;
            }
            send_command(tx, UiCommand::$cmd(UserId::new(arg)));
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

pub fn use_recovery_key(tx: &Tx, key: String) {
    if key.trim().is_empty() {
        return;
    }
    send_command(tx, UiCommand::UseRecoveryKey(RecoveryKey::new(key)));
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
            draft: MessageDraft {
                emotes: emote::bound_in(&body),
                body,
                reply,
            },
        },
    );
}

pub struct EditedRow {
    pub event_id: String,
    pub local_id: String,
}

pub struct EditedText {
    pub caption: bool,
    pub restored: bool,
    pub original: String,
    pub body: String,
}

pub fn edit_message(tx: &Tx, room_id: String, row: EditedRow, text: EditedText) -> bool {
    if room_id.is_empty() {
        return false;
    }
    let Some(target) = EditTarget::of(row.event_id, row.local_id) else {
        return false;
    };
    let kind = if text.caption {
        EditKind::Caption
    } else {
        EditKind::Message
    };
    let original = (!text.restored).then_some(text.original);
    match TextRevision::of(kind, original.as_deref(), text.body) {
        TextRevision::Blank => false,
        TextRevision::Unchanged => true,
        TextRevision::Changed(body) => {
            send_command(
                tx,
                UiCommand::EditMessage {
                    room_id: RoomId::new(room_id),
                    edit: MessageEdit {
                        target,
                        kind,
                        emotes: emote::bound_in(&body),
                        body,
                        original,
                    },
                },
            );
            true
        }
    }
}

pub fn suggest_mentions(tx: &Tx, key: RoomKey, query: String, recent: Vec<String>) {
    let Some((room_id, _)) = key else {
        return;
    };
    send_command(
        tx,
        UiCommand::SuggestMentions {
            room_id,
            query,
            recent: recent.into_iter().map(UserId::new).collect(),
        },
    );
}

pub fn dismiss_unsent(tx: &Tx, submission: i32) {
    send_command(tx, UiCommand::DismissUnsent { submission });
}

notify_modes!(variant_named notify_mode_named NotifyMode;);

pub fn set_room_notify(tx: &Tx, room_id: String, mode: &str) {
    let Some(room_id) = optional_room(room_id) else {
        return;
    };
    let Some(mode) = notify_mode_named(mode) else {
        tracing::warn!(mode, "ignoring an unknown notification mode");
        return;
    };
    send_command(tx, UiCommand::SetRoomNotify { room_id, mode });
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
            caption: ComposerText {
                emotes: emote::bound_in(&caption),
                text: caption,
            },
            as_document,
            reply_to: (!reply_to.is_empty()).then_some(reply_to),
        },
    );
}

fn poll_draft(
    question: String,
    answers: Vec<String>,
    multiple: bool,
    hide_results: bool,
) -> Option<PollDraft> {
    PollDraft::parse(
        question,
        answers,
        ChoiceMode::from_multiple(multiple),
        PollDisclosure::from_hidden(hide_results),
    )
    .inspect_err(|reason| tracing::debug!(?reason, "refusing a poll draft"))
    .ok()
}

room_kinds!(variant_named room_kind_named RoomKind;);
room_accesses!(variant_named room_access_named RoomAccess;);

#[allow(clippy::too_many_arguments)]
pub fn create_room(
    tx: &Tx,
    name: String,
    topic: String,
    address: String,
    kind: &str,
    access: &str,
    encrypted: bool,
    in_space: bool,
) {
    let (Some(kind), Some(access)) = (room_kind_named(kind), room_access_named(access)) else {
        tracing::warn!(kind, access, "the create dialog sent an unknown choice");
        return;
    };
    send_command(
        tx,
        UiCommand::CreateRoom(RoomDraft {
            kind,
            name,
            topic,
            access,
            address,
            encrypted,
            in_space,
        }),
    );
}

pub fn send_poll(
    tx: &Tx,
    room_id: String,
    question: String,
    answers: Vec<String>,
    multiple: bool,
    hide_results: bool,
) -> bool {
    if room_id.is_empty() {
        return false;
    }
    let Some(draft) = poll_draft(question, answers, multiple, hide_results) else {
        return false;
    };
    send_command(
        tx,
        UiCommand::SendPoll {
            room_id: RoomId::new(room_id),
            draft,
        },
    );
    true
}

pub fn edit_poll(
    tx: &Tx,
    event_id: String,
    question: String,
    answers: Vec<String>,
    multiple: bool,
    hide_results: bool,
) -> bool {
    if event_id.is_empty() {
        return false;
    }
    let Some(draft) = poll_draft(question, answers, multiple, hide_results) else {
        return false;
    };
    send_command(tx, UiCommand::EditPoll { event_id, draft });
    true
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

pub fn open_link(tx: &Tx, url: String) {
    let Some(command) = link_command(url) else {
        tracing::debug!("ignoring a message link U2DM does not open");
        return;
    };
    send_command(tx, command);
}

pub fn link_command(url: impl AsRef<str>) -> Option<UiCommand> {
    let url = url.as_ref();
    match permalink::matrix_link(url) {
        Some(MatrixLink::User(user_id)) => Some(UiCommand::OpenUserInfo(user_id)),
        Some(MatrixLink::Room(link)) => Some(UiCommand::OpenRoomLink(link)),
        None => LauncherSafeUrl::message_link(url).map(|url| UiCommand::OpenLink { url }),
    }
}

pub fn open_external_link(tx: &Tx, url: impl AsRef<str>) {
    let Some(link) = permalink::matrix_link(url.as_ref()) else {
        tracing::info!("ignoring a link from outside U2DM that names nothing in Matrix");
        return;
    };
    send_command(tx, UiCommand::OpenExternalLink(link));
}

pub fn find_room(tx: &Tx, address: String) {
    if address.trim().is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::FindRoom(permalink::room_address_entry(address)),
    );
}

pub fn jump_to_event(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::JumpToEvent { event_id });
}

pub fn open_pinned(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::OpenPinned { event_id });
}

pub fn previous_pinned(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::PreviousPinned { event_id });
}

pub fn next_pinned(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::NextPinned { event_id });
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

pub fn react_with_emote(tx: &Tx, event_id: String, pack: String, shortcode: String) {
    if event_id.is_empty() || pack.is_empty() || shortcode.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::ReactWithEmote {
            event_id,
            pack: PackId::new(pack),
            shortcode,
        },
    );
}

pub fn set_pack_global(tx: &Tx, pack: String, global: bool) {
    if pack.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::SetPackGlobal {
            pack: PackId::new(pack),
            global,
        },
    );
}

pub fn vote_poll(tx: &Tx, event_id: String, answer_id: String) {
    if event_id.is_empty() || answer_id.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::VotePoll {
            event_id,
            answer_id,
        },
    );
}

pub fn copy_message_link(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::CopyMessageLink { event_id });
}

pub fn open_event_source(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::OpenEventSource { event_id });
}

pub fn open_readers(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::OpenReaders { event_id });
}

pub fn pin_message(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::PinMessage { event_id });
}

pub fn unpin_message(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::UnpinMessage { event_id });
}

pub fn delete_message(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::DeleteMessage { event_id });
}

pub fn end_poll(tx: &Tx, event_id: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(tx, UiCommand::EndPoll { event_id });
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

pub fn window_focus(attention_tx: &watch::Sender<WindowAttention>, attended: bool) {
    let focus = if attended {
        WindowFocus::Focused
    } else {
        WindowFocus::Unfocused
    };
    attention_tx.send_if_modified(|attention| {
        let changed = attention.focus != focus;
        attention.focus = focus;
        changed
    });
}

pub fn user_activity(attention_tx: &watch::Sender<WindowAttention>, active: bool) {
    let activity = if active {
        UserActivity::Active
    } else {
        UserActivity::Idle
    };
    attention_tx.send_if_modified(|attention| {
        let changed = attention.activity != activity;
        attention.activity = activity;
        changed
    });
}

pub fn composer_changed(
    composer_tx: &watch::Sender<ComposerActivity>,
    room_id: String,
    composing: bool,
    edits: i32,
) {
    let activity = ComposerActivity {
        room_id: optional_room(room_id),
        composing,
        edits,
    };
    if composer_tx.send(activity).is_err() {
        tracing::debug!("composer activity receiver closed");
    }
}
