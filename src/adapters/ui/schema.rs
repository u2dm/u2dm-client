#[cfg(feature = "interpreted")]
macro_rules! gen_consts {
    ($($a:ident $c:ident $lit:literal $d:ident;)*) => {
        $( pub const $c: &str = $lit; )*
    };
}
#[cfg(feature = "interpreted")]
pub(crate) use gen_consts;

#[cfg(feature = "demo")]
macro_rules! enum_names {
    (val $fn:ident $src:ident; $($rows:tt)*) => {
        pub fn $fn(value: $src) -> &'static str { enum_names!(@source value, $src, $($rows)*) }
    };
    (ref $fn:ident $src:ident; $($rows:tt)*) => {
        pub fn $fn(value: &$src) -> &'static str { enum_names!(@source value, $src, $($rows)*) }
    };
    (slint $fn:ident $ui_enum:ident; $($rows:tt)*) => {
        fn $fn(value: $ui_enum) -> &'static str { enum_names!(@slint value, $ui_enum, $($rows)*) }
    };
    (@source $v:ident, $src:ident,
        $($rust:ident $(($($p:tt)*))? $({$($b:tt)*})? $ui:ident $lit:literal;)*) => {
        match $v { $($src::$rust $(($($p)*))? $({$($b)*})? => $lit,)* }
    };
    (@slint $v:ident, $ui_enum:ident,
        $($rust:ident $(($($p:tt)*))? $({$($b:tt)*})? $ui:ident $lit:literal;)*) => {
        match $v { $($ui_enum::$ui => $lit,)* }
    };
}
#[cfg(feature = "demo")]
pub(crate) use enum_names;

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
        AttachmentFilename AttachmentView "AttachmentView" "filename" set_filename;
        AttachmentMimetype AttachmentView "AttachmentView" "mimetype" set_mimetype;
        AttachmentExtension AttachmentView "AttachmentView" "extension" set_extension;
        AttachmentSize AttachmentView "AttachmentView" "size" set_size;
        AttachmentErrorDetail AttachmentView "AttachmentView" "error-detail" set_error_detail;
        AudioEventId AudioView "AudioView" "event-id" set_event_id;
        AudioRoomId AudioView "AudioView" "room-id" set_room_id;
        AudioSender AudioView "AudioView" "sender" set_sender;
        AudioTitle AudioView "AudioView" "title" set_title;
        AttachmentDuration AttachmentView "AttachmentView" "duration" set_duration;
    } };
}
pub(crate) use string_props;

macro_rules! simple_callbacks {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        on_check_server "check-server" check_server pass CheckServer;
        on_login_password "login-password" login_password
            request(username "username" text, password "password" text) LoginPassword;
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
        on_paginate_backwards "paginate-backwards" paginate_backwards room_key PaginateBackwards;
        on_paginate_forwards "paginate-forwards" paginate_forwards room_key PaginateForwards;
        on_jump_to_latest "jump-to-latest" jump_to_latest room_key JumpToLatest;
        on_send_message "send-message" send_message
            request(room_id "room-id" text, body "body" text, reply_to "reply-to" text) SendMessage;
        on_send_sticker "send-sticker" send_sticker
            request(room_id "room-id" text, pack_id "pack-id" text, shortcode "shortcode" text,
                reply_to "reply-to" text) SendSticker;
        on_send_attachment "send-attachment" send_attachment
            request(room_id "room-id" text, caption "caption" text, as_document "as-document" flag,
                reply_to "reply-to" text) SendAttachment;
        on_save_file "save-file" save_file
            request(event_id "event-id" text, filename "filename" text) SaveFile;
        on_open_media "open-media" open_media manual_string OpenMedia;
        on_open_video "open-video" open_video manual_string OpenVideo;
        on_close_video "close-video" close_video plain CloseVideo;
        on_play_audio "play-audio" play_audio manual_string PlayAudio;
        on_close_audio "close-audio" close_audio plain CloseAudio;
        on_open_link "open-link" open_link manual_string OpenLink;
        on_jump_to_event "jump-to-event" jump_to_event manual_string JumpToEvent;
        on_pick_photo "pick-photo" pick_photo manual_string PickAttachment;
        on_pick_document "pick-document" pick_document manual_string PickAttachment;
        on_cancel_attachment "cancel-attachment" cancel_attachment plain CancelAttachment;
        on_retry_send "retry-send" retry_send manual_string RetrySend;
        on_discard_send "discard-send" discard_send manual_string DiscardSend;
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
        #[cfg(feature = "demo")] ProbeEnabled Probe "Probe" "enabled" set_enabled;
        StickerLoading StickerView "StickerView" "loading" set_loading;
        StickerHasPacks StickerView "StickerView" "has-packs" set_has_packs;
        AttachmentVisible AttachmentView "AttachmentView" "visible" set_visible;
        AttachmentSending AttachmentView "AttachmentView" "sending" set_sending;
        VideoVisible VideoView "VideoView" "visible" set_visible;
        VideoLoading VideoView "VideoView" "loading" set_loading;
        VideoPlaying VideoView "VideoView" "playing" set_playing;
        VideoMuted VideoView "VideoView" "muted" set_muted;
        AudioVisible AudioView "AudioView" "visible" set_visible;
        AudioLoading AudioView "AudioView" "loading" set_loading;
        AudioPlaying AudioView "AudioView" "playing" set_playing;
        AudioSilent AudioView "AudioView" "silent" set_silent;
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
        AttachmentWidth AttachmentView "AttachmentView" "width" set_width;
        AttachmentHeight AttachmentView "AttachmentView" "height" set_height;
        VideoPositionMs VideoView "VideoView" "position-ms" set_position_ms;
        VideoDurationMs VideoView "VideoView" "duration-ms" set_duration_ms;
        AudioPositionMs AudioView "AudioView" "position-ms" set_position_ms;
        AudioDurationMs AudioView "AudioView" "duration-ms" set_duration_ms;
    } };
}
pub(crate) use int_props;

