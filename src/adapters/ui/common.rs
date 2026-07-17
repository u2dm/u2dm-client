use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use slint::{Image, Model, SharedString, VecModel};

thread_local! {
    static IMAGE_CACHE: RefCell<HashMap<PathBuf, Image>> = RefCell::new(HashMap::new());
}

pub fn load_image_cached(path: &Path) -> Option<Image> {
    let cached = IMAGE_CACHE.with_borrow(|cache| cache.get(path).cloned());
    if let Some(img) = cached {
        return Some(img);
    }
    let img = Image::load_from_path(path).ok()?;
    IMAGE_CACHE.with_borrow_mut(|cache| {
        cache.insert(path.to_path_buf(), img.clone());
    });
    Some(img)
}

pub fn sender_initial(name: &str) -> &str {
    match name.chars().next() {
        Some(c) => &name[..c.len_utf8()],
        None => "",
    }
}

const AVATAR_COLORS: u32 = 7;

pub fn avatar_initials(name: &str) -> String {
    let initials: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();

    if initials.is_empty() {
        return sender_initial(name.trim()).to_owned();
    }
    initials
}

pub fn avatar_color_index(id: &str) -> i32 {
    let hash = id.bytes().fold(0_u32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    i32::try_from(hash % AVATAR_COLORS).unwrap_or_default()
}

pub fn user_initial(user_id: &str) -> String {
    let name = user_id.strip_prefix('@').unwrap_or(user_id);
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

pub fn message_sender_label(m: &TimelineMessage) -> &str {
    m.sender_display_name.as_deref().unwrap_or(&m.sender)
}

const PRONOUNS_SHOWN: usize = 3;
const PRONOUNS_MAX_LEN: usize = 16;

pub fn pronoun_labels(pronouns: &[String]) -> Vec<String> {
    pronouns
        .iter()
        .map(|set| set.trim())
        .filter(|set| !set.is_empty())
        .take(PRONOUNS_SHOWN)
        .map(|set| truncate_chars(set, PRONOUNS_MAX_LEN).to_lowercase())
        .collect()
}

fn truncate_chars(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((cut, _)) => text.get(..cut).unwrap_or(text),
        None => text,
    }
}

pub fn message_timestamp_label(timestamp: u64) -> String {
    chrono::DateTime::from_timestamp((timestamp / 1000).cast_signed(), 0)
        .map(|utc| {
            utc.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

pub fn room_activity_label(last_activity_ts: u64) -> String {
    if last_activity_ts == 0 {
        return String::new();
    }
    let Some(utc) = chrono::DateTime::from_timestamp((last_activity_ts / 1000).cast_signed(), 0)
    else {
        return String::new();
    };
    let local = utc.with_timezone(&chrono::Local);
    let days = chrono::Local::now()
        .date_naive()
        .signed_duration_since(local.date_naive())
        .num_days();
    if days <= 0 {
        local.format("%H:%M").to_string()
    } else if days < 7 {
        local.format("%a").to_string()
    } else {
        local.format("%d/%m/%y").to_string()
    }
}

pub fn message_body_text(body: &MessageBody) -> &str {
    match body {
        MessageBody::Text(s) | MessageBody::Notice(s) | MessageBody::Emote(s) => s,
        MessageBody::Image { caption, .. } => caption.as_deref().unwrap_or_default(),
        MessageBody::File { meta, .. } => &meta.filename,
        MessageBody::Service(_) | MessageBody::UnableToDecrypt => "",
        MessageBody::Unsupported { fallback, .. } => fallback,
    }
}

pub fn message_type_token(body: &MessageBody) -> &'static str {
    match body {
        MessageBody::Text(_) => "text",
        MessageBody::Notice(_) => "notice",
        MessageBody::Emote(_) => "emote",
        MessageBody::Image { .. } => "image",
        MessageBody::File { .. } => "file",
        MessageBody::Service(_) => "service",
        MessageBody::UnableToDecrypt => "utd",
        MessageBody::Unsupported { .. } => "unsupported",
    }
}

pub fn service_kind_token(body: &MessageBody) -> &'static str {
    let MessageBody::Service(event) = body else {
        return "";
    };
    match event {
        ServiceEvent::Joined => "joined",
        ServiceEvent::Left => "left",
        ServiceEvent::Invited { .. } => "invited",
        ServiceEvent::InvitationAccepted => "invitation-accepted",
        ServiceEvent::InvitationRejected => "invitation-rejected",
        ServiceEvent::InvitationRevoked { .. } => "invitation-revoked",
        ServiceEvent::Kicked { .. } => "kicked",
        ServiceEvent::Banned { .. } => "banned",
        ServiceEvent::Unbanned { .. } => "unbanned",
        ServiceEvent::Knocked => "knocked",
        ServiceEvent::KnockAccepted { .. } => "knock-accepted",
        ServiceEvent::DisplayNameSet { .. } => "name-set",
        ServiceEvent::DisplayNameChanged { .. } => "name-changed",
        ServiceEvent::DisplayNameRemoved => "name-removed",
        ServiceEvent::AvatarChanged => "avatar-changed",
        ServiceEvent::RoomNameChanged { .. } => "room-name",
        ServiceEvent::RoomTopicChanged => "room-topic",
        ServiceEvent::RoomAvatarChanged => "room-avatar",
        ServiceEvent::RoomCreated => "room-created",
        ServiceEvent::EncryptionEnabled => "encryption",
        ServiceEvent::CallStarted => "call-started",
        ServiceEvent::CallNotification => "call-notification",
    }
}

pub fn service_target(body: &MessageBody) -> &str {
    let MessageBody::Service(event) = body else {
        return "";
    };
    match event {
        ServiceEvent::Invited { target }
        | ServiceEvent::InvitationRevoked { target }
        | ServiceEvent::Kicked { target }
        | ServiceEvent::Banned { target }
        | ServiceEvent::Unbanned { target }
        | ServiceEvent::KnockAccepted { target } => target.as_deref().unwrap_or_default(),
        ServiceEvent::DisplayNameSet { name }
        | ServiceEvent::DisplayNameChanged { name }
        | ServiceEvent::RoomNameChanged { name } => name,
        _ => "",
    }
}

pub fn unsupported_kind(body: &MessageBody) -> &str {
    match body {
        MessageBody::Unsupported { kind, .. } => kind,
        _ => "",
    }
}

pub fn message_preview_kind_token(kind: MessagePreviewKind) -> &'static str {
    match kind {
        MessagePreviewKind::None => "",
        MessagePreviewKind::Text => "text",
        MessagePreviewKind::Image => "image",
        MessagePreviewKind::Video => "video",
        MessagePreviewKind::Audio => "audio",
        MessagePreviewKind::File => "file",
        MessagePreviewKind::Location => "location",
        MessagePreviewKind::Encrypted => "encrypted",
        MessagePreviewKind::Sticker => "sticker",
    }
}

pub fn login_method_token(method: LoginMethod) -> &'static str {
    match method {
        LoginMethod::Password => "password",
        LoginMethod::OAuth => "oauth",
        LoginMethod::Both => "both",
        LoginMethod::None => "",
    }
}

