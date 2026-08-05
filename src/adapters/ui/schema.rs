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
        FocusEventId RoomView "RoomView" "focus-event-id" set_focus_event_id;
        SelectedRoomId DirectoryView "DirectoryView" "selected-room-id" set_selected_room_id;
        SelectedSpaceId DirectoryView "DirectoryView" "selected-space-id" set_selected_space_id;
        SelectedSubspaceId DirectoryView "DirectoryView" "selected-subspace-id" set_selected_subspace_id;
    } };
}
pub(crate) use string_props;

macro_rules! simple_callbacks {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        on_check_server "check-server" check_server pass CheckServer;
        on_login_oauth "login-oauth" login_oauth plain LoginOAuth;
        on_cancel_oauth "cancel-oauth" cancel_oauth plain CancelOAuth;
        on_back_to_homeserver "back-to-homeserver" back_to_homeserver plain BackToHomeserver;
        on_logout "logout" logout plain Logout;
        on_dismiss_toast "dismiss-toast" dismiss_toast plain DismissToast;
        on_accept_verification "accept-verification" accept_verification plain AcceptVerification;
        on_confirm_verification "confirm-verification" confirm_verification plain ConfirmVerification;
        on_reject_verification "reject-verification" reject_verification plain RejectVerification;
        on_dismiss_verification "dismiss-verification" dismiss_verification plain DismissVerification;
        on_retry_timeline "retry-timeline" retry_timeline plain RetryTimeline;
        on_select_room "select-room" select_room room SelectRoom;
        on_select_space "select-space" select_space opt_room SelectSpace;
        on_select_subspace "select-subspace" select_subspace opt_room SelectSubspace;
        on_open_media "open-media" open_media manual_string OpenMedia;
        on_jump_to_event "jump-to-event" jump_to_event manual_string JumpToEvent;
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
        StickerRoomEncrypted StickerView "StickerView" "room-encrypted" set_room_encrypted;
        StickerLoading StickerView "StickerView" "loading" set_loading;
        StickerHasPacks StickerView "StickerView" "has-packs" set_has_packs;
    } };
}
pub(crate) use bool_props;

macro_rules! int_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        NewMessagesCount RoomView "RoomView" "new-messages-count" set_new_messages_count;
        AnchorIndex RoomView "RoomView" "anchor-index" set_anchor_index;
        TimelineToken RoomView "RoomView" "timeline-token" set_timeline_token;
        PrependToken RoomView "RoomView" "prepend-token" set_prepend_token;
        SelectedRoomMembers RoomView "RoomView" "selected-room-members" set_selected_room_members;
        SelectedGeneration DirectoryView "DirectoryView" "selected-generation" set_selected_generation;
        StickerColumns StickerView "StickerView" "columns" set_columns;
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
        LoadingUnread LoadingUnread "loading-unread";
        LoadingFocus LoadingFocus "loading-focus";
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

macro_rules! verification_activities {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None       None       "none";
        Accepting  Accepting  "accepting";
        Declining  Declining  "declining";
        Confirming Confirming "confirming";
    } };
}
pub(crate) use verification_activities;