macro_rules! enum_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        set_login_phase(LoginStep) LoginView "LoginView" "step" set_step;
        set_login_activity(LoginActivity) LoginView "LoginView" "activity" set_activity;
        set_login_method_kind(LoginMethod) LoginView "LoginView" "method" set_method;
        set_connection_state(&ConnectionStatus) SessionView "SessionView" "connection-status" set_connection_status;
        set_timeline_state(TimelineStatus) RoomView "RoomView" "timeline-status" set_timeline_status;
        set_toast_message(UserMessageKind) RoomView "RoomView" "toast-message" set_toast_message;
        set_verification_phase(VerifyStep) VerificationView "VerificationView" "step" set_step;
        set_verification_activity(VerificationActivity) VerificationView "VerificationView" "activity" set_activity;
        set_verification_error(UserMessageKind) VerificationView "VerificationView" "error" set_error;
        set_attachment_kind(AttachmentKind) AttachmentView "AttachmentView" "kind" set_kind;
        set_attachment_error(UserMessageKind) AttachmentView "AttachmentView" "error" set_error;
        set_video_error(UserMessageKind) VideoView "VideoView" "error" set_error;
        set_audio_kind(AudioKind) AudioView "AudioView" "kind" set_kind;
    } };
}
pub(crate) use enum_props;

macro_rules! model_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        timeline Message SpliceModel RoomView "RoomView" "timeline" set_timeline;
        rooms Room VecModel DirectoryView "DirectoryView" "rooms" set_rooms;
        spaces Space VecModel DirectoryView "DirectoryView" "spaces" set_spaces;
        subspaces Space VecModel DirectoryView "DirectoryView" "subspaces" set_subspaces;
        sticker_rows StickerRow SpliceModel StickerView "StickerView" "rows" set_rows;
        sticker_packs StickerPack VecModel StickerView "StickerView" "packs" set_packs;
    } };
}
pub(crate) use model_props;

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
        AttachmentUnreadable      AttachmentUnreadable    "attachment-unreadable";
        AttachmentTooLarge        AttachmentTooLarge      "attachment-too-large";
        SendAttachmentFailed      SendAttachmentFailed    "send-attachment-failed";
        VideoPlaybackFailed       VideoPlaybackFailed     "video-playback-failed";
        AudioPlaybackFailed       AudioPlaybackFailed     "audio-playback-failed";
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

macro_rules! send_states {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Sent          Sent      "sent";
        Sending       Sending   "sending";
        Uploading{..} Uploading "uploading";
        Failed        Failed    "failed";
    } };
}
pub(crate) use send_states;

macro_rules! reaction_sends {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Sent    Sent    "sent";
        Sending Sending "sending";
        Failed  Failed  "failed";
    } };
}
pub(crate) use reaction_sends;

macro_rules! attachment_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        File  File  "file";
        Image Image "image";
        Video Video "video";
        Audio Audio "audio";
    } };
}
pub(crate) use attachment_kinds;

macro_rules! media_states {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Idle   Idle   "idle";
        Ready  Ready  "ready";
        Failed Failed "failed";
    } };
}
pub(crate) use media_states;

macro_rules! media_failures {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None       None       "none";
        NoSource   NoSource   "no-source";
        Download   Download   "download";
        TooLarge   TooLarge   "too-large";
        Storage    Storage    "storage";
        Unreadable Unreadable "unreadable";
    } };
}
pub(crate) use media_failures;