pub fn connection_status_token(status: &ConnectionStatus) -> &'static str {
    match status {
        ConnectionStatus::Disconnected => "disconnected",
        ConnectionStatus::Connecting => "connecting",
        ConnectionStatus::Connected => "connected",
        ConnectionStatus::Error(_) => "error",
    }
}

use crate::commands::UiEvent;
use crate::domain::models::{
    ConnectionStatus, LoginMethod, MessageBody, MessagePreviewKind, Room, ServerInfo, ServiceEvent,
    Space, TimelineMessage, TimelinePatch, VerificationEmoji as DomainVerificationEmoji,
    VerificationEvent as DomainVerificationEvent,
};

pub enum StringProp {
    LoginStep,
    LoginStatus,
    LoginError,
    LoginMethod,
    ResolvedHomeserver,
    UserId,
    UserInitial,
    ToastMessage,
    ConnectionStatus,
    VerificationStep,
    VerificationSender,
    VerificationError,
    SavedFilePath,
    SelectedRoomName,
    SelectedRoomId,
    SelectedSpaceId,
    SelectedSubspaceId,
    InputUsername,
    InputPassword,
}

impl StringProp {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LoginStep => "login-step",
            Self::LoginStatus => "login-status",
            Self::LoginError => "login-error",
            Self::LoginMethod => "login-method",
            Self::ResolvedHomeserver => "resolved-homeserver",
            Self::UserId => "user-id",
            Self::UserInitial => "user-initial",
            Self::ToastMessage => "toast-message",
            Self::ConnectionStatus => "connection-status",
            Self::VerificationStep => "verification-step",
            Self::VerificationSender => "verification-sender",
            Self::VerificationError => "verification-error",
            Self::SavedFilePath => "saved-file-path",
            Self::SelectedRoomName => "selected-room-name",
            Self::SelectedRoomId => "selected-room-id",
            Self::SelectedSpaceId => "selected-space-id",
            Self::SelectedSubspaceId => "selected-subspace-id",
            Self::InputUsername => "input-username",
            Self::InputPassword => "input-password",
        }
    }
}