macro_rules! user_message_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None                      None                    "none";
        ServerUnreachable         ServerUnreachable       "server-unreachable";
        UnsupportedLoginMethod    UnsupportedLoginMethod  "unsupported-login-method";
        LoginFailed               LoginFailed             "login-failed";
        InvalidCredentials        InvalidCredentials      "invalid-credentials";
        AccountDeactivated        AccountDeactivated      "account-deactivated";
        InvalidUsername           InvalidUsername         "invalid-username";
        RateLimited               RateLimited             "rate-limited";
        LoginMethodUnsupported    LoginMethodUnsupported  "login-method-unsupported";
        SessionUnreadable         SessionUnreadable       "session-unreadable";
        SessionRestoreFailed      SessionRestoreFailed    "session-restore-failed";
        StoreKeyMissing           StoreKeyMissing         "store-key-missing";
        StoreKeyUnreadable        StoreKeyUnreadable      "store-key-unreadable";
        IdentityDiverged          IdentityDiverged        "identity-diverged";
        SessionExpired            SessionExpired          "session-expired";
        DataQuarantined           DataQuarantined         "data-quarantined";
        DataNotErased             DataNotErased           "data-not-erased";
        InterruptedLoginUnresolved InterruptedLoginUnresolved "interrupted-login-unresolved";
        SessionSaveFailed         SessionSaveFailed       "session-save-failed";
        SendMessageFailed         SendMessageFailed       "send-message-failed";
        LoadMoreFailed            LoadMoreFailed          "load-more-failed";
        MessageNotFound           MessageNotFound         "message-not-found";
        MessageNotShowable        MessageNotShowable      "message-not-showable";
        SpaceOrderSaveFailed      SpaceOrderSaveFailed    "space-order-save-failed";
        MediaDownloadFailed       MediaDownloadFailed     "media-download-failed";
        FileDownloadFailed        FileDownloadFailed      "file-download-failed";
        MediaOpenFailed           MediaOpenFailed         "media-open-failed";
        MediaNotViewable          MediaNotViewable        "media-not-viewable";
        FileSaveFailed            FileSaveFailed          "file-save-failed";
        FileSaved                 FileSaved               "file-saved";
        VerificationAcceptFailed  VerificationAcceptFailed "verification-accept-failed";
        VerificationConfirmFailed VerificationConfirmFailed "verification-confirm-failed";
        VerificationRejectFailed  VerificationRejectFailed "verification-reject-failed";
        VerificationTimedOut      VerificationTimedOut    "verification-timed-out";
        VerificationSasAcceptFailed VerificationSasAcceptFailed "verification-sas-accept-failed";
        VerificationCancelled     VerificationCancelled   "verification-cancelled";
        VerificationDeclined      VerificationDeclined    "verification-declined";
        VerificationMismatch      VerificationMismatch    "verification-mismatch";
        VerificationAcceptedElsewhere VerificationAcceptedElsewhere "verification-accepted-elsewhere";
        VerificationFailed        VerificationFailed      "verification-failed";
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
        Sticker     Sticker     "sticker";
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
        is_first_unread IS_FIRST_UNREAD "first-unread" flag;
        has_reply HAS_REPLY "has-reply" flag;
        reply_event_id REPLY_EVENT_ID "reply-event-id" text;
        reply_sender REPLY_SENDER "reply-sender" text;
        reply_kind REPLY_KIND "reply-kind" enumk;
        reply_body REPLY_BODY "reply-body" text;
        service_kind SERVICE_KIND "service-kind" enumk;
        service_target SERVICE_TARGET "service-target" text;
        media_state MEDIA_STATE "media-state" enumk;
        image_width IMAGE_WIDTH "image-width" int;
        image_height IMAGE_HEIGHT "image-height" int;
        has_avatar HAS_AVATAR "has-avatar" flag;
        needs_media NEEDS_MEDIA "needs-media" flag;
        thumbnail THUMBNAIL "thumbnail" image;
        avatar AVATAR "avatar" image;
        reactions REACTIONS "reactions" structs;
        all_reactions ALL_REACTIONS "all-reactions" structs;
    } };
}
#[cfg(feature = "interpreted")]
pub(crate) use message_fields;

#[cfg(feature = "interpreted")]
macro_rules! reaction_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        key KEY "key" text;
        label LABEL "label" text;
        count COUNT "count" int;
        mine MINE "mine" flag;
        pending PENDING "pending" flag;
        overflow OVERFLOW "overflow" flag;
        reactors REACTORS "reactors" text;
        hidden_reactors HIDDEN_REACTORS "hidden-reactors" int;
    } };
}
#[cfg(feature = "interpreted")]
pub(crate) use reaction_fields;

#[cfg(feature = "interpreted")]
macro_rules! room_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        id ID "id" text;
        name NAME "name" text;
        initial INITIAL "initial" text;
        color_index COLOR_INDEX "color-index" int;
        members MEMBERS "members" int;
        alert ALERT "alert" flag;
        mention MENTION "mention" flag;
        hint HINT "hint" flag;
        muted MUTED "muted" flag;
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
        alert ALERT "alert" flag;
        mention MENTION "mention" flag;
        hint HINT "hint" flag;
        initial INITIAL "initial" text;
        has_avatar HAS_AVATAR "has-avatar" flag;
        avatar AVATAR "avatar" image;
    } };
}
#[cfg(feature = "interpreted")]
pub(crate) use space_fields;

#[cfg(feature = "interpreted")]
macro_rules! sticker_cell_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        key KEY "key" text;
        pack_id PACK_ID "pack-id" text;
        shortcode SHORTCODE "shortcode" text;
        label LABEL "label" text;
        media_state MEDIA_STATE "media-state" enumk;
        image IMAGE "image" image;
    } };
}
#[cfg(feature = "interpreted")]
pub(crate) use sticker_cell_fields;

#[cfg(feature = "interpreted")]
macro_rules! sticker_pack_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        id ID "id" text;
        title TITLE "title" text;
        header_row HEADER_ROW "header-row" int;
        icon ICON "icon" image;
        has_icon HAS_ICON "has-icon" flag;
    } };
}
#[cfg(feature = "interpreted")]
pub(crate) use sticker_pack_fields;

#[cfg(feature = "interpreted")]
macro_rules! sticker_row_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        title TITLE "title" text;
        is_header IS_HEADER "is-header" flag;
        cells CELLS "cells" list;
    } };
}
#[cfg(feature = "interpreted")]
pub(crate) use sticker_row_fields;
