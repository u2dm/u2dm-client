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
        ResolvedHomeserver LoginView "LoginView" "resolved-homeserver" set_resolved_homeserver;
        UserId SessionView "SessionView" "user-id" set_user_id;
        UserInitial SessionView "SessionView" "user-initial" set_user_initial;
        ToastDetail RoomView "RoomView" "toast-detail" set_toast_detail;
        VerificationSender VerificationView "VerificationView" "sender" set_sender;
        VerificationErrorDetail VerificationView "VerificationView" "error-detail" set_error_detail;
        SelectedRoomName RoomView "RoomView" "selected-room-name" set_selected_room_name;
        SelectedRoomId DirectoryView "DirectoryView" "selected-room-id" set_selected_room_id;
        SelectedSpaceId DirectoryView "DirectoryView" "selected-space-id" set_selected_space_id;
        SelectedSubspaceId DirectoryView "DirectoryView" "selected-subspace-id" set_selected_subspace_id;
    } };
}
pub(crate) use string_props;

macro_rules! simple_callbacks {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        on_check_server "check-server" Actions "Actions" check_server pass CheckServer;
        on_login_oauth "login-oauth" Actions "Actions" login_oauth plain LoginOAuth;
        on_cancel_oauth "cancel-oauth" Actions "Actions" cancel_oauth plain CancelOAuth;
        on_back_to_homeserver "back-to-homeserver" Actions "Actions" back_to_homeserver plain BackToHomeserver;
        on_logout "logout" Actions "Actions" logout plain Logout;
        on_dismiss_toast "dismiss-toast" Actions "Actions" dismiss_toast plain DismissToast;
        on_accept_verification "accept-verification" Actions "Actions" accept_verification plain AcceptVerification;
        on_confirm_verification "confirm-verification" Actions "Actions" confirm_verification plain ConfirmVerification;
        on_reject_verification "reject-verification" Actions "Actions" reject_verification plain RejectVerification;
        on_dismiss_verification "dismiss-verification" Actions "Actions" dismiss_verification plain DismissVerification;
        on_retry_timeline "retry-timeline" Actions "Actions" retry_timeline plain RetryTimeline;
        on_select_room "select-room" Actions "Actions" select_room room SelectRoom;
        on_select_space "select-space" Actions "Actions" select_space opt_room SelectSpace;
        on_select_subspace "select-subspace" Actions "Actions" select_subspace opt_room SelectSubspace;
        on_open_media "open-media" Actions "Actions" open_media manual_string OpenMedia;
    } };
}
pub(crate) use simple_callbacks;

macro_rules! bool_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        VerificationVisible VerificationView "VerificationView" "visible" set_visible;
        VerificationBusy VerificationView "VerificationView" "busy" set_busy;
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

macro_rules! define_ui_enum {
    ($name:ident; $($rust:ident $ui:ident $lit:literal;)*) => {
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($rust,)* }
    };
}
pub(crate) use define_ui_enum;

macro_rules! login_phases {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Loading     Loading     "loading";
        Homeserver  Homeserver  "homeserver";
        Credentials Credentials "credentials";
        LoggedIn    LoggedIn    "logged-in";
    } };
}
pub(crate) use login_phases;

macro_rules! login_activities {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Idle           Idle           "idle";
        LoadingSession LoadingSession "loading-session";
        OpeningStore   OpeningStore   "opening-store";
        Connecting     Connecting     "connecting";
        RestoringAuth  RestoringAuth  "restoring-auth";
        CheckingServer CheckingServer "checking-server";
        LoggingIn      LoggingIn      "logging-in";
        OpeningBrowser OpeningBrowser "opening-browser";
        WaitingAuth    WaitingAuth    "waiting-auth";
        Syncing        Syncing        "syncing";
        CleaningUp     CleaningUp     "cleaning-up";
    } };
}
pub(crate) use login_activities;

macro_rules! login_methods {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None     None     "none";
        Password Password "password";
        OAuth    Oauth    "oauth";
        Both     Both     "both";
    } };
}
pub(crate) use login_methods;

macro_rules! connection_states {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Disconnected Disconnected "disconnected";
        Connecting   Connecting   "connecting";
        Connected    Connected    "connected";
        Error(_)     Error        "error";
    } };
}
pub(crate) use connection_states;

macro_rules! timeline_states {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Loading      Loading      "loading";
        Ready        Ready        "ready";
        Failed{..}   Failed       "failed";
        Disconnected Disconnected "disconnected";
    } };
}
pub(crate) use timeline_states;