pub enum BoolProp {
    VerificationVisible,
    VerificationIsSelf,
    TimelineLoading,
    BackwardsLoading,
    ForwardsLoading,
}

impl BoolProp {
    #[cfg(feature = "interpreted")]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::VerificationVisible => "verification-visible",
            Self::VerificationIsSelf => "verification-is-self",
            Self::TimelineLoading => "timeline-loading",
            Self::BackwardsLoading => "backwards-loading",
            Self::ForwardsLoading => "forwards-loading",
        }
    }
}

pub enum IntProp {
    NewMessagesCount,
}

impl IntProp {
    #[cfg(feature = "interpreted")]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NewMessagesCount => "new-messages-count",
        }
    }
}

pub trait UiProps {
    fn set_string(&self, prop: StringProp, value: SharedString);
    fn set_bool(&self, prop: BoolProp, value: bool);
    fn set_int(&self, prop: IntProp, value: i32);
    fn get_string(&self, prop: StringProp) -> SharedString;
    fn apply_user_avatar(&self, avatar: Option<Image>);
    fn apply_emoji_model(&self, emojis: &[DomainVerificationEmoji]);
    fn clear_emoji_model(&self);
}

pub enum Status {
    CheckingServer,
    LoggingIn,
    OpeningBrowser,
    FileSaved,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CheckingServer => "checking-server",
            Self::LoggingIn => "logging-in",
            Self::OpeningBrowser => "opening-browser",
            Self::FileSaved => "file-saved",
        }
    }
}

pub enum LoginStep {
    Homeserver,
    Credentials,
    LoggedIn,
}

impl LoginStep {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Homeserver => "homeserver",
            Self::Credentials => "credentials",
            Self::LoggedIn => "logged-in",
        }
    }
}

pub enum VerifyStep {
    Requested,
    Emojis,
    Confirming,
    Done,
    Cancelled,
}

