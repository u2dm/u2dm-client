pub mod callback {
    pub const GLOBAL: &str = "Actions";
    pub const MOVE_SPACE: &str = "move-space";
    pub const DISMISS_UNSENT: &str = "dismiss-unsent";
    pub const EDIT_MESSAGE: &str = "edit-message";
    pub const LAST_EDITABLE_ROW: &str = "last-editable-row";
    pub const ROW_OF_MESSAGE: &str = "row-of-message";
    pub const SUGGEST_MENTIONS: &str = "suggest-mentions";
    pub const MENTION_AT: &str = "mention-at";
    pub const COMPLETE_MENTION: &str = "complete-mention";
    pub const SET_ROOM_NOTIFY: &str = "set-room-notify";
    pub const PASTE_ATTACHMENT: &str = "paste-attachment";
    pub const TOGGLE_REACTION: &str = "toggle-reaction";
    pub const REACT_WITH_EMOTE: &str = "react-with-emote";
    pub const SET_PACK_GLOBAL: &str = "set-pack-global";
    pub const POLL_ANSWER_TEXTS: &str = "poll-answer-texts";
    pub const ROOM_LOG_TEXT: &str = "room-log-text";
    pub const REQUEST_MEDIA: &str = "request-media";
    pub const REQUEST_ROOM_AVATAR: &str = "request-room-avatar";
    pub const REQUEST_PACK_CELL: &str = "request-pack-cell";
    pub const REQUEST_CUSTOM_EMOJI: &str = "request-custom-emoji";
    pub const TOGGLE_VIDEO: &str = "toggle-video";
    pub const TOGGLE_VIDEO_MUTED: &str = "toggle-video-muted";
    pub const SEEK_VIDEO: &str = "seek-video";
    pub const TOGGLE_AUDIO: &str = "toggle-audio";
    pub const SEEK_AUDIO: &str = "seek-audio";
    pub const BROWSE_PACKS: &str = "browse-packs";
    pub const REVEAL_SPOILERS: &str = "reveal-spoilers";
    pub const CALL_TILE_SEEN: &str = "call-tile-seen";
    pub const CALL_MEDIA_SHOWN: &str = "call-media-shown";
    pub const WATCH_MICROPHONE: &str = "watch-microphone";
    pub const WATCH_CONNECTIONS: &str = "watch-connections";
    pub const SHOW_CALL_WINDOW: &str = "show-call-window";
    pub const CLOSE_CALL_WINDOW: &str = "close-call-window";
    pub const SHOW_CALL_CHAT: &str = "show-call-chat";
    pub const SHOW_MAIN_WINDOW: &str = "show-main-window";
    pub const CALL_PREVIEW_SEEN: &str = "call-preview-seen";
    pub const EMOTE_AT: &str = "emote-at";
    pub const PLACE_EMOTE: &str = "place-emote";
    pub const SCROLL_POSITION_CHANGED: &str = "scroll-position-changed";
    pub const TIMELINE_VISIBILITY_CHANGED: &str = "timeline-visibility-changed";
    pub const COMPOSER_CHANGED: &str = "composer-changed";
    pub const KEEP_DRAFT: &str = "keep-draft";
    pub const TAKE_DRAFT: &str = "take-draft";
    pub const WINDOW_FOCUS_CHANGED: &str = "window-focus-changed";
    pub const USER_ACTIVITY_CHANGED: &str = "user-activity-changed";
}

pub mod emoji_store {
    pub const NAME: &str = "EmojiStore";
    pub const ROWS: &str = "rows";
    pub const HEADER_ROWS: &str = "header-rows";
    pub const RESULT_ROWS: &str = "result-rows";
    pub const LAYOUT: &str = "layout";
    pub const SEARCH: &str = "search";
    pub const INSERT: &str = "insert";
}

pub mod emoji_entry {
    pub const BASE: &str = "base";
    pub const TONES: &str = "tones";
    pub const NAME: &str = "name";
}

pub mod emoji_row {
    pub const IS_HEADER: &str = "is-header";
    pub const GROUP: &str = "group";
    pub const CELLS: &str = "cells";
}

pub mod text_splice {
    pub const TEXT: &str = "text";
    pub const CARET: &str = "caret";
}

pub mod draft_verdict {
    pub const ACCEPTED: &str = "accepted";
    pub const SHORTEN_BY: &str = "shorten-by";
}

pub mod composer_draft {
    pub const HELD: &str = "held";
    pub const ROOM_ID: &str = "room-id";
}

pub mod mention_word {
    pub const START: &str = "start";
    pub const QUERY: &str = "query";
}

pub mod user_message {
    pub const KIND: &str = "kind";
    pub const DETAIL: &str = "detail";
}

pub mod verification_emoji {
    pub const SYMBOL: &str = "symbol";
    pub const DESCRIPTION: &str = "description";
}

pub mod video_view {
    pub const NAME: &str = "VideoView";
    pub const FRAME: &str = "frame";
    pub const HAS_FRAME: &str = "has-frame";
}

pub mod room_info_view {
    pub const NAME: &str = "RoomInfoView";
    pub const TOPIC_STYLED: &str = "topic-styled";
}

pub mod login_view {
    pub const NAME: &str = "LoginView";
    pub const MESSAGES: &str = "messages";
}

pub mod verification_view {
    pub const NAME: &str = "VerificationView";
    pub const EMOJIS: &str = "emojis";
}

pub mod notification_text {
    pub const NAME: &str = "NotificationText";
    pub const MESSAGE: &str = "message";
    pub const HIDDEN_MESSAGE: &str = "hidden-message";
    pub const SERVICE: &str = "service";
    pub const INVITE: &str = "invite";
    pub const RING: &str = "ring";
}

pub mod window_props {
    pub const INPUT_USERNAME: &str = "input-username";
    pub const INPUT_MESSAGE: &str = "input-message";
    pub const READY: &str = "ready";
}
