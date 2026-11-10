mod call;
mod drafts;
mod settings;
mod watches;

pub(super) use self::call::{
    answer_call, join_call, mute_for_me, open_call_sheet, send_call_reaction, start_call,
    test_call_device, use_call_device, watch_connections, watch_microphone,
};
pub(super) use self::drafts::{
    DraftVerdict, EditedRow, EditedText, edit_message, edit_poll, send_attachment, send_message,
    send_poll,
};
pub(super) use self::settings::{
    add_keyword, set_call_layout, set_motion, set_receipts, set_room_default, set_send_key,
    set_setting_flag, set_theme, show_settings_page,
};
pub(super) use self::watches::{
    composer_changed, scroll_position, timeline_visibility, user_activity, window_focus,
};
use super::props::send_command;
use super::schema::{notify_modes, room_accesses, room_kinds, simple_callbacks, variant_named};
use super::{clipboard, permalink};
use crate::app::input::CommandSender;
use crate::commands::ui::{
    AccountCommand, AuthCommand, CallCommand, DepartureCommand, KeysCommand, RuleCommand,
    SettingsCommand, UiCommand,
};
use crate::domain::auth::LoginCredentials;
use crate::domain::capabilities::AccountPage;
use crate::domain::departure::membership_reason;
use crate::domain::encryption::{KeyPassphrase, RecoveryKey};
use crate::domain::image_pack::PackId;
use crate::domain::link::LauncherSafeUrl;
use crate::domain::matrix_link::MatrixLink;
use crate::domain::media::AttachmentPick;
use crate::domain::room::{NotifyMode, RoomId};
use crate::domain::room_creation::{RoomAccess, RoomDraft, RoomKind};
use crate::domain::uia::AccountPassword;
use crate::domain::user_info::UserId;

type Tx = CommandSender;

pub type RoomKey = Option<(RoomId, i32)>;

fn optional_room(id: String) -> Option<RoomId> {
    (!id.is_empty()).then(|| RoomId::new(id))
}

macro_rules! command {
    ([$variant:ident] $($payload:tt)*) => { UiCommand::$variant $($payload)* };
    ([$group:ident :: $variant:ident] $($payload:tt)*) => { $group::$variant $($payload)* };
}

macro_rules! gen_router_fns {
    ($($on:ident $lit:literal $fn:ident $kind:ident $(($($arg:tt)*))?
        $cmd:ident $(:: $variant:ident)?;)*) => {
        $( gen_router_fns!(@one $fn $kind [$cmd $(:: $variant)?]); )*
    };
    (@one $fn:ident plain $cmd:tt) => {
        pub fn $fn(tx: &Tx) {
            send_command(tx, command!($cmd));
        }
    };
    (@one $fn:ident pass $cmd:tt) => {
        pub fn $fn(tx: &Tx, arg: String) {
            send_command(tx, command!($cmd(arg)));
        }
    };
    (@one $fn:ident room $cmd:tt) => {
        pub fn $fn(tx: &Tx, arg: String) {
            send_command(tx, command!($cmd(RoomId::new(arg))));
        }
    };
    (@one $fn:ident opt_room $cmd:tt) => {
        pub fn $fn(tx: &Tx, arg: String) {
            send_command(tx, command!($cmd(optional_room(arg))));
        }
    };
    (@one $fn:ident user $cmd:tt) => {
        pub fn $fn(tx: &Tx, arg: String) {
            if arg.is_empty() {
                return;
            }
            send_command(tx, command!($cmd(UserId::new(arg))));
        }
    };
    (@one $fn:ident event $cmd:tt) => {
        pub fn $fn(tx: &Tx, event_id: String) {
            if event_id.is_empty() {
                return;
            }
            send_command(tx, command!($cmd { event_id }));
        }
    };
    (@one $fn:ident local $cmd:tt) => {
        pub fn $fn(tx: &Tx, local_id: String) {
            if local_id.is_empty() {
                return;
            }
            send_command(tx, command!($cmd { local_id }));
        }
    };
    (@one $fn:ident flag $cmd:tt) => {
        pub fn $fn(tx: &Tx, on: bool) {
            send_command(tx, command!($cmd(on)));
        }
    };
    (@one $fn:ident room_key $cmd:tt) => {
        pub fn $fn(tx: &Tx, key: RoomKey) {
            if let Some((room_id, generation)) = key {
                send_command(tx, command!($cmd { room_id, generation }));
            }
        }
    };
    (@one $fn:ident manual $cmd:tt) => {};
    (@one $fn:ident manual_string $cmd:tt) => {};
    (@one $fn:ident request $cmd:tt) => {};
    (@one $fn:ident verdict $cmd:tt) => {};
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

pub fn export_room_keys(tx: &Tx, passphrase: String) {
    if passphrase.is_empty() {
        return;
    }
    send_command(
        tx,
        KeysCommand::ExportRoomKeys(KeyPassphrase::new(passphrase)),
    );
}

pub fn import_room_keys(tx: &Tx, passphrase: String) {
    if passphrase.is_empty() {
        return;
    }
    send_command(
        tx,
        KeysCommand::ImportRoomKeys(KeyPassphrase::new(passphrase)),
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
        DepartureCommand::LeaveSpace {
            space_id: RoomId::new(space_id),
            with_rooms,
        },
    );
}

pub fn rename_session(tx: &Tx, device_id: String, name: String) {
    if device_id.is_empty() || name.trim().is_empty() {
        return;
    }
    send_command(tx, AccountCommand::RenameSession { device_id, name });
}

pub fn open_account_page(tx: &Tx) {
    send_command(tx, AccountCommand::OpenAccountPage(AccountPage::Account));
}

pub fn open_sessions_page(tx: &Tx) {
    send_command(tx, AccountCommand::OpenAccountPage(AccountPage::Sessions));
}

pub fn change_password(tx: &Tx, password: String, sign_out_others: bool) {
    if password.is_empty() {
        return;
    }
    send_command(
        tx,
        AccountCommand::ChangePassword {
            password: AccountPassword::new(password),
            sign_out_others,
        },
    );
}

pub fn deactivate_account(tx: &Tx, erase: bool) {
    send_command(tx, AccountCommand::DeactivateAccount { erase });
}

pub fn answer_auth_password(tx: &Tx, password: String) {
    if password.is_empty() {
        return;
    }
    send_command(
        tx,
        AuthCommand::AnswerAuthPassword(AccountPassword::new(password)),
    );
}

pub fn move_space(tx: &Tx, from: usize, to: usize, reorder: impl FnOnce(usize, usize)) {
    if from == to {
        return;
    }
    reorder(from, to);
    send_command(tx, UiCommand::MoveSpace { from, to });
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

pub fn save_file(tx: &Tx, event_id: String, filename: String, mimetype: String) {
    if event_id.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::SaveFile {
            event_id,
            filename,
            mimetype: (!mimetype.is_empty()).then_some(mimetype),
        },
    );
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