impl VerifyStep {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Emojis => "emojis",
            Self::Confirming => "confirming",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub fn dispatch_ui_event<T, R, S>(
    w: &impl UiProps,
    event: UiEvent,
    timeline_model: &VecModel<T>,
    rooms_model: &VecModel<R>,
    spaces_model: &VecModel<S>,
    subspaces_model: &VecModel<S>,
    convert_message: &dyn Fn(&TimelineMessage) -> T,
    convert_room: &dyn Fn(&Room) -> R,
    convert_space: &dyn Fn(&Space) -> S,
    room_entry_id: &dyn Fn(&R) -> &str,
    space_entry_id: &dyn Fn(&S) -> &str,
    message_entry_event_id: &dyn Fn(&T) -> String,
) where
    T: Clone + 'static,
    R: Clone + PartialEq + 'static,
    S: Clone + PartialEq + 'static,
{
    match event {
        UiEvent::ServerInfo(info) => apply_server_info(w, &info),
        UiEvent::ShowLogin => apply_show_login(w),
        UiEvent::LoginSuccess { user_id } => apply_login_success(w, &user_id),
        UiEvent::UserAvatar(path) => {
            w.apply_user_avatar(path.as_deref().and_then(load_image_cached));
        }
        UiEvent::LoginError(message) => apply_login_error(w, &message),
        UiEvent::ToastError(message) => apply_toast_error(w, &message),
        UiEvent::Status(msg) => apply_status(w, &msg),
        UiEvent::Rooms(rooms) => {
            apply_rooms(rooms_model, &rooms, convert_room, room_entry_id);
        }
        UiEvent::Spaces(spaces) => {
            apply_reconcile(
                spaces_model,
                &spaces,
                &|s| s.id.as_str(),
                convert_space,
                space_entry_id,
            );
        }
        UiEvent::Subspaces(spaces) => {
            apply_reconcile(
                subspaces_model,
                &spaces,
                &|s| s.id.as_str(),
                convert_space,
                space_entry_id,
            );
        }
        UiEvent::Timeline { room_id, patch } => {
            let selected = w.get_string(StringProp::SelectedRoomId);
            let matches = selected.as_str() == room_id.as_ref();
            tracing::debug!(
                patch = patch.label(),
                %room_id,
                %selected,
                matches,
                "dispatch_ui_event received Timeline event"
            );
            if matches {
                w.set_bool(BoolProp::TimelineLoading, false);
                apply_timeline_patch(
                    timeline_model,
                    *patch,
                    convert_message,
                    message_entry_event_id,
                );
            }
        }
        UiEvent::PaginationState { room_id, state } => {
            let selected = w.get_string(StringProp::SelectedRoomId);
            if selected.as_str() == room_id.as_ref() {
                w.set_bool(BoolProp::BackwardsLoading, state.backwards_loading);
                w.set_bool(BoolProp::ForwardsLoading, state.forwards_loading);
            }
        }
        UiEvent::NewMessagesBadge { room_id, count } => {
            let selected = w.get_string(StringProp::SelectedRoomId);
            if selected.as_str() == room_id.as_ref() {
                w.set_int(
                    IntProp::NewMessagesCount,
                    count.min(i32::MAX as u32).cast_signed(),
                );
            }
        }
        UiEvent::ScrollToBottom { room_id } => {
            let selected = w.get_string(StringProp::SelectedRoomId);
            if selected.as_str() == room_id.as_ref() {
                w.set_int(IntProp::NewMessagesCount, 0);
            }
        }
        UiEvent::ConnectionStatus(status) => apply_connection_status(w, &status),
        UiEvent::Verification(event) => apply_verification(w, &event),
        UiEvent::FileSaved { path } => {
            w.set_string(StringProp::SavedFilePath, SharedString::from(&path));
            w.set_string(
                StringProp::ToastMessage,
                SharedString::from(Status::FileSaved.as_str()),
            );
        }
        UiEvent::LoggedOut => {
            timeline_model.set_vec(Vec::new());
            rooms_model.set_vec(Vec::new());
            spaces_model.set_vec(Vec::new());
            subspaces_model.set_vec(Vec::new());
            apply_logged_out(w);
        }
    }
}

fn apply_server_info(w: &impl UiProps, info: &ServerInfo) {
    let method = LoginMethod::from_auth_methods(&info.auth_methods);
    w.set_string(
        StringProp::LoginMethod,
        SharedString::from(login_method_token(method)),
    );
    w.set_string(
        StringProp::ResolvedHomeserver,
        SharedString::from(&info.homeserver_url),
    );
    w.set_string(
        StringProp::LoginStep,
        SharedString::from(LoginStep::Credentials.as_str()),
    );
    w.set_string(StringProp::LoginStatus, SharedString::default());
}

fn apply_show_login(w: &impl UiProps) {
    w.set_string(
        StringProp::LoginStep,
        SharedString::from(LoginStep::Homeserver.as_str()),
    );
    w.set_string(StringProp::LoginStatus, SharedString::default());
}

fn apply_login_success(w: &impl UiProps, user_id: &str) {
    w.set_string(StringProp::UserId, SharedString::from(user_id));
    w.set_string(
        StringProp::UserInitial,
        SharedString::from(user_initial(user_id)),
    );
    w.set_string(
        StringProp::LoginStep,
        SharedString::from(LoginStep::LoggedIn.as_str()),
    );
    w.set_string(StringProp::LoginStatus, SharedString::default());
}

fn apply_login_error(w: &impl UiProps, msg: &str) {
    w.set_string(StringProp::LoginError, SharedString::from(msg));
    w.set_string(StringProp::LoginStatus, SharedString::default());
}

fn apply_toast_error(w: &impl UiProps, msg: &str) {
    w.set_string(StringProp::ToastMessage, SharedString::from(msg));
}

fn apply_status(w: &impl UiProps, msg: &str) {
    w.set_string(StringProp::LoginStatus, SharedString::from(msg));
}

fn apply_connection_status(w: &impl UiProps, status: &ConnectionStatus) {
    w.set_string(
        StringProp::ConnectionStatus,
        SharedString::from(connection_status_token(status)),
    );
}

fn apply_verification(w: &impl UiProps, event: &DomainVerificationEvent) {
    match event {
        DomainVerificationEvent::Requested { sender, is_self } => {
            w.set_bool(BoolProp::VerificationVisible, true);
            w.set_string(
                StringProp::VerificationStep,
                SharedString::from(VerifyStep::Requested.as_str()),
            );
            w.set_string(
                StringProp::VerificationSender,
                SharedString::from(sender.as_str()),
            );
            w.set_bool(BoolProp::VerificationIsSelf, *is_self);
            w.set_string(StringProp::VerificationError, SharedString::default());
        }
        DomainVerificationEvent::Emojis(emojis) => {
            w.set_string(
                StringProp::VerificationStep,
                SharedString::from(VerifyStep::Emojis.as_str()),
            );
            w.apply_emoji_model(emojis);
        }
        DomainVerificationEvent::Confirming => {
            w.set_string(
                StringProp::VerificationStep,
                SharedString::from(VerifyStep::Confirming.as_str()),
            );
        }
        DomainVerificationEvent::Done => {
            w.set_string(
                StringProp::VerificationStep,
                SharedString::from(VerifyStep::Done.as_str()),
            );
        }
        DomainVerificationEvent::Cancelled(reason) => {
            w.set_string(
                StringProp::VerificationStep,
                SharedString::from(VerifyStep::Cancelled.as_str()),
            );
            w.set_string(
                StringProp::VerificationError,
                SharedString::from(reason.as_str()),
            );
        }
    }
}

fn apply_logged_out(w: &impl UiProps) {
    w.set_string(
        StringProp::LoginStep,
        SharedString::from(LoginStep::Homeserver.as_str()),
    );
    w.set_string(StringProp::UserId, SharedString::default());
    w.set_string(StringProp::UserInitial, SharedString::default());
    w.set_string(StringProp::LoginStatus, SharedString::default());
    w.set_string(StringProp::LoginError, SharedString::default());
    w.set_string(StringProp::LoginMethod, SharedString::default());
    w.set_string(StringProp::ResolvedHomeserver, SharedString::default());
    w.set_string(StringProp::SelectedRoomName, SharedString::default());
    w.set_string(StringProp::SelectedRoomId, SharedString::default());
    w.set_string(StringProp::SelectedSpaceId, SharedString::default());
    w.set_string(StringProp::SelectedSubspaceId, SharedString::default());
    w.set_string(StringProp::InputUsername, SharedString::default());
    w.set_string(StringProp::InputPassword, SharedString::default());
    w.set_string(
        StringProp::ConnectionStatus,
        SharedString::from(connection_status_token(&ConnectionStatus::Disconnected)),
    );
    w.set_bool(BoolProp::VerificationVisible, false);
    w.set_string(StringProp::VerificationStep, SharedString::default());
    w.set_string(StringProp::VerificationSender, SharedString::default());
    w.set_bool(BoolProp::VerificationIsSelf, false);
    w.set_string(StringProp::VerificationError, SharedString::default());
    w.set_string(StringProp::ToastMessage, SharedString::default());
    w.set_string(StringProp::SavedFilePath, SharedString::default());
    w.set_bool(BoolProp::BackwardsLoading, false);
    w.set_bool(BoolProp::ForwardsLoading, false);
    w.set_int(IntProp::NewMessagesCount, 0);
    w.apply_user_avatar(None);
    w.clear_emoji_model();
}

pub fn apply_timeline_patch<T: Clone + 'static>(
    model: &VecModel<T>,
    patch: TimelinePatch,
    convert: &dyn Fn(&TimelineMessage) -> T,
    entry_event_id: &dyn Fn(&T) -> String,
) {
    let before = model.row_count();
    tracing::debug!(
        patch = patch.label(),
        model_rows_before = before,
        "apply_timeline_patch"
    );
    match patch {
        TimelinePatch::Reset(messages) => {
            let entries: Vec<T> = messages.iter().map(convert).collect();
            model.set_vec(entries);
        }
        TimelinePatch::Append(messages) => {
            for m in &messages {
                model.push(convert(m));
            }
        }
        TimelinePatch::PushFront(m) => {
            model.insert(0, convert(&m));
        }
        TimelinePatch::PushBack(m) => {
            model.push(convert(&m));
        }
        TimelinePatch::Insert { index, message } => {
            let idx = index.min(model.row_count());
            model.insert(idx, convert(&message));
        }
        TimelinePatch::Set { index, message } => {
            if index < model.row_count() {
                model.set_row_data(index, convert(&message));
            }
        }
        TimelinePatch::Remove { index } => {
            if index < model.row_count() {
                model.remove(index);
            }
        }
        TimelinePatch::PopFront => {
            if model.row_count() > 0 {
                model.remove(0);
            }
        }
        TimelinePatch::PopBack => {
            let count = model.row_count();
            if count > 0 {
                model.remove(count - 1);
            }
        }
        TimelinePatch::Truncate { length } => {
            while model.row_count() > length {
                model.remove(model.row_count() - 1);
            }
        }
        TimelinePatch::Clear => {
            model.set_vec(Vec::new());
        }
        TimelinePatch::Batch(patches) => {
            apply_batch(model, patches, convert, entry_event_id);
        }
        TimelinePatch::UpdateMedia { event_id, message } => {
            let target = event_id.0;
            for i in 0..model.row_count() {
                if let Some(entry) = model.row_data(i)
                    && entry_event_id(&entry) == target
                {
                    model.set_row_data(i, convert(&message));
                    break;
                }
            }
        }
    }
    tracing::debug!(
        model_rows_after = model.row_count(),
        "apply_timeline_patch done"
    );
}

