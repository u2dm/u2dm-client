#![allow(dead_code)]

#[cfg(feature = "interpreted")]
macro_rules! gen_consts {
    ($($a:ident $c:ident $lit:literal $d:ident;)*) => {
        $( pub const $c: &str = $lit; )*
    };
}
#[cfg(feature = "interpreted")]
pub(crate) use gen_consts;

macro_rules! string_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        LoginStatus LoginView "LoginView" "status" set_status;
        LoginError LoginView "LoginView" "error" set_error;
        ResolvedHomeserver LoginView "LoginView" "resolved-homeserver" set_resolved_homeserver;
        UserId SessionView "SessionView" "user-id" set_user_id;
        UserInitial SessionView "SessionView" "user-initial" set_user_initial;
        ToastMessage RoomView "RoomView" "toast-message" set_toast_message;
        VerificationSender VerificationView "VerificationView" "sender" set_sender;
        VerificationError VerificationView "VerificationView" "error" set_error;
        SavedFilePath RoomView "RoomView" "saved-file-path" set_saved_file_path;
        SelectedRoomName RoomView "RoomView" "selected-room-name" set_selected_room_name;
        SelectedRoomId DirectoryView "DirectoryView" "selected-room-id" set_selected_room_id;
        SelectedSpaceId DirectoryView "DirectoryView" "selected-space-id" set_selected_space_id;
        SelectedSubspaceId DirectoryView "DirectoryView" "selected-subspace-id" set_selected_subspace_id;
    } };
}
pub(crate) use string_props;

macro_rules! simple_callbacks {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        on_cancel_oauth "cancel-oauth" cancel_oauth unit;
        on_logout "logout" logout unit;
        on_accept_verification "accept-verification" accept_verification unit;
        on_confirm_verification "confirm-verification" confirm_verification unit;
        on_reject_verification "reject-verification" reject_verification unit;
        on_retry_timeline "retry-timeline" retry_timeline unit;
        on_select_room "select-room" select_room string;
        on_select_space "select-space" select_space string;
        on_select_subspace "select-subspace" select_subspace string;
        on_open_media "open-media" open_media string;
    } };
}
pub(crate) use simple_callbacks;

macro_rules! bool_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        VerificationVisible VerificationView "VerificationView" "visible" set_visible;
        VerificationIsSelf VerificationView "VerificationView" "is-self" set_is_self;
        TimelineRetryable RoomView "RoomView" "timeline-retryable" set_timeline_retryable;
        BackwardsLoading RoomView "RoomView" "backwards-loading" set_backwards_loading;
        ForwardsLoading RoomView "RoomView" "forwards-loading" set_forwards_loading;
    } };
}
pub(crate) use bool_props;

macro_rules! int_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        NewMessagesCount RoomView "RoomView" "new-messages-count" set_new_messages_count;
        PrependToken RoomView "RoomView" "prepend-token" set_prepend_token;
        SelectedRoomMembers RoomView "RoomView" "selected-room-members" set_selected_room_members;
        SelectedGeneration DirectoryView "DirectoryView" "selected-generation" set_selected_generation;
    } };
}
pub(crate) use int_props;

#[cfg(feature = "interpreted")]
macro_rules! message_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        unique_id UNIQUE_ID "unique-id" text;
        sender SENDER "sender" text;
        pronouns PRONOUNS "pronouns" list;
        body BODY "body" text;
        timestamp TIMESTAMP "timestamp" text;
        message_type MESSAGE_TYPE "message-type" enumk;
        preview_kind PREVIEW_KIND "preview-kind" enumk;
        unsupported_kind UNSUPPORTED_KIND "unsupported-kind" text;
        event_id EVENT_ID "event-id" text;
        sender_initial SENDER_INITIAL "sender-initial" text;
        color_index COLOR_INDEX "color-index" int;
        is_own IS_OWN "is-own" flag;
        edited EDITED "edited" flag;
        has_reply HAS_REPLY "has-reply" flag;
        reply_sender REPLY_SENDER "reply-sender" text;
        reply_kind REPLY_KIND "reply-kind" enumk;
        reply_body REPLY_BODY "reply-body" text;
        service_kind SERVICE_KIND "service-kind" enumk;
        service_target SERVICE_TARGET "service-target" text;
        media_state MEDIA_STATE "media-state" enumk;
        image_width IMAGE_WIDTH "image-width" int;
        image_height IMAGE_HEIGHT "image-height" int;
        has_avatar HAS_AVATAR "has-avatar" flag;
        thumbnail THUMBNAIL "thumbnail" image;
        avatar AVATAR "avatar" image;
    } };
}
#[cfg(feature = "interpreted")]
pub(crate) use message_fields;

#[cfg(feature = "interpreted")]
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
        last_message_kind LAST_MESSAGE_KIND "last-message-kind" enumk;
        last_message_body LAST_MESSAGE_BODY "last-message-body" text;
        last_message_service_kind LAST_MESSAGE_SERVICE_KIND "last-message-service-kind" enumk;
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

#[cfg(feature = "interpreted")]
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
