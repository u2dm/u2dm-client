#![allow(dead_code)]

macro_rules! gen_consts {
    ($($a:ident $c:ident $lit:literal $d:ident;)*) => {
        $( pub const $c: &str = $lit; )*
    };
}

macro_rules! string_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        LoginStatus LOGIN_STATUS "login-status" set_login_status;
        LoginError LOGIN_ERROR "login-error" set_login_error;
        LoginMethod LOGIN_METHOD "login-method" set_login_method;
        ResolvedHomeserver RESOLVED_HOMESERVER "resolved-homeserver" set_resolved_homeserver;
        UserId USER_ID "user-id" set_user_id;
        UserInitial USER_INITIAL "user-initial" set_user_initial;
        ToastMessage TOAST_MESSAGE "toast-message" set_toast_message;
        VerificationSender VERIFICATION_SENDER "verification-sender" set_verification_sender;
        VerificationError VERIFICATION_ERROR "verification-error" set_verification_error;
        SavedFilePath SAVED_FILE_PATH "saved-file-path" set_saved_file_path;
        SelectedRoomName SELECTED_ROOM_NAME "selected-room-name" set_selected_room_name;
        SelectedRoomId SELECTED_ROOM_ID "selected-room-id" set_selected_room_id;
        SelectedSpaceId SELECTED_SPACE_ID "selected-space-id" set_selected_space_id;
        SelectedSubspaceId SELECTED_SUBSPACE_ID "selected-subspace-id" set_selected_subspace_id;
        InputUsername INPUT_USERNAME "input-username" set_input_username;
        InputPassword INPUT_PASSWORD "input-password" set_input_password;
    } };
}
pub(crate) use string_props;

macro_rules! bool_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        VerificationVisible VERIFICATION_VISIBLE "verification-visible" set_verification_visible;
        VerificationIsSelf VERIFICATION_IS_SELF "verification-is-self" set_verification_is_self;
        TimelineRetryable TIMELINE_RETRYABLE "timeline-retryable" set_timeline_retryable;
        BackwardsLoading BACKWARDS_LOADING "backwards-loading" set_backwards_loading;
        ForwardsLoading FORWARDS_LOADING "forwards-loading" set_forwards_loading;
    } };
}
pub(crate) use bool_props;

macro_rules! int_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        NewMessagesCount NEW_MESSAGES_COUNT "new-messages-count" set_new_messages_count;
        PrependToken PREPEND_TOKEN "prepend-token" set_prepend_token;
        SelectedRoomMembers SELECTED_ROOM_MEMBERS "selected-room-members" set_selected_room_members;
        SelectedGeneration SELECTED_GENERATION "selected-generation" set_selected_generation;
    } };
}
pub(crate) use int_props;

macro_rules! message_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        unique_id UNIQUE_ID "unique-id" text;
        sender SENDER "sender" text;
        pronouns PRONOUNS "pronouns" list;
        body BODY "body" text;
        timestamp TIMESTAMP "timestamp" text;
        message_type MESSAGE_TYPE "message-type" text;
        preview_kind PREVIEW_KIND "preview-kind" text;
        unsupported_kind UNSUPPORTED_KIND "unsupported-kind" text;
        event_id EVENT_ID "event-id" text;
        sender_initial SENDER_INITIAL "sender-initial" text;
        color_index COLOR_INDEX "color-index" int;
        is_own IS_OWN "is-own" flag;
        edited EDITED "edited" flag;
        has_reply HAS_REPLY "has-reply" flag;
        reply_sender REPLY_SENDER "reply-sender" text;
        reply_kind REPLY_KIND "reply-kind" text;
        reply_body REPLY_BODY "reply-body" text;
        service_kind SERVICE_KIND "service-kind" text;
        service_target SERVICE_TARGET "service-target" text;
        has_thumbnail HAS_THUMBNAIL "has-thumbnail" flag;
        media_failed MEDIA_FAILED "media-failed" flag;
        image_width IMAGE_WIDTH "image-width" int;
        image_height IMAGE_HEIGHT "image-height" int;
        has_avatar HAS_AVATAR "has-avatar" flag;
        thumbnail THUMBNAIL "thumbnail" image;
        avatar AVATAR "avatar" image;
    } };
}
#[cfg(feature = "interpreted")]
pub(crate) use message_fields;