fn apply_batch<T: Clone + 'static>(
    model: &VecModel<T>,
    patches: Vec<TimelinePatch>,
    convert: &dyn Fn(&TimelineMessage) -> T,
    entry_event_id: &dyn Fn(&T) -> String,
) {
    let all_media = !patches.is_empty()
        && patches
            .iter()
            .all(|p| matches!(p, TimelinePatch::UpdateMedia { .. }));

    if all_media {
        let mut updates: HashMap<&str, &TimelineMessage> = HashMap::new();
        for p in &patches {
            if let TimelinePatch::UpdateMedia { event_id, message } = p {
                updates.insert(&event_id.0, message);
            }
        }
        for i in 0..model.row_count() {
            if let Some(entry) = model.row_data(i) {
                let eid = entry_event_id(&entry);
                if let Some(msg) = updates.get(eid.as_str()) {
                    model.set_row_data(i, convert(msg));
                }
            }
        }
    } else {
        for p in patches {
            apply_timeline_patch(model, p, convert, entry_event_id);
        }
    }
}

pub fn apply_rooms<T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    rooms: &[Room],
    convert: &dyn Fn(&Room) -> T,
    get_id: &dyn Fn(&T) -> &str,
) {
    apply_reconcile(model, rooms, &|r| r.id.as_ref(), convert, get_id);
}

pub fn apply_reconcile<S, T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    items: &[S],
    source_id: &dyn Fn(&S) -> &str,
    convert: &dyn Fn(&S) -> T,
    get_id: &dyn Fn(&T) -> &str,
) {
    let new_ids: HashMap<&str, usize> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (source_id(item), i))
        .collect();

    let mut i = 0;
    while i < model.row_count() {
        let keep = model
            .row_data(i)
            .is_some_and(|entry| new_ids.contains_key(get_id(&entry)));

        if keep {
            i += 1;
        } else {
            model.remove(i);
        }
    }

    for idx in 0..items.len() {
        let Some(item) = items.get(idx) else { continue };
        let new_entry = convert(item);

        if idx < model.row_count() {
            let same_id = model
                .row_data(idx)
                .is_some_and(|entry| get_id(&entry) == source_id(item));

            if same_id {
                if model.row_data(idx).as_ref() != Some(&new_entry) {
                    model.set_row_data(idx, new_entry);
                }
            } else {
                model.insert(idx, new_entry);
            }
        } else {
            model.push(new_entry);
        }
    }

    while model.row_count() > items.len() {
        model.remove(model.row_count() - 1);
    }
}
