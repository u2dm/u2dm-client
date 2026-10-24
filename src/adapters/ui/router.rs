use tokio::sync::watch;

use super::clipboard;
use super::emote;
use super::permalink;
use super::props::send_command;
use super::schema::{
    call_device_kinds, notify_modes, room_accesses, room_kinds, simple_callbacks, variant_named,
};
use super::sending::SendingContext;
use crate::adapters::draft_size;
use crate::app::input::CommandSender;
use crate::commands::ui::{
    ComposerActivity, MessageDraft, ReplyDraft, TimelineVisibility, UiCommand, UserActivity,
    ViewportChanged, WindowAttention, WindowFocus,
};
use crate::domain::auth::LoginCredentials;
use crate::domain::call::devices::{DeviceKey, DeviceKind};
use crate::domain::call::media::MediaChoice;
use crate::domain::capabilities::AccountPage;
use crate::domain::departure::membership_reason;
use crate::domain::emote::ComposerText;
use crate::domain::encryption::{KeyPassphrase, RecoveryKey};
use crate::domain::event_size::{EventSize, Oversize};
use crate::domain::image_pack::PackId;
use crate::domain::link::LauncherSafeUrl;
use crate::domain::matrix_link::MatrixLink;
use crate::domain::media::AttachmentPick;
use crate::domain::message::{EditKind, EditTarget, MessageEdit, TextRevision};
use crate::domain::poll::{ChoiceMode, PollDisclosure, PollDraft};
use crate::domain::room::{NotifyMode, RoomId};
use crate::domain::room_creation::{RoomAccess, RoomDraft, RoomKind};
use crate::domain::uia::AccountPassword;
use crate::domain::user_info::UserId;

type Tx = CommandSender;

pub type RoomKey = Option<(RoomId, i32)>;

pub enum DraftVerdict {
    Accepted,
    Incomplete,
    TooLong(Oversize),
}

impl DraftVerdict {
    pub fn accepted(&self) -> bool {
        matches!(self, Self::Accepted)
    }

    pub fn shorten_by(&self) -> i32 {
        match self {
            Self::TooLong(oversize) => i32::from(oversize.shorten_by()),
            Self::Accepted | Self::Incomplete => 0,
        }
    }
}

fn sent_if_it_fits(size: EventSize, send: impl FnOnce()) -> DraftVerdict {
    match size.checked() {
        Ok(_) => {
            send();
            DraftVerdict::Accepted
        }
        Err(oversize) => {
            tracing::info!(%oversize, "refusing a draft too large to send");
            DraftVerdict::TooLong(oversize)
        }
    }
}

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
    (@one $fn:ident manual $cmd:ident) => {};
    (@one $fn:ident manual_string $cmd:ident) => {};
    (@one $fn:ident request $cmd:ident) => {};
    (@one $fn:ident verdict $cmd:ident) => {};
}

simple_callbacks!(gen_router_fns);

pub fn login_password(tx: &Tx, username: String, password: String) {
    send_command(
        tx,
        UiCommand::LoginPassword(LoginCredentials { username, password }),
    );
}

pub fn open_call_sheet(tx: &Tx, room_id: String, video: bool) {
    if room_id.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::OpenCallSheet {
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
        UiCommand::StartCall {
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
        UiCommand::JoinCall {
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
        UiCommand::AnswerCall {
            room_id: RoomId::new(room_id),
            camera,
            leaving: optional_room(leaving),
        },
    );
}

pub fn set_microphone(tx: &Tx, on: bool) {
    send_command(tx, UiCommand::SetMicrophone(on));
}

pub fn set_camera(tx: &Tx, on: bool) {
    send_command(tx, UiCommand::SetCamera(on));
}

pub fn sheet_camera(tx: &Tx, on: bool) {
    send_command(tx, UiCommand::SheetCamera(on));
}

pub fn share_screen(tx: &Tx, on: bool) {
    send_command(tx, UiCommand::ShareScreen(on));
}

pub fn show_self_view(tx: &Tx, on: bool) {
    send_command(tx, UiCommand::ShowSelfView(on));
}

pub fn show_share_preview(tx: &Tx, on: bool) {
    send_command(tx, UiCommand::ShowSharePreview(on));
}

pub fn follow_speaker(tx: &Tx, on: bool) {
    send_command(tx, UiCommand::FollowSpeaker(on));
}

call_device_kinds!(variant_named call_device_kind_named DeviceKind;);

pub fn test_call_device(tx: &Tx, kind: &str, on: bool) {
    let Some(kind) = call_device_kind_named(kind) else {
        tracing::warn!(kind, "the call sheet asked to test an unknown kind");
        return;
    };
    send_command(tx, UiCommand::TestCallDevice { kind, on });
}

pub fn use_call_device(tx: &Tx, kind: &str, key: String) {
    let Some(kind) = call_device_kind_named(kind) else {
        tracing::warn!(kind, "the device menu sent an unknown kind");
        return;
    };
    let key = (!key.is_empty()).then(|| DeviceKey::new(key));
    send_command(tx, UiCommand::UseCallDevice { kind, key });
}

pub fn use_recovery_key(tx: &Tx, key: String) {
    if key.trim().is_empty() {
        return;
    }
    send_command(tx, UiCommand::UseRecoveryKey(RecoveryKey::new(key)));
}

pub fn export_room_keys(tx: &Tx, passphrase: String) {
    if passphrase.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::ExportRoomKeys(KeyPassphrase::new(passphrase)),
    );
}

pub fn import_room_keys(tx: &Tx, passphrase: String) {
    if passphrase.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::ImportRoomKeys(KeyPassphrase::new(passphrase)),
    );
}