macro_rules! message_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Text        Text        "text";
        Notice      Notice      "notice";
        Emote       Emote       "emote";
        Image       Image       "image";
        Video       Video       "video";
        Audio       Audio       "audio";
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
        Voice     Voice     "voice";
        File      File      "file";
        Location  Location  "location";
        Encrypted Encrypted "encrypted";
        Sticker   Sticker   "sticker";
    } };
}
pub(crate) use preview_kinds;

macro_rules! audio_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Voice Voice "voice";
        Track Track "track";
    } };
}
pub(crate) use audio_kinds;

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

macro_rules! message_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        unique_id UNIQUE_ID "unique-id" text;
        local_id LOCAL_ID "local-id" text;
        sender SENDER "sender" text;
        sender_id SENDER_ID "sender-id" text;
        pronouns PRONOUNS "pronouns" list;
        body BODY "body" text;
        styled STYLED "styled" styled;
        has_links HAS_LINKS "has-links" flag;
        timestamp TIMESTAMP "timestamp" text;
        message_type MESSAGE_TYPE "message-type" enumk;
        preview_kind PREVIEW_KIND "preview-kind" enumk;
        unsupported_kind UNSUPPORTED_KIND "unsupported-kind" text;
        event_id EVENT_ID "event-id" text;
        sender_initial SENDER_INITIAL "sender-initial" text;
        color_index COLOR_INDEX "color-index" int;
        is_own IS_OWN "is-own" flag;
        edited EDITED "edited" flag;
        first_unread FIRST_UNREAD "first-unread" flag;
        send_state SEND_STATE "send-state" enumk;
        send_progress SEND_PROGRESS "send-progress" ratio;
        has_reply HAS_REPLY "has-reply" flag;
        reply_event_id REPLY_EVENT_ID "reply-event-id" text;
        reply_sender REPLY_SENDER "reply-sender" text;
        reply_kind REPLY_KIND "reply-kind" enumk;
        reply_body REPLY_BODY "reply-body" text;
        service_kind SERVICE_KIND "service-kind" enumk;
        service_target SERVICE_TARGET "service-target" text;
        media_state MEDIA_STATE "media-state" enumk;
        media_failure MEDIA_FAILURE "media-failure" enumk;
        image_mimetype IMAGE_MIMETYPE "image-mimetype" text;
        image_extension IMAGE_EXTENSION "image-extension" text;
        image_width IMAGE_WIDTH "image-width" int;
        image_height IMAGE_HEIGHT "image-height" int;
        duration DURATION "duration" text;
        filename FILENAME "filename" text;
        size SIZE "size" text;
        audio_kind AUDIO_KIND "audio-kind" enumk;
        waveform WAVEFORM "waveform" floats;
        has_avatar HAS_AVATAR "has-avatar" flag;
        needs_media NEEDS_MEDIA "needs-media" flag;
        thumbnail THUMBNAIL "thumbnail" image;
        avatar AVATAR "avatar" image;
        reactions REACTIONS "reactions" structs;
        all_reactions ALL_REACTIONS "all-reactions" structs;
    } };
}
pub(crate) use message_fields;

macro_rules! reaction_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        key KEY "key" text;
        label LABEL "label" text;
        count COUNT "count" int;
        mine MINE "mine" flag;
        send SEND "send" enumk;
        overflow OVERFLOW "overflow" flag;
        reactors REACTORS "reactors" text;
        hidden_reactors HIDDEN_REACTORS "hidden-reactors" int;
        avatars AVATARS "avatars" structs;
    } };
}

macro_rules! reactor_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        user_id USER_ID "user-id" text;
        initial INITIAL "initial" text;
        color_index COLOR_INDEX "color-index" int;
        avatar AVATAR "avatar" image;
        has_avatar HAS_AVATAR "has-avatar" flag;
    } };
}
pub(crate) use reaction_fields;
pub(crate) use reactor_fields;

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
pub(crate) use room_fields;

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
pub(crate) use space_fields;

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
pub(crate) use sticker_cell_fields;

macro_rules! sticker_pack_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        id ID "id" text;
        title TITLE "title" text;
        header_row HEADER_ROW "header-row" int;
        icon ICON "icon" image;
        has_icon HAS_ICON "has-icon" flag;
    } };
}
pub(crate) use sticker_pack_fields;

macro_rules! sticker_row_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        title TITLE "title" text;
        is_header IS_HEADER "is-header" flag;
        cells CELLS "cells" structs;
    } };
}
pub(crate) use sticker_row_fields;
