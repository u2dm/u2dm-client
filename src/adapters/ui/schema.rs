#![allow(dead_code)]

pub mod prop {
    pub const LOGIN_STEP: &str = "login-step";
    pub const LOGIN_STATUS: &str = "login-status";
    pub const LOGIN_ERROR: &str = "login-error";
    pub const LOGIN_METHOD: &str = "login-method";
    pub const RESOLVED_HOMESERVER: &str = "resolved-homeserver";
    pub const USER_ID: &str = "user-id";
    pub const USER_INITIAL: &str = "user-initial";
    pub const TOAST_MESSAGE: &str = "toast-message";
    pub const CONNECTION_STATUS: &str = "connection-status";
    pub const VERIFICATION_STEP: &str = "verification-step";
    pub const VERIFICATION_SENDER: &str = "verification-sender";
    pub const VERIFICATION_ERROR: &str = "verification-error";
    pub const SAVED_FILE_PATH: &str = "saved-file-path";
    pub const SELECTED_ROOM_NAME: &str = "selected-room-name";
    pub const SELECTED_ROOM_ID: &str = "selected-room-id";
    pub const SELECTED_SPACE_ID: &str = "selected-space-id";
    pub const SELECTED_SUBSPACE_ID: &str = "selected-subspace-id";
    pub const TIMELINE_STATUS: &str = "timeline-status";
    pub const INPUT_USERNAME: &str = "input-username";
    pub const INPUT_PASSWORD: &str = "input-password";

    pub const VERIFICATION_VISIBLE: &str = "verification-visible";
    pub const VERIFICATION_IS_SELF: &str = "verification-is-self";
    pub const TIMELINE_RETRYABLE: &str = "timeline-retryable";
    pub const BACKWARDS_LOADING: &str = "backwards-loading";
    pub const FORWARDS_LOADING: &str = "forwards-loading";

    pub const NEW_MESSAGES_COUNT: &str = "new-messages-count";
    pub const PREPEND_TOKEN: &str = "prepend-token";
    pub const SELECTED_ROOM_MEMBERS: &str = "selected-room-members";
    pub const SELECTED_GENERATION: &str = "selected-generation";

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
    pub const UNIQUE_ID: &str = "unique-id";
    pub const SENDER: &str = "sender";
    pub const PRONOUNS: &str = "pronouns";
    pub const BODY: &str = "body";
    pub const TIMESTAMP: &str = "timestamp";
    pub const MESSAGE_TYPE: &str = "message-type";
    pub const PREVIEW_KIND: &str = "preview-kind";
    pub const UNSUPPORTED_KIND: &str = "unsupported-kind";
    pub const HAS_THUMBNAIL: &str = "has-thumbnail";
    pub const THUMBNAIL: &str = "thumbnail";
    pub const MEDIA_FAILED: &str = "media-failed";
    pub const IMAGE_WIDTH: &str = "image-width";
    pub const IMAGE_HEIGHT: &str = "image-height";
    pub const EVENT_ID: &str = "event-id";
    pub const HAS_AVATAR: &str = "has-avatar";
    pub const AVATAR: &str = "avatar";
    pub const SENDER_INITIAL: &str = "sender-initial";
    pub const COLOR_INDEX: &str = "color-index";
    pub const IS_OWN: &str = "is-own";
    pub const EDITED: &str = "edited";
    pub const HAS_REPLY: &str = "has-reply";
    pub const REPLY_SENDER: &str = "reply-sender";
    pub const REPLY_KIND: &str = "reply-kind";
    pub const REPLY_BODY: &str = "reply-body";
    pub const SERVICE_KIND: &str = "service-kind";
    pub const SERVICE_TARGET: &str = "service-target";
}

pub mod room {
    pub const ID: &str = "id";
    pub const NAME: &str = "name";
    pub const INITIAL: &str = "initial";
    pub const AVATAR: &str = "avatar";
    pub const HAS_AVATAR: &str = "has-avatar";
    pub const COLOR_INDEX: &str = "color-index";
    pub const MEMBERS: &str = "members";
    pub const UNREAD: &str = "unread";
    pub const MENTIONS: &str = "mentions";
    pub const LAST_MESSAGE_SENDER: &str = "last-message-sender";
    pub const LAST_MESSAGE_KIND: &str = "last-message-kind";
    pub const LAST_MESSAGE_BODY: &str = "last-message-body";
    pub const LAST_MESSAGE_SERVICE_KIND: &str = "last-message-service-kind";
    pub const LAST_MESSAGE_SERVICE_TARGET: &str = "last-message-service-target";
    pub const LAST_MESSAGE_IS_OWN: &str = "last-message-is-own";
    pub const LAST_MESSAGE_EDITED: &str = "last-message-edited";
    pub const LAST_MESSAGE_TIME: &str = "last-message-time";
}

pub mod space {
    pub const ID: &str = "id";
    pub const NAME: &str = "name";
    pub const UNREAD: &str = "unread";
    pub const MENTIONS: &str = "mentions";
    pub const INITIAL: &str = "initial";
    pub const AVATAR: &str = "avatar";
    pub const HAS_AVATAR: &str = "has-avatar";
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