pub fn kick_user(tx: &Tx, user_id: String, reason: String) {
    if user_id.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::KickUser {
            user_id: UserId::new(user_id),
            reason: membership_reason(reason),
        },
    );
}

pub fn ban_user(tx: &Tx, user_id: String, reason: String) {
    if user_id.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::BanUser {
            user_id: UserId::new(user_id),
            reason: membership_reason(reason),
        },
    );
}

pub fn leave_space(tx: &Tx, space_id: String, with_rooms: bool) {
    if space_id.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::LeaveSpace {
            space_id: RoomId::new(space_id),
            with_rooms,
        },
    );
}

pub fn rename_session(tx: &Tx, device_id: String, name: String) {
    if device_id.is_empty() || name.trim().is_empty() {
        return;
    }
    send_command(tx, UiCommand::RenameSession { device_id, name });
}

pub fn open_account_page(tx: &Tx) {
    send_command(tx, UiCommand::OpenAccountPage(AccountPage::Account));
}

pub fn open_sessions_page(tx: &Tx) {
    send_command(tx, UiCommand::OpenAccountPage(AccountPage::Sessions));
}

pub fn change_password(tx: &Tx, password: String, sign_out_others: bool) {
    if password.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::ChangePassword {
            password: AccountPassword::new(password),
            sign_out_others,
        },
    );
}

pub fn deactivate_account(tx: &Tx, erase: bool) {
    send_command(tx, UiCommand::DeactivateAccount { erase });
}

pub fn answer_auth_password(tx: &Tx, password: String) {
    if password.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::AnswerAuthPassword(AccountPassword::new(password)),
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
) -> DraftVerdict {
    if room_id.is_empty() || body.is_empty() {
        return DraftVerdict::Incomplete;
    }
    let emotes = emote::bound_in(&body);
    let context = SendingContext::of(&room_id);
    let size = if reply_to.is_empty() {
        draft_size::message(&body, &emotes, &context.facts())
    } else {
        draft_size::reply(&body, &emotes, &reply_to, &context.facts())
    };
    let reply = (!reply_to.is_empty()).then_some(ReplyDraft {
        event_id: reply_to,
        sender: reply_sender,
        preview: reply_preview,
    });
    sent_if_it_fits(size, || {
        send_command(
            tx,
            UiCommand::SendMessage {
                room_id: RoomId::new(room_id),
                draft: MessageDraft {
                    body,
                    reply,
                    emotes,
                },
            },
        );
    })
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

pub fn edit_message(tx: &Tx, room_id: String, row: EditedRow, text: EditedText) -> DraftVerdict {
    if room_id.is_empty() {
        return DraftVerdict::Incomplete;
    }
    let Some(target) = EditTarget::of(row.event_id, row.local_id) else {
        return DraftVerdict::Incomplete;
    };
    let kind = if text.caption {
        EditKind::Caption
    } else {
        EditKind::Message
    };
    let original = (!text.restored).then_some(text.original);
    match TextRevision::of(kind, original.as_deref(), text.body) {
        TextRevision::Blank => DraftVerdict::Incomplete,
        TextRevision::Unchanged => DraftVerdict::Accepted,
        TextRevision::Changed(body) => {
            let edit = MessageEdit {
                target,
                kind,
                emotes: emote::bound_in(&body),
                body,
                original,
            };
            let size = draft_size::edit(&edit, &SendingContext::of(&room_id).facts());
            sent_if_it_fits(size, || {
                send_command(
                    tx,
                    UiCommand::EditMessage {
                        room_id: RoomId::new(room_id),
                        edit,
                    },
                );
            })
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
) -> DraftVerdict {
    if room_id.is_empty() {
        return DraftVerdict::Incomplete;
    }
    let Some(draft) = poll_draft(question, answers, multiple, hide_results) else {
        return DraftVerdict::Incomplete;
    };
    let size = draft_size::poll_start(&draft, &SendingContext::of(&room_id).facts());
    sent_if_it_fits(size, || {
        send_command(
            tx,
            UiCommand::SendPoll {
                room_id: RoomId::new(room_id),
                draft,
            },
        );
    })
}

pub fn edit_poll(
    tx: &Tx,
    event_id: String,
    question: String,
    answers: Vec<String>,
    multiple: bool,
    hide_results: bool,
) -> DraftVerdict {
    if event_id.is_empty() {
        return DraftVerdict::Incomplete;
    }
    let Some(draft) = poll_draft(question, answers, multiple, hide_results) else {
        return DraftVerdict::Incomplete;
    };
    let size = draft_size::poll_edit(&draft, &event_id, &SendingContext::selected().facts());
    sent_if_it_fits(size, || {
        send_command(tx, UiCommand::EditPoll { event_id, draft });
    })
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