macro_rules! room_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        id ID "id" text;
        name NAME "name" text;
        initial INITIAL "initial" text;
        color_index COLOR_INDEX "color-index" int;
        members MEMBERS "members" int;
        unread UNREAD "unread" int;
        mentions MENTIONS "mentions" int;
        last_message_sender LAST_MESSAGE_SENDER "last-message-sender" text;
        last_message_kind LAST_MESSAGE_KIND "last-message-kind" text;
        last_message_body LAST_MESSAGE_BODY "last-message-body" text;
        last_message_service_kind LAST_MESSAGE_SERVICE_KIND "last-message-service-kind" text;
        last_message_service_target LAST_MESSAGE_SERVICE_TARGET "last-message-service-target" text;
        last_message_is_own LAST_MESSAGE_IS_OWN "last-message-is-own" flag;
        last_message_edited LAST_MESSAGE_EDITED "last-message-edited" flag;
        last_message_time LAST_MESSAGE_TIME "last-message-time" text;
        has_avatar HAS_AVATAR "has-avatar" flag;
        avatar AVATAR "avatar" image;
    } };
}
#[cfg(feature = "interpreted")]
pub(crate) use room_fields;

macro_rules! space_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        id ID "id" text;
        name NAME "name" text;
        unread UNREAD "unread" int;
        mentions MENTIONS "mentions" int;
        initial INITIAL "initial" text;
        has_avatar HAS_AVATAR "has-avatar" flag;
        avatar AVATAR "avatar" image;
    } };
}
#[cfg(feature = "interpreted")]
pub(crate) use space_fields;

pub mod prop {
    string_props!(gen_consts);
    bool_props!(gen_consts);
    int_props!(gen_consts);

    pub const LOGIN_STEP: &str = "login-step";
    pub const CONNECTION_STATUS: &str = "connection-status";
    pub const VERIFICATION_STEP: &str = "verification-step";
    pub const TIMELINE_STATUS: &str = "timeline-status";

    pub const USER_AVATAR: &str = "user-avatar";
    pub const USER_HAS_AVATAR: &str = "user-has-avatar";
    pub const VERIFICATION_EMOJIS: &str = "verification-emojis";

    pub const TIMELINE: &str = "timeline";
    pub const ROOMS: &str = "rooms";
    pub const SPACES: &str = "spaces";
    pub const SUBSPACES: &str = "subspaces";
}

pub mod callback {
    pub const CHECK_SERVER: &str = "check-server";
    pub const LOGIN_PASSWORD: &str = "login-password";
    pub const LOGIN_OAUTH: &str = "login-oauth";
    pub const CANCEL_OAUTH: &str = "cancel-oauth";
    pub const SELECT_ROOM: &str = "select-room";
    pub const SELECT_SPACE: &str = "select-space";
    pub const SELECT_SUBSPACE: &str = "select-subspace";
    pub const MOVE_SPACE: &str = "move-space";
    pub const LOGOUT: &str = "logout";
    pub const SEND_MESSAGE: &str = "send-message";
    pub const ACCEPT_VERIFICATION: &str = "accept-verification";
    pub const CONFIRM_VERIFICATION: &str = "confirm-verification";
    pub const REJECT_VERIFICATION: &str = "reject-verification";
    pub const OPEN_MEDIA: &str = "open-media";
    pub const SAVE_FILE: &str = "save-file";
    pub const REQUEST_MEDIA: &str = "request-media";
    pub const REQUEST_ROOM_AVATAR: &str = "request-room-avatar";
    pub const SCROLL_POSITION_CHANGED: &str = "scroll-position-changed";
    pub const PAGINATE_BACKWARDS: &str = "paginate-backwards";
    pub const PAGINATE_FORWARDS: &str = "paginate-forwards";
    pub const JUMP_TO_LATEST: &str = "jump-to-latest";
    pub const RETRY_TIMELINE: &str = "retry-timeline";
}

pub mod emoji_store {
    pub const NAME: &str = "EmojiStore";
    pub const GROUPS: &str = "groups";
    pub const RESULTS: &str = "results";
    pub const SEARCH: &str = "search";
    pub const INSERT: &str = "insert";
}

pub mod message {
    message_fields!(gen_consts);
}

pub mod room {
    room_fields!(gen_consts);
}

pub mod space {
    space_fields!(gen_consts);
}

pub mod emoji_entry {
    pub const BASE: &str = "base";
    pub const TONES: &str = "tones";
    pub const NAME: &str = "name";
}

pub mod emoji_group {
    pub const ITEMS: &str = "items";
}

pub mod emoji_insert {
    pub const TEXT: &str = "text";
    pub const CARET: &str = "caret";
}

pub mod verification_emoji {
    pub const SYMBOL: &str = "symbol";
    pub const DESCRIPTION: &str = "description";
}

pub mod login_request {
    pub const HOMESERVER: &str = "homeserver";
    pub const USERNAME: &str = "username";
    pub const PASSWORD: &str = "password";
}

pub mod send_message_request {
    pub const ROOM_ID: &str = "room-id";
    pub const BODY: &str = "body";
    pub const REPLY_TO: &str = "reply-to";
}

pub mod save_file_request {
    pub const EVENT_ID: &str = "event-id";
    pub const FILENAME: &str = "filename";
}