macro_rules! verification_phases {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None       None       "none";
        Requested  Requested  "requested";
        Emojis     Emojis     "emojis";
        Confirming Confirming "confirming";
        Done       Done       "done";
        Cancelled  Cancelled  "cancelled";
    } };
}
pub(crate) use verification_phases;

macro_rules! toast_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None      None      "none";
        Error     Error     "error";
        FileSaved FileSaved "file-saved";
    } };
}
pub(crate) use toast_kinds;

macro_rules! user_message_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None                      None                    "none";
        ServerUnreachable         ServerUnreachable       "server-unreachable";
        LoginFailed               LoginFailed             "login-failed";
        SessionUnreadable         SessionUnreadable       "session-unreadable";
        SessionRestoreFailed      SessionRestoreFailed    "session-restore-failed";
        StoreKeyMissing           StoreKeyMissing         "store-key-missing";
        StoreKeyUnreadable        StoreKeyUnreadable      "store-key-unreadable";
        SessionExpired            SessionExpired          "session-expired";
        DataQuarantined           DataQuarantined         "data-quarantined";
        DataNotErased             DataNotErased           "data-not-erased";
        SessionSaveFailed         SessionSaveFailed       "session-save-failed";
        SendMessageFailed         SendMessageFailed       "send-message-failed";
        LoadMoreFailed            LoadMoreFailed          "load-more-failed";
        SpaceOrderSaveFailed      SpaceOrderSaveFailed    "space-order-save-failed";
        MediaDownloadFailed       MediaDownloadFailed     "media-download-failed";
        FileDownloadFailed        FileDownloadFailed      "file-download-failed";
        MediaOpenFailed           MediaOpenFailed         "media-open-failed";
        FileSaveFailed            FileSaveFailed          "file-save-failed";
        FileSaved                 FileSaved               "file-saved";
        VerificationAcceptFailed  VerificationAcceptFailed "verification-accept-failed";
        VerificationConfirmFailed VerificationConfirmFailed "verification-confirm-failed";
        VerificationRejectFailed  VerificationRejectFailed "verification-reject-failed";
        VerificationTimedOut      VerificationTimedOut    "verification-timed-out";
        VerificationSasAcceptFailed VerificationSasAcceptFailed "verification-sas-accept-failed";
        VerificationCancelled     VerificationCancelled   "verification-cancelled";
    } };
}
pub(crate) use user_message_kinds;

macro_rules! media_states {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Idle   Idle   "idle";
        Ready  Ready  "ready";
        Failed Failed "failed";
    } };
}
pub(crate) use media_states;

macro_rules! message_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Text        Text        "text";
        Notice      Notice      "notice";
        Emote       Emote       "emote";
        Image       Image       "image";
        File        File        "file";
        Service     Service     "service";
        Utd         Utd         "utd";
        Unsupported Unsupported "unsupported";
    } };
}
pub(crate) use message_kinds;

macro_rules! preview_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None      None      "none";
        Text      Text      "text";
        Image     Image     "image";
        Video     Video     "video";
        Audio     Audio     "audio";
        File      File      "file";
        Location  Location  "location";
        Encrypted Encrypted "encrypted";
        Sticker   Sticker   "sticker";
    } };
}
pub(crate) use preview_kinds;

macro_rules! service_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None               None               "none";
        Joined             Joined             "joined";
        Left               Left               "left";
        Invited            Invited            "invited";
        InvitationAccepted InvitationAccepted "invitation-accepted";
        InvitationRejected InvitationRejected "invitation-rejected";
        InvitationRevoked  InvitationRevoked  "invitation-revoked";
        Kicked             Kicked             "kicked";
        Banned             Banned             "banned";
        Unbanned           Unbanned           "unbanned";
        Knocked            Knocked            "knocked";
        KnockAccepted      KnockAccepted      "knock-accepted";
        NameSet            NameSet            "name-set";
        NameChanged        NameChanged        "name-changed";
        NameRemoved        NameRemoved        "name-removed";
        AvatarChanged      AvatarChanged      "avatar-changed";
        RoomName           RoomName           "room-name";
        RoomTopic          RoomTopic          "room-topic";
        RoomAvatar         RoomAvatar         "room-avatar";
        RoomCreated        RoomCreated        "room-created";
        Encryption         Encryption         "encryption";
        CallStarted        CallStarted        "call-started";
        CallNotification   CallNotification   "call-notification";
    } };
}
pub(crate) use service_kinds;

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
