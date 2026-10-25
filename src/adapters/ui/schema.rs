#[cfg(feature = "demo")]
macro_rules! enum_names {
    (val $fn:ident $src:ident; $($rows:tt)*) => {
        pub fn $fn(value: $src) -> &'static str { enum_names!(@source value, $src, $($rows)*) }
    };
    (ref $fn:ident $src:ident; $($rows:tt)*) => {
        pub fn $fn(value: &$src) -> &'static str { enum_names!(@source value, $src, $($rows)*) }
    };
    (@source $v:ident, $src:ident,
        $($rust:ident $(($($p:tt)*))? $({$($b:tt)*})? $ui:ident $lit:literal;)*) => {
        match $v { $($src::$rust $(($($p)*))? $({$($b)*})? => $lit,)* }
    };
}
#[cfg(feature = "demo")]
pub(crate) use enum_names;

macro_rules! string_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        ResolvedHomeserver LoginView "LoginView" "resolved-homeserver" set_resolved_homeserver get_resolved_homeserver;
        UserId SessionView "SessionView" "user-id" set_user_id get_user_id;
        UserInitial SessionView "SessionView" "user-initial" set_user_initial get_user_initial;
        ToastDetail RoomView "RoomView" "toast-detail" set_toast_detail get_toast_detail;
        VerificationSender VerificationView "VerificationView" "sender" set_sender get_sender;
        VerificationErrorDetail VerificationView "VerificationView" "error-detail" set_error_detail get_error_detail;
        EncryptionRecoveryKey EncryptionView "EncryptionView" "recovery-key" set_recovery_key get_recovery_key;
        EncryptionErrorDetail EncryptionView "EncryptionView" "error-detail" set_error_detail get_error_detail;
        EncryptionKeyFile EncryptionView "EncryptionView" "key-file" set_key_file get_key_file;
        EncryptionAuthErrorDetail EncryptionView "EncryptionView" "auth-error-detail" set_auth_error_detail get_auth_error_detail;
        AccountThisName AccountView "AccountView" "this-name" set_this_name get_this_name;
        AccountThisDeviceId AccountView "AccountView" "this-device-id" set_this_device_id get_this_device_id;
        AccountThisKey AccountView "AccountView" "this-key" set_this_key get_this_key;
        AccountBusySession AccountView "AccountView" "busy-session" set_busy_session get_busy_session;
        AccountErrorDetail AccountView "AccountView" "error-detail" set_error_detail get_error_detail;
        AccountNoticeDetail AccountView "AccountView" "notice-detail" set_notice_detail get_notice_detail;
        AccountAuthErrorDetail AccountView "AccountView" "auth-error-detail" set_auth_error_detail get_auth_error_detail;
        AccountDefaultRoomVersion AccountView "AccountView" "default-room-version" set_default_room_version get_default_room_version;
        AccountAuthSubject AccountView "AccountView" "auth-subject" set_auth_subject get_auth_subject;
        SelectedRoomName RoomView "RoomView" "selected-room-name" set_selected_room_name get_selected_room_name;
        SelectedRoomInitial RoomView "RoomView" "selected-room-initial" set_selected_room_initial get_selected_room_initial;
        FocusEventId RoomView "RoomView" "focus-event-id" set_focus_event_id get_focus_event_id;
        PinnedEventId RoomView "RoomView" "pinned-event-id" set_pinned_event_id get_pinned_event_id;
        PinnedBody RoomView "RoomView" "pinned-body" set_pinned_body get_pinned_body;
        SelectedRoomId DirectoryView "DirectoryView" "selected-room-id" set_selected_room_id get_selected_room_id;
        SelectedSpaceId DirectoryView "DirectoryView" "selected-space-id" set_selected_space_id get_selected_space_id;
        SelectedSpaceName DirectoryView "DirectoryView" "selected-space-name" set_selected_space_name get_selected_space_name;
        SelectedSubspaceId DirectoryView "DirectoryView" "selected-subspace-id" set_selected_subspace_id get_selected_subspace_id;
        ListedSpaceName DirectoryView "DirectoryView" "listed-space-name" set_listed_space_name get_listed_space_name;
        ListedSpaceId DirectoryView "DirectoryView" "listed-space-id" set_listed_space_id get_listed_space_id;
        AttachmentFilename AttachmentView "AttachmentView" "filename" set_filename get_filename;
        AttachmentMimetype AttachmentView "AttachmentView" "mimetype" set_mimetype get_mimetype;
        AttachmentExtension AttachmentView "AttachmentView" "extension" set_extension get_extension;
        AttachmentSize AttachmentView "AttachmentView" "size" set_size get_size;
        AttachmentErrorDetail AttachmentView "AttachmentView" "error-detail" set_error_detail get_error_detail;
        AudioEventId AudioView "AudioView" "event-id" set_event_id get_event_id;
        AudioRoomId AudioView "AudioView" "room-id" set_room_id get_room_id;
        AudioSender AudioView "AudioView" "sender" set_sender get_sender;
        AudioTitle AudioView "AudioView" "title" set_title get_title;
        AttachmentDuration AttachmentView "AttachmentView" "duration" set_duration get_duration;
        UnsentRoomId UnsentView "UnsentView" "room-id" set_room_id get_room_id;
        UnsentBody UnsentView "UnsentView" "body" set_body get_body;
        UnsentReplyEventId UnsentView "UnsentView" "reply-event-id" set_reply_event_id get_reply_event_id;
        UnsentReplySender UnsentView "UnsentView" "reply-sender" set_reply_sender get_reply_sender;
        UnsentReplyPreview UnsentView "UnsentView" "reply-preview" set_reply_preview get_reply_preview;
        UnsentEditEventId UnsentView "UnsentView" "edit-event-id" set_edit_event_id get_edit_event_id;
        UnsentEditLocalId UnsentView "UnsentView" "edit-local-id" set_edit_local_id get_edit_local_id;
        UnsentEditPreview UnsentView "UnsentView" "edit-preview" set_edit_preview get_edit_preview;
        IdentityAlertUserId IdentityAlertView "IdentityAlertView" "user-id" set_user_id get_user_id;
        IdentityAlertName IdentityAlertView "IdentityAlertView" "name" set_name get_name;
        UpgradeRoomId UpgradeView "UpgradeView" "room-id" set_room_id get_room_id;
        UpgradeSuccessorId UpgradeView "UpgradeView" "successor-id" set_successor_id get_successor_id;
        UpgradePredecessorId UpgradeView "UpgradeView" "predecessor-id" set_predecessor_id get_predecessor_id;
        TypingRoomId TypingView "TypingView" "room-id" set_room_id get_room_id;
        TypingFirst TypingView "TypingView" "first" set_first get_first;
        TypingSecond TypingView "TypingView" "second" set_second get_second;
        PresenceRoomId PresenceView "PresenceView" "room-id" set_room_id get_room_id;
        PresenceUserId PresenceView "PresenceView" "user-id" set_user_id get_user_id;
        PresenceStatus PresenceView "PresenceView" "status" set_status get_status;
        RingRoomId CallView "CallView" "ring-room-id" set_ring_room_id get_ring_room_id;
        RingRoomName CallView "CallView" "ring-room-name" set_ring_room_name get_ring_room_name;
        RingCaller CallView "CallView" "ring-caller" set_ring_caller get_ring_caller;
        RingInitial CallView "CallView" "ring-initial" set_ring_initial get_ring_initial;
        HereRoomId CallView "CallView" "here-room-id" set_here_room_id get_here_room_id;
        OfferRoomId CallView "CallView" "offer-room-id" set_offer_room_id get_offer_room_id;
        SheetRoomId CallView "CallView" "sheet-room-id" set_sheet_room_id get_sheet_room_id;
        SheetRoomName CallView "CallView" "sheet-room-name" set_sheet_room_name get_sheet_room_name;
        SheetLeavingId CallView "CallView" "sheet-leaving-id" set_sheet_leaving_id get_sheet_leaving_id;
        SheetLeavingName CallView "CallView" "sheet-leaving-name" set_sheet_leaving_name get_sheet_leaving_name;
        SheetPreviewFeed CallView "CallView" "sheet-preview-feed" set_sheet_preview_feed get_sheet_preview_feed;
        SheetPreviewCall CallView "CallView" "sheet-preview-call" set_sheet_preview_call get_sheet_preview_call;
        ActiveRoomId CallView "CallView" "active-room-id" set_active_room_id get_active_room_id;
        ActiveRoomName CallView "CallView" "active-room-name" set_active_room_name get_active_room_name;
        CallElapsed CallView "CallView" "elapsed" set_elapsed get_elapsed;
        NoticeName CallView "CallView" "notice-name" set_notice_name get_notice_name;
        FailureRoomId CallView "CallView" "failure-room-id" set_failure_room_id get_failure_room_id;
        FailureDetail CallView "CallView" "failure-detail" set_failure_detail get_failure_detail;
        FirstSharer CallView "CallView" "first-sharer" set_first_sharer get_first_sharer;
        OfferedShare CallView "CallView" "offered-share" set_offered_share get_offered_share;
        StartedShare CallView "CallView" "started-share" set_started_share get_started_share;
        EndedShare CallView "CallView" "ended-share" set_ended_share get_ended_share;
        PeopleFirst CallView "CallView" "people-first" set_people_first get_people_first;
        PeopleSecond CallView "CallView" "people-second" set_people_second get_people_second;
        SheetPeopleFirst CallView "CallView" "sheet-people-first" set_sheet_people_first get_sheet_people_first;
        SheetPeopleSecond CallView "CallView" "sheet-people-second" set_sheet_people_second get_sheet_people_second;
        BridgesFirst CallView "CallView" "bridges-first" set_bridges_first get_bridges_first;
        BridgesSecond CallView "CallView" "bridges-second" set_bridges_second get_bridges_second;
        SheetBridgesFirst CallView "CallView" "sheet-bridges-first" set_sheet_bridges_first get_sheet_bridges_first;
        SheetBridgesSecond CallView "CallView" "sheet-bridges-second" set_sheet_bridges_second get_sheet_bridges_second;
        DeviceNewsName CallView "CallView" "device-news-name" set_device_news_name get_device_news_name;
        SpaceIndexName SpaceIndexView "SpaceIndexView" "space-name" set_space_name get_space_name;
        RoomInfoRoomId RoomInfoView "RoomInfoView" "room-id" set_room_id get_room_id;
        RoomInfoName RoomInfoView "RoomInfoView" "name" set_name get_name;
        RoomInfoInitial RoomInfoView "RoomInfoView" "initial" set_initial get_initial;
        RoomInfoTopic RoomInfoView "RoomInfoView" "topic" set_topic get_topic;
        RoomInfoAlias RoomInfoView "RoomInfoView" "alias" set_alias get_alias;
        RoomInfoJoinedOn RoomInfoView "RoomInfoView" "joined-on" set_joined_on get_joined_on;
        RoomInfoLink RoomInfoView "RoomInfoView" "link" set_link get_link;
        MessageLink RoomView "RoomView" "message-link" set_message_link get_message_link;
        SourceEventId SourceView "SourceView" "event-id" set_event_id get_event_id;
        SourceJson SourceView "SourceView" "json" set_json get_json;
        SourceEditJson SourceView "SourceView" "edit-json" set_edit_json get_edit_json;
        SourceEncryptionJson SourceView "SourceView" "encryption-json" set_encryption_json get_encryption_json;
        ReadersSender ReadersView "ReadersView" "sender" set_sender get_sender;
        ReadersSentAt ReadersView "ReadersView" "sent-at" set_sent_at get_sent_at;
        ReadersBody ReadersView "ReadersView" "body" set_body get_body;
        ReadersServiceTarget ReadersView "ReadersView" "service-target" set_service_target get_service_target;
        MentionsRoomId MentionsView "MentionsView" "room-id" set_room_id get_room_id;
        RoomInfoErrorDetail RoomInfoView "RoomInfoView" "error-detail" set_error_detail get_error_detail;
        RoomMenuRoomId RoomMenuView "RoomMenuView" "room-id" set_room_id get_room_id;
        RoomMenuName RoomMenuView "RoomMenuView" "name" set_name get_name;
        RoomLink RoomMenuView "RoomMenuView" "link" set_link get_link;
        UserInfoUserId UserInfoView "UserInfoView" "user-id" set_user_id get_user_id;
        UserInfoName UserInfoView "UserInfoView" "name" set_name get_name;
        UserInfoInitial UserInfoView "UserInfoView" "initial" set_initial get_initial;
        UserInfoPronouns UserInfoView "UserInfoView" "pronouns" set_pronouns get_pronouns;
        UserInfoLink UserInfoView "UserInfoView" "link" set_link get_link;
        UserInfoErrorDetail UserInfoView "UserInfoView" "error-detail" set_error_detail get_error_detail;
        InviteRoomId InviteView "InviteView" "room-id" set_room_id get_room_id;
        InviteName InviteView "InviteView" "name" set_name get_name;
        InviteInitial InviteView "InviteView" "initial" set_initial get_initial;
        InviteTopic InviteView "InviteView" "topic" set_topic get_topic;
        InviteInviter InviteView "InviteView" "inviter" set_inviter get_inviter;
        InviteErrorDetail InviteView "InviteView" "error-detail" set_error_detail get_error_detail;
        RemovalRoomId RemovalView "RemovalView" "room-id" set_room_id get_room_id;
        RemovalName RemovalView "RemovalView" "name" set_name get_name;
        RemovalInitial RemovalView "RemovalView" "initial" set_initial get_initial;
        RemovalBy RemovalView "RemovalView" "by" set_by get_by;
        RemovalReason RemovalView "RemovalView" "reason" set_reason get_reason;
        RemovalErrorDetail RemovalView "RemovalView" "error-detail" set_error_detail get_error_detail;
        LeaveSpaceId LeaveSpaceView "LeaveSpaceView" "space-id" set_space_id get_space_id;
        LeaveSpaceName LeaveSpaceView "LeaveSpaceView" "name" set_name get_name;
        LeaveSpaceErrorDetail LeaveSpaceView "LeaveSpaceView" "error-detail" set_error_detail get_error_detail;
        RoomPreviewAddress RoomPreviewView "RoomPreviewView" "address" set_address get_address;
        RoomPreviewRoomId RoomPreviewView "RoomPreviewView" "room-id" set_room_id get_room_id;
        RoomPreviewName RoomPreviewView "RoomPreviewView" "name" set_name get_name;
        RoomPreviewInitial RoomPreviewView "RoomPreviewView" "initial" set_initial get_initial;
        RoomPreviewAlias RoomPreviewView "RoomPreviewView" "alias" set_alias get_alias;
        RoomPreviewTopic RoomPreviewView "RoomPreviewView" "topic" set_topic get_topic;
        RoomPreviewErrorDetail RoomPreviewView "RoomPreviewView" "error-detail" set_error_detail get_error_detail;
        CreateRoomParentName CreateRoomView "CreateRoomView" "parent-name" set_parent_name get_parent_name;
        CreateRoomServerName CreateRoomView "CreateRoomView" "server-name" set_server_name get_server_name;
        CreateRoomErrorDetail CreateRoomView "CreateRoomView" "error-detail" set_error_detail get_error_detail;
        InvitePeopleRoomId InvitePeopleView "InvitePeopleView" "room-id" set_room_id get_room_id;
        InvitePeopleRoomName InvitePeopleView "InvitePeopleView" "room-name" set_room_name get_room_name;
        InvitePeopleErrorDetail InvitePeopleView "InvitePeopleView" "error-detail" set_error_detail get_error_detail;
        RoomLogRoomId RoomLogView "RoomLogView" "room-id" set_room_id get_room_id;
        RoomLogName RoomLogView "RoomLogView" "name" set_name get_name;
        SpoilerLabel SpoilerCover "SpoilerCover" "label" set_label get_label;
        SpoilerReasonLabel SpoilerCover "SpoilerCover" "reason-label" set_reason_label get_reason_label;
        SpoilerReasonSlot SpoilerCover "SpoilerCover" "reason-slot" set_reason_slot get_reason_slot;
        NotificationOpenLabel NotificationText "NotificationText" "open-label" set_open_label get_open_label;
        NotificationDeclineLabel NotificationText "NotificationText" "decline-label" set_decline_label get_decline_label;
        NotificationAnswerLabel NotificationText "NotificationText" "answer-label" set_answer_label get_answer_label;
        NotificationAnswerWithoutCameraLabel NotificationText "NotificationText" "answer-without-camera-label" set_answer_without_camera_label get_answer_without_camera_label;
        CallRoom WindowView "WindowView" "call-room" set_call_room get_call_room;
        NotificationRoom WindowView "WindowView" "notification-room" set_notification_room get_notification_room;
        ExternalLink WindowView "WindowView" "external-link" set_external_link get_external_link;
        #[cfg(feature = "demo")] WindowTitleText WindowTitle "WindowTitle" "text" set_text get_text;
        #[cfg(feature = "demo")] RoomMenuKey RoomMenu "RoomMenu" "key" set_key get_key;
    } };
}
pub(crate) use string_props;

macro_rules! simple_callbacks {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        on_retry_restore "retry-restore" retry_restore plain RestoreSession;
        on_check_server "check-server" check_server pass CheckServer;
        on_login_password "login-password" login_password
            request(username "username" text, password "password" text) LoginPassword;
        on_login_oauth "login-oauth" login_oauth plain LoginOAuth;
        on_cancel_oauth "cancel-oauth" cancel_oauth plain CancelOAuth;
        on_back_to_homeserver "back-to-homeserver" back_to_homeserver plain BackToHomeserver;
        on_reauth_password "reauth-password" reauth_password pass ReauthPassword;
        on_reauth_oauth "reauth-oauth" reauth_oauth plain ReauthOAuth;
        on_logout "logout" logout plain Logout;
        on_dismiss_toast "dismiss-toast" dismiss_toast plain DismissToast;
        on_accept_verification "accept-verification" accept_verification plain AcceptVerification;
        on_confirm_verification "confirm-verification" confirm_verification plain ConfirmVerification;
        on_reject_verification "reject-verification" reject_verification plain RejectVerification;
        on_dismiss_verification "dismiss-verification" dismiss_verification plain DismissVerification;
        on_open_encryption "open-encryption" open_encryption plain OpenEncryption;
        on_close_encryption "close-encryption" close_encryption plain CloseEncryption;
        on_show_encryption_overview "show-encryption-overview" show_encryption_overview plain
            ShowEncryptionOverview;
        on_enter_recovery_key "enter-recovery-key" enter_recovery_key plain EnterRecoveryKey;
        on_use_recovery_key "use-recovery-key" use_recovery_key manual_string UseRecoveryKey;
        on_verify_with_other_session "verify-with-other-session" verify_with_other_session plain
            VerifyWithOtherSession;
        on_set_up_recovery "set-up-recovery" set_up_recovery plain SetUpRecovery;
        on_change_recovery_key "change-recovery-key" change_recovery_key plain ChangeRecoveryKey;
        on_retry_encryption_setup "retry-encryption-setup" retry_encryption_setup plain
            RetryEncryptionSetup;
        on_start_identity_reset "start-identity-reset" start_identity_reset plain
            StartIdentityReset;
        on_reset_identity "reset-identity" reset_identity plain ResetIdentity;
        on_start_key_export "start-key-export" start_key_export plain StartKeyExport;
        on_export_room_keys "export-room-keys" export_room_keys manual_string ExportRoomKeys;
        on_start_key_import "start-key-import" start_key_import plain StartKeyImport;
        on_import_room_keys "import-room-keys" import_room_keys manual_string ImportRoomKeys;
        on_open_account "open-account" open_account plain OpenAccount;
        on_close_account "close-account" close_account plain CloseAccount;
        on_show_account_overview "show-account-overview" show_account_overview plain
            ShowAccountOverview;
        on_rename_session "rename-session" rename_session
            request(device_id "device-id" text, name "name" text) RenameSession;
        on_sign_out_session "sign-out-session" sign_out_session pass SignOutSession;
        on_open_account_page "open-account-page" open_account_page manual OpenAccountPage;
        on_open_sessions_page "open-sessions-page" open_sessions_page manual OpenAccountPage;
        on_start_password_change "start-password-change" start_password_change plain
            StartPasswordChange;
        on_change_password "change-password" change_password
            request(password "password" text, sign_out_others "sign-out-others" flag) ChangePassword;
        on_start_deactivation "start-deactivation" start_deactivation plain StartDeactivation;
        on_deactivate_account "deactivate-account" deactivate_account
            request(erase "erase" flag) DeactivateAccount;
        on_answer_auth_password "answer-auth-password" answer_auth_password manual_string
            AnswerAuthPassword;
        on_continue_auth "continue-auth" continue_auth plain ContinueAuth;
        on_reopen_auth_page "reopen-auth-page" reopen_auth_page plain ReopenAuthPage;
        on_cancel_auth "cancel-auth" cancel_auth plain CancelAuth;
        on_confirm_logout "confirm-logout" confirm_logout plain ConfirmLogout;
        on_cancel_logout "cancel-logout" cancel_logout plain CancelLogout;
        on_retry_timeline "retry-timeline" retry_timeline plain RetryTimeline;
        on_decline_call "decline-call" decline_call room DeclineCall;
        on_silence_ring "silence-ring" silence_ring plain SilenceRing;
        on_open_call_sheet "open-call-sheet" open_call_sheet
            request(room_id "room-id" text, video "video" flag) OpenCallSheet;
        on_close_call_sheet "close-call-sheet" close_call_sheet plain CloseCallSheet;
        on_sheet_camera "sheet-camera" sheet_camera request(on "on" flag) SheetCamera;
        on_test_call_device "test-call-device" test_call_device
            request(kind "kind" variant, on "on" flag) TestCallDevice;
        on_start_call "start-call" start_call
            request(room_id "room-id" text, video "video" flag, microphone "microphone" flag,
                camera "camera" flag, leaving "leaving" text) StartCall;
        on_join_call "join-call" join_call
            request(room_id "room-id" text, microphone "microphone" flag, camera "camera" flag,
                leaving "leaving" text) JoinCall;
        on_answer_call "answer-call" answer_call
            request(room_id "room-id" text, camera "camera" flag, leaving "leaving" text)
            AnswerCall;
        on_leave_call "leave-call" leave_call plain LeaveCall;
        on_retry_failed_call "retry-failed-call" retry_failed_call plain RetryFailedCall;
        on_dismiss_failed_call "dismiss-failed-call" dismiss_failed_call plain DismissFailedCall;
        on_announce_call_again "announce-call-again" announce_call_again plain AnnounceCallAgain;
        on_set_microphone "set-microphone" set_microphone request(on "on" flag) SetMicrophone;
        on_set_camera "set-camera" set_camera request(on "on" flag) SetCamera;
        on_share_screen "share-screen" share_screen request(on "on" flag) ShareScreen;
        on_change_screen_share "change-screen-share" change_screen_share plain ChangeScreenShare;
        on_use_call_device "use-call-device" use_call_device
            request(kind "kind" variant, key "key" text) UseCallDevice;
        on_refresh_call_devices "refresh-call-devices" refresh_call_devices plain
            RefreshCallDevices;
        on_focus_call_tile "focus-call-tile" focus_call_tile pass FocusCallTile;
        on_unpin_call_tile "unpin-call-tile" unpin_call_tile plain UnpinCallTile;
        on_ring_devices "ring-devices" ring_devices request(open "open" flag) RingDevices;
        on_show_call_gallery "show-call-gallery" show_call_gallery plain ShowCallGallery;
        on_view_call_share "view-call-share" view_call_share plain ViewCallShare;
        on_show_self_view "show-self-view" show_self_view request(on "on" flag) ShowSelfView;
        on_show_share_preview "show-share-preview" show_share_preview request(on "on" flag)
            ShowSharePreview;
        on_follow_speaker "follow-speaker" follow_speaker request(on "on" flag) FollowSpeaker;
        on_select_room "select-room" select_room room SelectRoom;
        on_open_notification "open-notification" open_notification room OpenNotification;
        on_select_space "select-space" select_space opt_room SelectSpace;
        on_select_direct "select-direct" select_direct plain SelectDirect;
        on_select_subspace "select-subspace" select_subspace opt_room SelectSubspace;
        on_filter_rooms "filter-rooms" filter_rooms pass FilterRooms;
        on_fold_invites "fold-invites" fold_invites plain FoldInvites;
        on_unfold_invites "unfold-invites" unfold_invites plain UnfoldInvites;
        on_open_space "open-space" open_space room OpenSpace;
        on_open_space_index "open-space-index" open_space_index plain OpenSpaceIndex;
        on_close_space_index "close-space-index" close_space_index plain CloseSpaceIndex;
        on_page_space_index "page-space-index" page_space_index plain PageSpaceIndex;
        on_retry_space_index "retry-space-index" retry_space_index plain RetrySpaceIndex;
        on_join_space_child "join-space-child" join_space_child room JoinSpaceChild;
        on_open_space_child "open-space-child" open_space_child room OpenSpaceChild;
        on_open_invite "open-invite" open_invite room OpenInvite;
        on_close_invite "close-invite" close_invite plain CloseInvite;
        on_accept_invite "accept-invite" accept_invite room AcceptInvite;
        on_decline_invite "decline-invite" decline_invite room DeclineInvite;
        on_open_removal "open-removal" open_removal room OpenRemoval;
        on_close_removal "close-removal" close_removal plain CloseRemoval;
        on_forget_room "forget-room" forget_room room ForgetRoom;
        on_join_successor "join-successor" join_successor room JoinSuccessor;
        on_join_predecessor "join-predecessor" join_predecessor room JoinPredecessor;
        on_open_room_finder "open-room-finder" open_room_finder plain OpenRoomFinder;
        on_find_room "find-room" find_room manual_string FindRoom;
        on_join_previewed_room "join-previewed-room" join_previewed_room pass JoinPreviewedRoom;
        on_retry_room_preview "retry-room-preview" retry_room_preview plain RetryRoomPreview;
        on_close_room_preview "close-room-preview" close_room_preview plain CloseRoomPreview;
        on_open_create_room "open-create-room" open_create_room plain OpenCreateRoom;
        on_open_create_space "open-create-space" open_create_space plain OpenCreateSpace;
        on_close_create_room "close-create-room" close_create_room plain CloseCreateRoom;
        on_create_room "create-room" create_room
            request(name "name" text, topic "topic" text, address "address" text,
                kind "kind" variant, access "access" variant, encrypted "encrypted" flag,
                in_space "in-space" flag) CreateRoom;
        on_open_invite_people "open-invite-people" open_invite_people room OpenInvitePeople;
        on_close_invite_people "close-invite-people" close_invite_people plain CloseInvitePeople;
        on_search_people "search-people" search_people pass SearchPeople;
        on_retry_people_search "retry-people-search" retry_people_search plain RetryPeopleSearch;
        on_pick_invitee "pick-invitee" pick_invitee user PickInvitee;
        on_unpick_invitee "unpick-invitee" unpick_invitee user UnpickInvitee;
        on_send_invites "send-invites" send_invites pass SendInvites;
        on_open_room_info "open-room-info" open_room_info room OpenRoomInfo;
        on_close_room_info "close-room-info" close_room_info plain CloseRoomInfo;
        on_show_room_info_pane "show-room-info-pane" show_room_info_pane plain ShowRoomInfoPane;
        on_hide_room_info_pane "hide-room-info-pane" hide_room_info_pane plain HideRoomInfoPane;
        on_page_room_members "page-room-members" page_room_members plain PageRoomMembers;
        on_retry_room_members "retry-room-members" retry_room_members plain RetryRoomMembers;
        on_filter_room_members "filter-room-members" filter_room_members pass FilterRoomMembers;
        on_leave_room "leave-room" leave_room room LeaveRoom;
        on_open_leave_space "open-leave-space" open_leave_space room OpenLeaveSpace;
        on_close_leave_space "close-leave-space" close_leave_space plain CloseLeaveSpace;
        on_leave_space "leave-space" leave_space
            request(space_id "space-id" text, with_rooms "with-rooms" flag) LeaveSpace;
        on_open_room_menu "open-room-menu" open_room_menu room OpenRoomMenu;
        on_close_room_menu "close-room-menu" close_room_menu plain CloseRoomMenu;
        on_mark_room_read "mark-room-read" mark_room_read room MarkRoomRead;
        on_copy_room_link "copy-room-link" copy_room_link room CopyRoomLink;
        on_open_room_log "open-room-log" open_room_log room OpenRoomLog;
        on_close_room_log "close-room-log" close_room_log plain CloseRoomLog;
        on_open_user_info "open-user-info" open_user_info user OpenUserInfo;
        on_close_user_info "close-user-info" close_user_info plain CloseUserInfo;
        on_retry_user_info "retry-user-info" retry_user_info plain RetryUserInfo;
        on_message_user "message-user" message_user user MessageUser;
        on_verify_user "verify-user" verify_user user VerifyUser;
        on_ignore_user "ignore-user" ignore_user user IgnoreUser;
        on_unignore_user "unignore-user" unignore_user user UnignoreUser;
        on_kick_user "kick-user" kick_user
            request(user_id "user-id" text, reason "reason" text) KickUser;
        on_ban_user "ban-user" ban_user
            request(user_id "user-id" text, reason "reason" text) BanUser;
        on_unban_user "unban-user" unban_user user UnbanUser;
        on_accept_identity "accept-identity" accept_identity user AcceptIdentity;
        on_withdraw_verification "withdraw-verification" withdraw_verification user WithdrawVerification;
        on_acknowledge_identity_reset "acknowledge-identity-reset" acknowledge_identity_reset plain
            AcknowledgeIdentityReset;
        on_paginate_backwards "paginate-backwards" paginate_backwards room_key PaginateBackwards;
        on_paginate_forwards "paginate-forwards" paginate_forwards room_key PaginateForwards;
        on_jump_to_latest "jump-to-latest" jump_to_latest room_key JumpToLatest;
        on_send_message "send-message" send_message
            verdict(room_id "room-id" text, body "body" text, reply_to "reply-to" text,
                reply_sender "reply-sender" text, reply_preview "reply-preview" text) SendMessage;
        on_send_sticker "send-sticker" send_sticker
            request(room_id "room-id" text, pack_id "pack-id" text, shortcode "shortcode" text,
                reply_to "reply-to" text) SendSticker;
        on_send_attachment "send-attachment" send_attachment
            request(room_id "room-id" text, caption "caption" text, as_document "as-document" flag,
                reply_to "reply-to" text) SendAttachment;
        on_send_poll "send-poll" send_poll
            verdict(room_id "room-id" text, question "question" text, answers "answers" list,
                multiple "multiple" flag, hide_results "hide-results" flag) SendPoll;
        on_save_file "save-file" save_file
            request(event_id "event-id" text, filename "filename" text) SaveFile;
        on_edit_poll "edit-poll" edit_poll
            verdict(event_id "event-id" text, question "question" text, answers "answers" list,
                multiple "multiple" flag, hide_results "hide-results" flag) EditPoll;
        on_vote_poll "vote-poll" vote_poll
            request(event_id "event-id" text, answer_id "answer-id" text) VotePoll;
        on_end_poll "end-poll" end_poll manual_string EndPoll;
        on_open_media "open-media" open_media manual_string OpenMedia;
        on_open_video "open-video" open_video manual_string OpenVideo;
        on_close_video "close-video" close_video plain CloseVideo;
        on_play_audio "play-audio" play_audio manual_string PlayAudio;
        on_close_audio "close-audio" close_audio plain CloseAudio;
        on_open_link "open-link" open_link manual_string OpenLink;
        on_open_external_link "open-external-link" open_external_link manual_string OpenExternalLink;
        on_jump_to_event "jump-to-event" jump_to_event manual_string JumpToEvent;
        on_open_pinned "open-pinned" open_pinned manual_string OpenPinned;
        on_previous_pinned "previous-pinned" previous_pinned manual_string PreviousPinned;
        on_next_pinned "next-pinned" next_pinned manual_string NextPinned;
        on_copy_message_link "copy-message-link" copy_message_link manual_string CopyMessageLink;
        on_open_event_source "open-event-source" open_event_source manual_string OpenEventSource;
        on_close_event_source "close-event-source" close_event_source plain CloseEventSource;
        on_open_readers "open-readers" open_readers manual_string OpenReaders;
        on_close_readers "close-readers" close_readers plain CloseReaders;
        on_page_readers "page-readers" page_readers plain PageReaders;
        on_end_mentions "end-mentions" end_mentions plain EndMentions;
        on_pin_message "pin-message" pin_message manual_string PinMessage;
        on_unpin_message "unpin-message" unpin_message manual_string UnpinMessage;
        on_delete_message "delete-message" delete_message manual_string DeleteMessage;
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
        SessionRefused LoginView "LoginView" "session-refused" set_session_refused get_session_refused;
        RingVisible CallView "CallView" "ring-visible" set_ring_visible get_ring_visible;
        SheetVisible CallView "CallView" "sheet-visible" set_sheet_visible get_sheet_visible;
        SheetDirect CallView "CallView" "sheet-direct" set_sheet_direct get_sheet_direct;
        SheetVideo CallView "CallView" "sheet-video" set_sheet_video get_sheet_video;
        SheetMicrophone CallView "CallView" "sheet-microphone" set_sheet_microphone get_sheet_microphone;
        SheetCamera CallView "CallView" "sheet-camera" set_sheet_camera get_sheet_camera;
        SheetEnded CallView "CallView" "sheet-ended" set_sheet_ended get_sheet_ended;
        SheetTests CallView "CallView" "sheet-tests" set_sheet_tests get_sheet_tests;
        RingVideo CallView "CallView" "ring-video" set_ring_video get_ring_video;
        RingDirect CallView "CallView" "ring-direct" set_ring_direct get_ring_direct;
        RingAnswerable CallView "CallView" "ring-answerable" set_ring_answerable get_ring_answerable;
        RingQuiet CallView "CallView" "ring-quiet" set_ring_quiet get_ring_quiet;
        HereVideo CallView "CallView" "here-video" set_here_video get_here_video;
        HereJoined CallView "CallView" "here-joined" set_here_joined get_here_joined;
        ActiveVisible CallView "CallView" "active" set_active get_active;
        ActiveEncrypted CallView "CallView" "active-encrypted" set_active_encrypted get_active_encrypted;
        ActiveVideo CallView "CallView" "active-video" set_active_video get_active_video;
        VideoCall CallView "CallView" "video-call" set_video_call get_video_call;
        SelfView CallView "CallView" "self-view" set_self_view get_self_view;
        SharePreview CallView "CallView" "share-preview" set_share_preview get_share_preview;
        FollowsSpeaker CallView "CallView" "follows-speaker" set_follows_speaker get_follows_speaker;
        ViewChosen CallView "CallView" "view-chosen" set_view_chosen get_view_chosen;
        StageChosen CallView "CallView" "stage-chosen" set_stage_chosen get_stage_chosen;
        FailureRetry CallView "CallView" "failure-retry" set_failure_retry get_failure_retry;
        VerificationVisible VerificationView "VerificationView" "visible" set_visible get_visible;
        EncryptionVisible EncryptionView "EncryptionView" "visible" set_visible get_visible;
        EncryptionVerifiedSessions EncryptionView "EncryptionView" "verified-sessions" set_verified_sessions get_verified_sessions;
        EncryptionKeyStorageOff EncryptionView "EncryptionView" "key-storage-off" set_key_storage_off get_key_storage_off;
        AccountVisible AccountView "AccountView" "visible" set_visible get_visible;
        AccountSessionsKnown AccountView "AccountView" "sessions-known" set_sessions_known get_sessions_known;
        AccountFactsKnown AccountView "AccountView" "facts-known" set_facts_known get_facts_known;
        AccountOAuth AccountView "AccountView" "oauth" set_oauth get_oauth;
        AccountHasPages AccountView "AccountView" "has-pages" set_has_pages get_has_pages;
        AccountMayChangePassword AccountView "AccountView" "may-change-password" set_may_change_password get_may_change_password;
        AccountNameManaged AccountView "AccountView" "name-managed" set_name_managed get_name_managed;
        AccountAvatarManaged AccountView "AccountView" "avatar-managed" set_avatar_managed get_avatar_managed;
        AccountForgetsRooms AccountView "AccountView" "forgets-rooms" set_forgets_rooms get_forgets_rooms;
        LogoutVisible LogoutView "LogoutView" "visible" set_visible get_visible;
        LogoutKeysAtRisk LogoutView "LogoutView" "keys-at-risk" set_keys_at_risk get_keys_at_risk;
        DirectAlert DirectoryView "DirectoryView" "direct-alert" set_direct_alert get_direct_alert;
        DirectMention DirectoryView "DirectoryView" "direct-mention" set_direct_mention get_direct_mention;
        DirectHint DirectoryView "DirectoryView" "direct-hint" set_direct_hint get_direct_hint;
        InvitesPending DirectoryView "DirectoryView" "invites-pending" set_invites_pending get_invites_pending;
        ListedSpaceMayInvite DirectoryView "DirectoryView" "listed-space-may-invite" set_listed_space_may_invite get_listed_space_may_invite;
        ListedSpaceMayAddChildren DirectoryView "DirectoryView" "listed-space-may-add-children" set_listed_space_may_add_children get_listed_space_may_add_children;
        InviteVisible InviteView "InviteView" "visible" set_visible get_visible;
        RemovalVisible RemovalView "RemovalView" "visible" set_visible get_visible;
        RemovalForgetting RemovalView "RemovalView" "forgetting" set_forgetting get_forgetting;
        LeaveSpaceVisible LeaveSpaceView "LeaveSpaceView" "visible" set_visible get_visible;
        LeaveSpaceOnlyAdmin LeaveSpaceView "LeaveSpaceView" "only-admin" set_only_admin get_only_admin;
        LeaveSpaceLeaving LeaveSpaceView "LeaveSpaceView" "leaving" set_leaving get_leaving;
        RoomPreviewEntry RoomPreviewView "RoomPreviewView" "entry" set_entry get_entry;
        RoomPreviewIsSpace RoomPreviewView "RoomPreviewView" "is-space" set_is_space get_is_space;
        RoomPreviewOpensEvent RoomPreviewView "RoomPreviewView" "opens-event" set_opens_event get_opens_event;
        InvitePeopleVisible InvitePeopleView "InvitePeopleView" "visible" set_visible get_visible;
        InvitePeopleIsSpace InvitePeopleView "InvitePeopleView" "is-space" set_is_space get_is_space;
        InvitePeopleLimited InvitePeopleView "InvitePeopleView" "limited" set_limited get_limited;
        InvitePeopleSending InvitePeopleView "InvitePeopleView" "sending" set_sending get_sending;
        VerificationIsSelf VerificationView "VerificationView" "is-self" set_is_self get_is_self;
        TimelineRetryable RoomView "RoomView" "timeline-retryable" set_timeline_retryable get_timeline_retryable;
        TimelineDetached RoomView "RoomView" "timeline-detached" set_timeline_detached get_timeline_detached;
        BackwardsLoading RoomView "RoomView" "backwards-loading" set_backwards_loading get_backwards_loading;
        OlderHistoryAvailable RoomView "RoomView" "older-history-available" set_older_history_available get_older_history_available;
        ForwardsLoading RoomView "RoomView" "forwards-loading" set_forwards_loading get_forwards_loading;
        SelectedRoomEncrypted RoomView "RoomView" "selected-room-encrypted" set_selected_room_encrypted get_selected_room_encrypted;
        SelectedRoomReadableByAnyone RoomView "RoomView" "selected-room-readable-by-anyone" set_selected_room_readable_by_anyone get_selected_room_readable_by_anyone;
        MayVote RoomView "RoomView" "may-vote" set_may_vote get_may_vote;
        MayEndPolls RoomView "RoomView" "may-end-polls" set_may_end_polls get_may_end_polls;
        MayStartPolls RoomView "RoomView" "may-start-polls" set_may_start_polls get_may_start_polls;
        MayDeleteOwn RoomView "RoomView" "may-delete-own" set_may_delete_own get_may_delete_own;
        MayDeleteOthers RoomView "RoomView" "may-delete-others" set_may_delete_others get_may_delete_others;
        MayPin RoomView "RoomView" "may-pin" set_may_pin get_may_pin;
        MaySendMessages RoomView "RoomView" "may-send-messages" set_may_send_messages get_may_send_messages;
        MayReact RoomView "RoomView" "may-react" set_may_react get_may_react;
        MaySendStickers RoomView "RoomView" "may-send-stickers" set_may_send_stickers get_may_send_stickers;
        PackRoomEncrypted PackView "PackView" "room-encrypted" set_room_encrypted get_room_encrypted;
        #[cfg(feature = "demo")] ProbeEnabled Probe "Probe" "enabled" set_enabled get_enabled;
        PackLoading PackView "PackView" "loading" set_loading get_loading;
        PackHasPacks PackView "PackView" "has-packs" set_has_packs get_has_packs;
        AttachmentVisible AttachmentView "AttachmentView" "visible" set_visible get_visible;
        AttachmentSending AttachmentView "AttachmentView" "sending" set_sending get_sending;
        VideoVisible VideoView "VideoView" "visible" set_visible get_visible;
        VideoLoading VideoView "VideoView" "loading" set_loading get_loading;
        VideoPlaying VideoView "VideoView" "playing" set_playing get_playing;
        VideoMuted VideoView "VideoView" "muted" set_muted get_muted;
        AudioVisible AudioView "AudioView" "visible" set_visible get_visible;
        AudioLoading AudioView "AudioView" "loading" set_loading get_loading;
        AudioPlaying AudioView "AudioView" "playing" set_playing get_playing;
        AudioSilent AudioView "AudioView" "silent" set_silent get_silent;
        UnsentVisible UnsentView "UnsentView" "visible" set_visible get_visible;
        UnsentIsEdit UnsentView "UnsentView" "is-edit" set_is_edit get_is_edit;
        UnsentEditCaption UnsentView "UnsentView" "edit-caption" set_edit_caption get_edit_caption;
        IdentityAlertBusy IdentityAlertView "IdentityAlertView" "busy" set_busy get_busy;
        WindowFocused WindowView "WindowView" "focused" set_focused get_focused;
        WindowActive WindowView "WindowView" "active" set_active get_active;
        ReplySwipeArmed ReplySwipe "ReplySwipe" "armed" set_armed get_armed;
        ReducedMotion Motion "Motion" "reduced" set_reduced get_reduced;
        RoomInfoIsDirect RoomInfoView "RoomInfoView" "is-direct" set_is_direct get_is_direct;
        RoomInfoTopicHasLinks RoomInfoView "RoomInfoView" "topic-has-links" set_topic_has_links get_topic_has_links;
        RoomInfoNotifyBusy RoomInfoView "RoomInfoView" "notify-busy" set_notify_busy get_notify_busy;
        RoomInfoLeaving RoomInfoView "RoomInfoView" "leaving" set_leaving get_leaving;
        RoomInfoHasMore RoomInfoView "RoomInfoView" "has-more" set_has_more get_has_more;
        RoomInfoMayInvite RoomInfoView "RoomInfoView" "may-invite" set_may_invite get_may_invite;
        RoomInfoReadableByAnyone RoomInfoView "RoomInfoView" "readable-by-anyone" set_readable_by_anyone get_readable_by_anyone;
        RoomMenuUnread RoomMenuView "RoomMenuView" "unread" set_unread get_unread;
        RoomMenuNotifyBusy RoomMenuView "RoomMenuView" "notify-busy" set_notify_busy get_notify_busy;
        RoomMenuLeaving RoomMenuView "RoomMenuView" "leaving" set_leaving get_leaving;
        RoomMenuMayInvite RoomMenuView "RoomMenuView" "may-invite" set_may_invite get_may_invite;
        RoomMenuIsSpace RoomMenuView "RoomMenuView" "is-space" set_is_space get_is_space;
        RoomMenuMayAddChildren RoomMenuView "RoomMenuView" "may-add-children" set_may_add_children get_may_add_children;
        RoomMenuHasCall RoomMenuView "RoomMenuView" "has-call" set_has_call get_has_call;
        UserInfoVisible UserInfoView "UserInfoView" "visible" set_visible get_visible;
        UserInfoIsSelf UserInfoView "UserInfoView" "is-self" set_is_self get_is_self;
        UserInfoReadFailed UserInfoView "UserInfoView" "read-failed" set_read_failed get_read_failed;
        UserInfoRetrying UserInfoView "UserInfoView" "retrying" set_retrying get_retrying;
        UserInfoIgnored UserInfoView "UserInfoView" "ignored" set_ignored get_ignored;
        UserInfoIgnoreBusy UserInfoView "UserInfoView" "ignore-busy" set_ignore_busy get_ignore_busy;
        UserInfoMayKick UserInfoView "UserInfoView" "may-kick" set_may_kick get_may_kick;
        UserInfoMayBan UserInfoView "UserInfoView" "may-ban" set_may_ban get_may_ban;
        UserInfoMayUnban UserInfoView "UserInfoView" "may-unban" set_may_unban get_may_unban;
        UserInfoSessionsKnown UserInfoView "UserInfoView" "sessions-known" set_sessions_known get_sessions_known;
        RoomLogVisible RoomLogView "RoomLogView" "visible" set_visible get_visible;
        ReadersHasMore ReadersView "ReadersView" "has-more" set_has_more get_has_more;
        MentionsOfferRoom MentionsView "MentionsView" "offers-room" set_offers_room get_offers_room;
    } };
}
pub(crate) use bool_props;

macro_rules! int_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        NewMessagesCount RoomView "RoomView" "new-messages-count" set_new_messages_count get_new_messages_count;
        AnchorIndex RoomView "RoomView" "anchor-index" set_anchor_index get_anchor_index;
        TimelineToken RoomView "RoomView" "timeline-token" set_timeline_token get_timeline_token;
        PrependToken RoomView "RoomView" "prepend-token" set_prepend_token get_prepend_token;
        SelectedRoomMembers RoomView "RoomView" "selected-room-members" set_selected_room_members get_selected_room_members;
        SelectedRoomColorIndex RoomView "RoomView" "selected-room-color-index" set_selected_room_color_index get_selected_room_color_index;
        PinnedCount RoomView "RoomView" "pinned-count" set_pinned_count get_pinned_count;
        PinnedIndex RoomView "RoomView" "pinned-index" set_pinned_index get_pinned_index;
        SelectedGeneration DirectoryView "DirectoryView" "selected-generation" set_selected_generation get_selected_generation;
        SelectedRoomRow DirectoryView "DirectoryView" "selected-room-row" set_selected_room_row get_selected_room_row;
        SelectedRoomHeadings DirectoryView "DirectoryView" "selected-room-headings" set_selected_room_headings get_selected_room_headings;
        InviteColorIndex InviteView "InviteView" "color-index" set_color_index get_color_index;
        RemovalColorIndex RemovalView "RemovalView" "color-index" set_color_index get_color_index;
        LeaveSpaceRooms LeaveSpaceView "LeaveSpaceView" "rooms" set_rooms get_rooms;
        LeaveSpaceSpaces LeaveSpaceView "LeaveSpaceView" "spaces" set_spaces get_spaces;
        RoomPreviewColorIndex RoomPreviewView "RoomPreviewView" "color-index" set_color_index get_color_index;
        RoomPreviewMembers RoomPreviewView "RoomPreviewView" "members" set_members get_members;
        ListedSpaceMembers DirectoryView "DirectoryView" "listed-space-members" set_listed_space_members get_listed_space_members;
        SpaceIndexPagesLanded SpaceIndexView "SpaceIndexView" "pages-landed" set_pages_landed get_pages_landed;
        AttachmentWidth AttachmentView "AttachmentView" "width" set_width get_width;
        AttachmentHeight AttachmentView "AttachmentView" "height" set_height get_height;
        VideoPositionMs VideoView "VideoView" "position-ms" set_position_ms get_position_ms;
        VideoDurationMs VideoView "VideoView" "duration-ms" set_duration_ms get_duration_ms;
        AudioPositionMs AudioView "AudioView" "position-ms" set_position_ms get_position_ms;
        AudioDurationMs AudioView "AudioView" "duration-ms" set_duration_ms get_duration_ms;
        UnsentSubmission UnsentView "UnsentView" "submission" set_submission get_submission;
        UnsentShortenBy UnsentView "UnsentView" "shorten-by" set_shorten_by get_shorten_by;
        TypingCount TypingView "TypingView" "count" set_count get_count;
        RingColorIndex CallView "CallView" "ring-color-index" set_ring_color_index get_ring_color_index;
        RingSerial CallView "CallView" "ring-serial" set_ring_serial get_ring_serial;
        SheetSerial CallView "CallView" "sheet-serial" set_sheet_serial get_sheet_serial;
        SheetPeople CallView "CallView" "sheet-people" set_sheet_people get_sheet_people;
        SheetPeopleMore CallView "CallView" "sheet-people-more" set_sheet_people_more get_sheet_people_more;
        SheetBridgesMore CallView "CallView" "sheet-bridges-more" set_sheet_bridges_more get_sheet_bridges_more;
        HerePeople CallView "CallView" "here-people" set_here_people get_here_people;
        NoticeOthers CallView "CallView" "notice-others" set_notice_others get_notice_others;
        ActiveCall CallView "CallView" "active-call" set_active_call get_active_call;
        ShareCount CallView "CallView" "share-count" set_share_count get_share_count;
        StartedShareSerial CallView "CallView" "started-share-serial" set_started_share_serial get_started_share_serial;
        EndedShareSerial CallView "CallView" "ended-share-serial" set_ended_share_serial get_ended_share_serial;
        ShareChangeCancelled CallView "CallView" "share-change-cancelled" set_share_change_cancelled get_share_change_cancelled;
        PeopleMore CallView "CallView" "people-more" set_people_more get_people_more;
        BridgesMore CallView "CallView" "bridges-more" set_bridges_more get_bridges_more;
        MicrophoneChosen CallView "CallView" "microphone-chosen" set_microphone_chosen get_microphone_chosen;
        CameraChosen CallView "CallView" "camera-chosen" set_camera_chosen get_camera_chosen;
        SpeakerChosen CallView "CallView" "speaker-chosen" set_speaker_chosen get_speaker_chosen;
        SheetTestLevel CallView "CallView" "sheet-test-level" set_sheet_test_level get_sheet_test_level;
        DeviceNewsSerial CallView "CallView" "device-news-serial" set_device_news_serial get_device_news_serial;
        CallDeclines WindowView "WindowView" "call-declines" set_call_declines get_call_declines;
        CallAnswers WindowView "WindowView" "call-answers" set_call_answers get_call_answers;
        PresencePartnerMinutes PresenceView "PresenceView" "partner-minutes" set_partner_minutes get_partner_minutes;
        PresencePersonMinutes PresenceView "PresenceView" "person-minutes" set_person_minutes get_person_minutes;
        SwipeTravel WindowView "WindowView" "swipe-travel" set_swipe_travel get_swipe_travel;
        ContextPresses WindowView "WindowView" "context-presses" set_context_presses get_context_presses;
        NotificationOpens WindowView "WindowView" "notification-opens" set_notification_opens get_notification_opens;
        ExternalLinkOpens WindowView "WindowView" "external-link-opens" set_external_link_opens get_external_link_opens;
        Attention DirectoryView "DirectoryView" "attention" set_attention get_attention;
        MessageLinkSerial RoomView "RoomView" "message-link-serial" set_message_link_serial get_message_link_serial;
        RoomInfoMembers RoomInfoView "RoomInfoView" "members" set_members get_members;
        RoomInfoColorIndex RoomInfoView "RoomInfoView" "color-index" set_color_index get_color_index;
        RoomInfoPagesLanded RoomInfoView "RoomInfoView" "pages-landed" set_pages_landed get_pages_landed;
        RoomLinkSerial RoomMenuView "RoomMenuView" "link-serial" set_link_serial get_link_serial;
        UserInfoColorIndex UserInfoView "UserInfoView" "color-index" set_color_index get_color_index;
        RoomLogDropped RoomLogView "RoomLogView" "dropped" set_dropped get_dropped;
        RoomLogLinesLanded RoomLogView "RoomLogView" "lines-landed" set_lines_landed get_lines_landed;
        ReadersCount ReadersView "ReadersView" "count" set_count get_count;
        ReadersPagesLanded ReadersView "ReadersView" "pages-landed" set_pages_landed get_pages_landed;
        LogoutUnsent LogoutView "LogoutView" "unsent" set_unsent get_unsent;
        EncryptionKeysCount EncryptionView "EncryptionView" "keys-count" set_keys_count get_keys_count;
        EncryptionKeysTotal EncryptionView "EncryptionView" "keys-total" set_keys_total get_keys_total;
    } };
}
pub(crate) use int_props;

macro_rules! enum_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        LoginPhase set_login_phase(LoginStep)
            LoginView "LoginView" "step" set_step get_step;
        LoginActivity set_login_activity(LoginActivity)
            LoginView "LoginView" "activity" set_activity get_activity;
        LoginMethodKind set_login_method_kind(LoginMethod)
            LoginView "LoginView" "method" set_method get_method;
        ConnectionState set_connection_state(&ConnectionStatus)
            SessionView "SessionView" "connection-status" set_connection_status get_connection_status;
        RoomScope set_room_scope(RoomScope)
            DirectoryView "DirectoryView" "room-scope" set_room_scope get_room_scope;
        TimelineState set_timeline_state(TimelineStatus)
            RoomView "RoomView" "timeline-status" set_timeline_status get_timeline_status;
        ToastMessage set_toast_message(UserMessageKind)
            RoomView "RoomView" "toast-message" set_toast_message get_toast_message;
        PinnedKind set_pinned_kind(MessagePreviewKind)
            RoomView "RoomView" "pinned-kind" set_pinned_kind get_pinned_kind;
        VerificationPhase set_verification_phase(VerifyStep)
            VerificationView "VerificationView" "step" set_step get_step;
        VerificationActivity set_verification_activity(VerificationActivity)
            VerificationView "VerificationView" "activity" set_activity get_activity;
        VerificationError set_verification_error(UserMessageKind)
            VerificationView "VerificationView" "error" set_error get_error;
        EncryptionStanding set_encryption_standing(Standing)
            EncryptionView "EncryptionView" "standing" set_standing get_standing;
        EncryptionRecovery set_encryption_recovery(Recovery)
            EncryptionView "EncryptionView" "recovery" set_recovery get_recovery;
        EncryptionStep set_encryption_step(&EncryptionStep)
            EncryptionView "EncryptionView" "step" set_step get_step;
        EncryptionActivity set_encryption_activity(EncryptionActivity)
            EncryptionView "EncryptionView" "activity" set_activity get_activity;
        EncryptionError set_encryption_error(UserMessageKind)
            EncryptionView "EncryptionView" "error" set_error get_error;
        EncryptionAuthStage set_encryption_auth_stage(AuthStage)
            EncryptionView "EncryptionView" "auth-stage" set_auth_stage get_auth_stage;
        EncryptionAuthError set_encryption_auth_error(UserMessageKind)
            EncryptionView "EncryptionView" "auth-error" set_auth_error get_auth_error;
        AccountStep set_account_step(&AccountStep)
            AccountView "AccountView" "step" set_step get_step;
        AccountActivity set_account_activity(AccountActivity)
            AccountView "AccountView" "activity" set_activity get_activity;
        AccountError set_account_error(UserMessageKind)
            AccountView "AccountView" "error" set_error get_error;
        AccountNotice set_account_notice(UserMessageKind)
            AccountView "AccountView" "notice" set_notice get_notice;
        AccountAuthStage set_account_auth_stage(AuthStage)
            AccountView "AccountView" "auth-stage" set_auth_stage get_auth_stage;
        AccountAuthError set_account_auth_error(UserMessageKind)
            AccountView "AccountView" "auth-error" set_auth_error get_auth_error;
        AccountAuthFor set_account_auth_for(AccountActivity)
            AccountView "AccountView" "auth-for" set_auth_for get_auth_for;
        AttachmentKind set_attachment_kind(AttachmentKind)
            AttachmentView "AttachmentView" "kind" set_kind get_kind;
        AttachmentError set_attachment_error(UserMessageKind)
            AttachmentView "AttachmentView" "error" set_error get_error;
        VideoError set_video_error(UserMessageKind)
            VideoView "VideoView" "error" set_error get_error;
        AudioKind set_audio_kind(AudioKind)
            AudioView "AudioView" "kind" set_kind get_kind;
        SpaceIndexStatus set_space_index_status(SpaceIndexStatus)
            SpaceIndexView "SpaceIndexView" "status" set_status get_status;
        RoomInfoPlacement set_room_info_placement(RoomInfoPlacement)
            RoomInfoView "RoomInfoView" "placement" set_placement get_placement;
        RoomInfoNotify set_room_info_notify(NotifyMode)
            RoomInfoView "RoomInfoView" "notify" set_notify get_notify;
        RoomInfoRoster set_room_info_roster(RosterStatus)
            RoomInfoView "RoomInfoView" "roster" set_roster get_roster;
        RoomInfoError set_room_info_error(UserMessageKind)
            RoomInfoView "RoomInfoView" "error" set_error get_error;
        RoomMenuNotify set_room_menu_notify(NotifyMode)
            RoomMenuView "RoomMenuView" "notify" set_notify get_notify;
        UserInfoRole set_user_info_role(MemberRole)
            UserInfoView "UserInfoView" "role" set_role get_role;
        UserInfoMembership set_user_info_membership(RoomMembership)
            UserInfoView "UserInfoView" "membership" set_membership get_membership;
        UserInfoError set_user_info_error(UserMessageKind)
            UserInfoView "UserInfoView" "error" set_error get_error;
        UserInfoDirect set_user_info_direct(DirectChat)
            UserInfoView "UserInfoView" "direct" set_direct get_direct;
        UserInfoModerating set_user_info_moderating(PendingModeration)
            UserInfoView "UserInfoView" "moderating" set_moderating get_moderating;
        UserInfoIdentity set_user_info_identity(IdentityTrust)
            UserInfoView "UserInfoView" "identity" set_identity get_identity;
        UserInfoVerify set_user_info_verify(VerifyOffer)
            UserInfoView "UserInfoView" "verify" set_verify get_verify;
        PresencePartner set_presence_partner(PresenceLine)
            PresenceView "PresenceView" "partner" set_partner get_partner;
        PresencePerson set_presence_person(PresenceLine)
            PresenceView "PresenceView" "person" set_person get_person;
        InviteKind set_invite_kind(InviteKind)
            InviteView "InviteView" "kind" set_kind get_kind;
        InviteAnswer set_invite_answer(PendingAnswer)
            InviteView "InviteView" "answer" set_answer get_answer;
        InviteError set_invite_error(UserMessageKind)
            InviteView "InviteView" "error" set_error get_error;
        RemovalKind set_removal_kind(RemovalKind)
            RemovalView "RemovalView" "kind" set_kind get_kind;
        RemovalShape set_removal_shape(InviteKind)
            RemovalView "RemovalView" "shape" set_shape get_shape;
        RemovalError set_removal_error(UserMessageKind)
            RemovalView "RemovalView" "error" set_error get_error;
        LeaveSpaceError set_leave_space_error(UserMessageKind)
            LeaveSpaceView "LeaveSpaceView" "error" set_error get_error;
        RoomPreviewStatus set_room_preview_status(&RoomPreviewSheet)
            RoomPreviewView "RoomPreviewView" "status" set_status get_status;
        RoomPreviewAccess set_room_preview_access(ChildAccess)
            RoomPreviewView "RoomPreviewView" "access" set_access get_access;
        RoomPreviewError set_room_preview_error(UserMessageKind)
            RoomPreviewView "RoomPreviewView" "error" set_error get_error;
        CreateRoomStatus set_create_room_status(CreateRoomStatus)
            CreateRoomView "CreateRoomView" "status" set_status get_status;
        CreateRoomKind set_create_room_kind(RoomKind)
            CreateRoomView "CreateRoomView" "kind" set_kind get_kind;
        CreateRoomError set_create_room_error(UserMessageKind)
            CreateRoomView "CreateRoomView" "error" set_error get_error;
        PeopleSearchStatus set_people_search_status(PeopleSearchStatus)
            InvitePeopleView "InvitePeopleView" "search" set_search get_search;
        InvitePeopleError set_invite_people_error(UserMessageKind)
            InvitePeopleView "InvitePeopleView" "error" set_error get_error;
        IdentityAlertKind set_identity_alert_kind(IdentityAlertKind)
            IdentityAlertView "IdentityAlertView" "kind" set_kind get_kind;
        UpgradeSuccessor set_upgrade_successor(&UpgradeLink)
            UpgradeView "UpgradeView" "successor" set_successor get_successor;
        UpgradePredecessor set_upgrade_predecessor(&UpgradeLink)
            UpgradeView "UpgradeView" "predecessor" set_predecessor get_predecessor;
        SourceStatus set_source_status(&SourceState)
            SourceView "SourceView" "status" set_status get_status;
        SourceEncryption set_source_encryption(&SourceEncryption)
            SourceView "SourceView" "encryption" set_encryption get_encryption;
        ReadersStatus set_readers_status(ReadersStatus)
            ReadersView "ReadersView" "status" set_status get_status;
        ReadersKind set_readers_kind(MessagePreviewKind)
            ReadersView "ReadersView" "kind" set_kind get_kind;
        ReadersServiceKind set_readers_service_kind(ServiceKind)
            ReadersView "ReadersView" "service-kind" set_service_kind get_service_kind;
        PackSurface set_pack_surface(PackSurface)
            PackView "PackView" "surface" set_surface get_surface;
        CallOffer set_call_offer(CallOfferMark)
            CallView "CallView" "offer" set_offer get_offer;
        CallRefusal set_call_refusal(JoinRefusal)
            CallView "CallView" "refusal" set_refusal get_refusal;
        CallSheetOffer set_call_sheet_offer(CallOfferMark)
            CallView "CallView" "sheet-offer" set_sheet_offer get_sheet_offer;
        CallSheetRefusal set_call_sheet_refusal(JoinRefusal)
            CallView "CallView" "sheet-refusal" set_sheet_refusal get_sheet_refusal;
        CallSheetPreview set_call_sheet_preview(SheetPreviewMark)
            CallView "CallView" "sheet-preview" set_sheet_preview get_sheet_preview;
        CallPreviewTrouble set_call_preview_trouble(PreviewTrouble)
            CallView "CallView" "sheet-preview-trouble" set_sheet_preview_trouble
            get_sheet_preview_trouble;
        CallSheetTestKind set_call_sheet_test_kind(DeviceKind)
            CallView "CallView" "sheet-test-kind" set_sheet_test_kind get_sheet_test_kind;
        CallSheetTestPhase set_call_sheet_test_phase(TestPhaseMark)
            CallView "CallView" "sheet-test-phase" set_sheet_test_phase get_sheet_test_phase;
        CallPhase set_call_phase(CallPhase)
            CallView "CallView" "phase" set_phase get_phase;
        CallMic set_call_mic(MicrophoneView)
            CallView "CallView" "mic" set_mic get_mic;
        CallCamera set_call_camera(CameraView)
            CallView "CallView" "camera" set_camera get_camera;
        CallScreen set_call_screen(CallScreenMark)
            CallView "CallView" "screen" set_screen get_screen;
        CallScreenRefusal set_call_screen_refusal(ScreenRefusal)
            CallView "CallView" "screen-refusal" set_screen_refusal get_screen_refusal;
        CallNotice set_call_notice(CallNoticeMark)
            CallView "CallView" "notice" set_notice get_notice;
        CallFailure set_call_failure(UserMessageKind)
            CallView "CallView" "failure" set_failure get_failure;
        RingRefusal set_ring_refusal(JoinRefusal)
            CallView "CallView" "ring-refusal" set_ring_refusal get_ring_refusal;
        DeviceNewsKind set_device_news_kind(DeviceKind)
            CallView "CallView" "device-news-kind" set_device_news_kind get_device_news_kind;
        DeviceNewsTrouble set_device_news_trouble(DeviceTrouble)
            CallView "CallView" "device-news-trouble" set_device_news_trouble get_device_news_trouble;
    } };
}
pub(crate) use enum_props;

macro_rules! model_props {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        timeline Message SpliceModel RoomView "RoomView" "timeline" set_timeline;
        rooms Room VecModel DirectoryView "DirectoryView" "rooms" set_rooms;
        spaces Space VecModel DirectoryView "DirectoryView" "spaces" set_spaces;
        subspaces Space VecModel DirectoryView "DirectoryView" "subspaces" set_subspaces;
        pack_rows PackRow SpliceModel PackView "PackView" "rows" set_rows;
        pack_tabs PackTab VecModel PackView "PackView" "packs" set_packs;
        space_children SpaceChild VecModel SpaceIndexView "SpaceIndexView" "rows" set_rows;
        room_members MemberRow VecModel RoomInfoView "RoomInfoView" "rows" set_rows;
        readers MemberRow VecModel ReadersView "ReadersView" "rows" set_rows;
        mentions MemberRow VecModel MentionsView "MentionsView" "rows" set_rows;
        invitee_results Invitee VecModel InvitePeopleView "InvitePeopleView" "results" set_results;
        invitee_picks Invitee VecModel InvitePeopleView "InvitePeopleView" "picked" set_picked;
        room_log LogLine SpliceModel RoomLogView "RoomLogView" "lines" set_lines;
        call_stage CallTile VecModel CallView "CallView" "stage" set_stage;
        call_tiles CallTile VecModel CallView "CallView" "tiles" set_tiles;
        call_self CallTile VecModel CallView "CallView" "compact-self" set_compact_self;
        call_people CallPerson VecModel CallView "CallView" "people" set_people;
        call_microphones CallDevice VecModel CallView "CallView" "microphones" set_microphones;
        call_speakers CallDevice VecModel CallView "CallView" "speakers" set_speakers;
        call_cameras CallDevice VecModel CallView "CallView" "cameras" set_cameras;
        user_devices UserDevice VecModel UserInfoView "UserInfoView" "devices" set_devices;
        account_sessions UserDevice VecModel AccountView "AccountView" "sessions" set_sessions;
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

macro_rules! define_ui_names {
    ($name:ident; $($rust:ident $ui:ident $lit:literal;)*) => {
        impl $name {
            pub fn ui_name(self) -> &'static str {
                match self { $(Self::$rust => $lit,)* }
            }
        }
    };
}
pub(crate) use define_ui_names;

macro_rules! login_phases {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Loading     Loading     "loading";
        RestorePaused RestorePaused "restore-paused";
        Homeserver  Homeserver  "homeserver";
        Credentials Credentials "credentials";
        Reauthenticate(_) Reauthenticate "reauthenticate";
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
        Cancelling     Cancelling     "cancelling";
        Syncing        Syncing        "syncing";
        SigningOut     SigningOut     "signing-out";
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
        SlidingSyncUnsupported SlidingSyncUnsupported "sliding-sync-unsupported";
    } };
}
pub(crate) use connection_states;

macro_rules! timeline_states {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None         None         "none";
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
        Waiting    Waiting    "waiting";
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
        SlidingSyncUnsupported    SlidingSyncUnsupported  "sliding-sync-unsupported";
        UnsupportedLoginMethod    UnsupportedLoginMethod  "unsupported-login-method";
        LoginFailed               LoginFailed             "login-failed";
        InvalidCredentials        InvalidCredentials      "invalid-credentials";
        AccountDeactivated        AccountDeactivated      "account-deactivated";
        InvalidUsername           InvalidUsername         "invalid-username";
        RateLimited               RateLimited             "rate-limited";
        LoginMethodUnsupported    LoginMethodUnsupported  "login-method-unsupported";
        InsecureSignInPage        InsecureSignInPage      "insecure-sign-in-page";
        SessionUnreadable         SessionUnreadable       "session-unreadable";
        SessionRestoreFailed      SessionRestoreFailed    "session-restore-failed";
        StoreKeyMissing           StoreKeyMissing         "store-key-missing";
        StoreKeyUnreadable        StoreKeyUnreadable      "store-key-unreadable";
        IdentityDiverged          IdentityDiverged        "identity-diverged";
        SessionExpired            SessionExpired          "session-expired";
        ReauthNotPossible         ReauthNotPossible       "reauth-not-possible";
        DataQuarantined           DataQuarantined         "data-quarantined";
        DataNotErased             DataNotErased           "data-not-erased";
        DeviceLeftBehind          DeviceLeftBehind        "device-left-behind";
        InterruptedLoginUnresolved InterruptedLoginUnresolved "interrupted-login-unresolved";
        AnotherInstanceRunning    AnotherInstanceRunning  "another-instance-running";
        LocalDataClaimFailed      LocalDataClaimFailed    "local-data-claim-failed";
        SessionSaveFailed         SessionSaveFailed       "session-save-failed";
        SendMessageFailed         SendMessageFailed       "send-message-failed";
        SendNotAllowed            SendNotAllowed          "send-not-allowed";
        LoadMoreFailed            LoadMoreFailed          "load-more-failed";
        MessageNotFound           MessageNotFound         "message-not-found";
        MessageNotShowable        MessageNotShowable      "message-not-showable";
        MessageLinkFailed         MessageLinkFailed       "message-link-failed";
        MessagePinFailed          MessagePinFailed        "message-pin-failed";
        MessageUnpinFailed        MessageUnpinFailed      "message-unpin-failed";
        MessageDeleteFailed       MessageDeleteFailed     "message-delete-failed";
        SpaceOrderSaveFailed      SpaceOrderSaveFailed    "space-order-save-failed";
        PackSettingFailed         PackSettingFailed       "pack-setting-failed";
        JoinRoomFailed            JoinRoomFailed          "join-room-failed";
        JoinNewRoomFailed         JoinNewRoomFailed       "join-new-room-failed";
        JoinOldRoomFailed         JoinOldRoomFailed       "join-old-room-failed";
        DeclineInviteFailed       DeclineInviteFailed     "decline-invite-failed";
        CallDeclineFailed         CallDeclineFailed       "call-decline-failed";
        CallJoinFailed            CallJoinFailed          "call-join-failed";
        CallServerUnreachable     CallServerUnreachable   "call-server-unreachable";
        CallServerRefused         CallServerRefused       "call-server-refused";
        CallMediaFailed           CallMediaFailed         "call-media-failed";
        CallCutOff                CallCutOff              "call-cut-off";
        CallNoAnswer              CallNoAnswer            "call-no-answer";
        CallDeclinedThere         CallDeclinedThere       "call-declined-there";
        CallUnjoinable            CallUnjoinable          "call-unjoinable";
        RoomNotFound              RoomNotFound            "room-not-found";
        RoomPreviewFailed         RoomPreviewFailed       "room-preview-failed";
        RoomAddressInvalid        RoomAddressInvalid      "room-address-invalid";
        LeaveRoomFailed           LeaveRoomFailed         "leave-room-failed";
        SpaceRoomsKept            SpaceRoomsKept          "space-rooms-kept";
        ForgetRoomFailed          ForgetRoomFailed        "forget-room-failed";
        RoomNameMissing           RoomNameMissing         "room-name-missing";
        RoomAddressMissing        RoomAddressMissing      "room-address-missing";
        RoomAddressUnusable       RoomAddressUnusable     "room-address-unusable";
        RoomAddressTaken          RoomAddressTaken        "room-address-taken";
        RoomCreateRefused         RoomCreateRefused       "room-create-refused";
        RoomCreateUnsupported     RoomCreateUnsupported   "room-create-unsupported";
        RoomCreateFailed          RoomCreateFailed        "room-create-failed";
        AddToSpaceFailed          AddToSpaceFailed        "add-to-space-failed";
        PeopleSearchFailed        PeopleSearchFailed      "people-search-failed";
        InviteBlocked             InviteBlocked           "invite-blocked";
        InviteRefused             InviteRefused           "invite-refused";
        InviteFailed              InviteFailed            "invite-failed";
        NotifyChangeFailed        NotifyChangeFailed      "notify-change-failed";
        MarkReadFailed            MarkReadFailed          "mark-read-failed";
        RoomLinkFailed            RoomLinkFailed          "room-link-failed";
        UserInfoFailed            UserInfoFailed          "user-info-failed";
        DirectChatFailed          DirectChatFailed        "direct-chat-failed";
        UserVerificationFailed    UserVerificationFailed  "user-verification-failed";
        PartnerLeftChat           PartnerLeftChat         "partner-left-chat";
        IgnoreFailed              IgnoreFailed            "ignore-failed";
        UnignoreFailed            UnignoreFailed          "unignore-failed";
        KickFailed                KickFailed              "kick-failed";
        BanFailed                 BanFailed               "ban-failed";
        UnbanFailed               UnbanFailed             "unban-failed";
        IdentityAcceptFailed      IdentityAcceptFailed    "identity-accept-failed";
        VerificationWithdrawFailed VerificationWithdrawFailed "verification-withdraw-failed";
        IdentityResetAcknowledgeFailed IdentityResetAcknowledgeFailed "identity-reset-acknowledge-failed";
        MediaDownloadFailed       MediaDownloadFailed     "media-download-failed";
        FileDownloadFailed        FileDownloadFailed      "file-download-failed";
        MediaOpenFailed           MediaOpenFailed         "media-open-failed";
        MediaNotViewable          MediaNotViewable        "media-not-viewable";
        AttachmentUnreadable      AttachmentUnreadable    "attachment-unreadable";
        AttachmentTooLarge        AttachmentTooLarge      "attachment-too-large";
        CaptionTooLong            CaptionTooLong          "caption-too-long";
        SendAttachmentFailed      SendAttachmentFailed    "send-attachment-failed";
        PollVoteFailed            PollVoteFailed          "poll-vote-failed";
        PollEndFailed             PollEndFailed           "poll-end-failed";
        PollEditFailed            PollEditFailed          "poll-edit-failed";
        PollTooLong               PollTooLong             "poll-too-long";
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
        VerificationWithdrawn     VerificationWithdrawn   "verification-withdrawn";
        VerificationMismatch      VerificationMismatch    "verification-mismatch";
        VerificationAcceptedElsewhere VerificationAcceptedElsewhere "verification-accepted-elsewhere";
        VerificationFailed        VerificationFailed      "verification-failed";
        VerificationRequestFailed VerificationRequestFailed "verification-request-failed";
        RecoveryKeyWrong          RecoveryKeyWrong        "recovery-key-wrong";
        RecoveryKeyFailed         RecoveryKeyFailed       "recovery-key-failed";
        RecoverySetupFailed       RecoverySetupFailed     "recovery-setup-failed";
        RecoveryBackupInTheWay    RecoveryBackupInTheWay  "recovery-backup-in-the-way";
        RecoveryKeyChangeFailed   RecoveryKeyChangeFailed "recovery-key-change-failed";
        AuthPasswordWrong         AuthPasswordWrong       "auth-password-wrong";
        AuthStepUnfinished        AuthStepUnfinished      "auth-step-unfinished";
        AuthRefused               AuthRefused             "auth-refused";
        AuthUnsupported           AuthUnsupported         "auth-unsupported";
        SessionRenameFailed       SessionRenameFailed     "session-rename-failed";
        SessionSignOutFailed      SessionSignOutFailed    "session-sign-out-failed";
        SessionSignedOut          SessionSignedOut        "session-signed-out";
        PasswordChangeFailed      PasswordChangeFailed    "password-change-failed";
        NewPasswordTooWeak        NewPasswordTooWeak      "new-password-too-weak";
        NewPasswordRefused        NewPasswordRefused      "new-password-refused";
        PasswordChanged           PasswordChanged         "password-changed";
        PasswordChangedOthersSignedOut PasswordChangedOthersSignedOut "password-changed-others-signed-out";
        DeactivationFailed        DeactivationFailed      "deactivation-failed";
        AccountPageOpened         AccountPageOpened       "account-page-opened";
        IdentityResetFailed       IdentityResetFailed     "identity-reset-failed";
        EncryptionSetupFailed     EncryptionSetupFailed   "encryption-setup-failed";
        KeyExportFailed           KeyExportFailed         "key-export-failed";
        KeyImportFailed           KeyImportFailed         "key-import-failed";
        KeyPassphraseWrong        KeyPassphraseWrong      "key-passphrase-wrong";
        KeyFileUnreadable         KeyFileUnreadable       "key-file-unreadable";
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

macro_rules! emoji_arts {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Text    Text    "text";
        Pending Pending "pending";
        Ready   Ready   "ready";
        Failed  Failed  "failed";
    } };
}
pub(crate) use emoji_arts;

macro_rules! spoiler_states {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None    None    "none";
        Covered Covered "covered";
        Shown   Shown   "shown";
    } };
}
pub(crate) use spoiler_states;

macro_rules! deliveries {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None    None    "none";
        Pending Pending "pending";
        Sent    Sent    "sent";
        Read    Read    "read";
        Seen    Seen    "seen";
        Failed  Failed  "failed";
    } };
}
pub(crate) use deliveries;

macro_rules! shields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None       None    "none";
        Warning(_) Warning "warning";
        Notice(_)  Notice  "notice";
    } };
}
pub(crate) use shields;

macro_rules! shield_reasons {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None                      None                      "none";
        SentInClear               SentInClear               "sent-in-clear";
        MismatchedSender          MismatchedSender          "mismatched-sender";
        UnknownDevice             UnknownDevice             "unknown-device";
        UnsignedDevice            UnsignedDevice            "unsigned-device";
        VerificationViolation     VerificationViolation     "verification-violation";
        UnverifiedIdentity        UnverifiedIdentity        "unverified-identity";
        AuthenticityNotGuaranteed AuthenticityNotGuaranteed "authenticity-not-guaranteed";
    } };
}
pub(crate) use shield_reasons;

macro_rules! decryption_failures {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Unexplained              Unexplained              "unexplained";
        SentBeforeJoining        SentBeforeJoining        "sent-before-joining";
        WithheldFromUnverified   WithheldFromUnverified   "withheld-from-unverified";
        WithheldBySender         WithheldBySender         "withheld-by-sender";
        HistoryWithoutBackup     HistoryWithoutBackup     "history-without-backup";
        HistoryNeedsVerification HistoryNeedsVerification "history-needs-verification";
        UnsignedDevice           UnsignedDevice           "unsigned-device";
        UnknownDevice            UnknownDevice            "unknown-device";
        VerificationViolation    VerificationViolation    "verification-violation";
    } };
}
pub(crate) use decryption_failures;

macro_rules! attachment_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        File  File  "file";
        Image Image "image";
        Video Video "video";
        Audio Audio "audio";
    } };
}
pub(crate) use attachment_kinds;

macro_rules! room_scopes {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        All    All    "all";
        Direct Direct "direct";
        Space  Space  "space";
    } };
}
pub(crate) use room_scopes;

macro_rules! space_index_statuses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Closed      Closed      "closed";
        Loading     Loading     "loading";
        Partial     Partial     "partial";
        Complete    Complete    "complete";
        LoadingMore LoadingMore "loading-more";
        Failed      Failed      "failed";
        MoreFailed  MoreFailed  "more-failed";
    } };
}
pub(crate) use space_index_statuses;

macro_rules! source_statuses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Closed          Closed      "closed";
        Locating{..}    Locating    "locating";
        Ready(_)        Ready       "ready";
        Unavailable{..} Unavailable "unavailable";
    } };
}
pub(crate) use source_statuses;

macro_rules! readers_statuses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Closed      Closed      "closed";
        Locating    Locating    "locating";
        Ready       Ready       "ready";
        Unavailable Unavailable "unavailable";
    } };
}
pub(crate) use readers_statuses;

macro_rules! source_encryptions {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Plain         Plain         "plain";
        Decrypted{..} Decrypted     "decrypted";
        Undecryptable Undecryptable "undecryptable";
    } };
}
pub(crate) use source_encryptions;

macro_rules! room_preview_statuses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Closed      Closed      "closed";
        Idle        Idle        "idle";
        Loading     Loading     "loading";
        Loaded(_)   Loaded      "loaded";
        Missing     Missing     "missing";
        Unavailable Unavailable "unavailable";
        Failed      Failed      "failed";
    } };
}
pub(crate) use room_preview_statuses;

macro_rules! child_accesses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Open         Open         "open";
        OpenSubspace OpenSubspace "open-subspace";
        Joined       Joined       "joined";
        Joining      Joining      "joining";
        Join         Join         "join";
        InviteOnly   InviteOnly   "invite-only";
        Knock        Knock        "knock";
        MembersOnly  MembersOnly  "members-only";
        Unavailable  Unavailable  "unavailable";
    } };
}
pub(crate) use child_accesses;

macro_rules! notify_modes {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        AllMessages  AllMessages  "all-messages";
        MentionsOnly MentionsOnly "mentions-only";
        Muted        Muted        "muted";
    } };
}
pub(crate) use notify_modes;

macro_rules! member_roles {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Member    Member    "member";
        Moderator Moderator "moderator";
        Admin     Admin     "admin";
        Owner     Owner     "owner";
    } };
}
pub(crate) use member_roles;

macro_rules! room_memberships {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Joined   Joined   "joined";
        Invited  Invited  "invited";
        Knocking Knocking "knocking";
        Left     Left     "left";
        Banned   Banned   "banned";
        Outside  Outside  "outside";
        Unknown  Unknown  "unknown";
    } };
}
pub(crate) use room_memberships;

macro_rules! direct_chats {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Hidden   Hidden   "hidden";
        Open     Open     "open";
        Start    Start    "start";
        Starting Starting "starting";
    } };
}
pub(crate) use direct_chats;

macro_rules! upgrade_links {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Hidden     Hidden  "hidden";
        Open(_)    Open    "open";
        Join(_)    Join    "join";
        Joining(_) Joining "joining";
    } };
}
pub(crate) use upgrade_links;

macro_rules! invite_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Room   Room   "room";
        Space  Space  "space";
        Direct Direct "direct";
    } };
}
pub(crate) use invite_kinds;

macro_rules! removal_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Kicked Kicked "kicked";
        Banned Banned "banned";
    } };
}
pub(crate) use removal_kinds;

macro_rules! invite_answers {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None    None    "none";
        Accept  Accept  "accept";
        Decline Decline "decline";
    } };
}
pub(crate) use invite_answers;

macro_rules! presence_lines {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Hidden       Hidden   "hidden";
        Online       Online   "online";
        Away         Away     "away";
        LastSeen{..} LastSeen "last-seen";
    } };
}
pub(crate) use presence_lines;

macro_rules! identity_trusts {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Unverified       Unverified       "unverified";
        Verified         Verified         "verified";
        NoLongerVerified NoLongerVerified "no-longer-verified";
        NoIdentity       NoIdentity       "no-identity";
    } };
}
pub(crate) use identity_trusts;

macro_rules! verify_offers {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Hidden     Hidden     "hidden";
        Offered    Offered    "offered";
        Requesting Requesting "requesting";
    } };
}
pub(crate) use verify_offers;

macro_rules! device_trusts {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Verified         Verified         "verified";
        SignedByOwner    SignedByOwner    "signed-by-owner";
        NotSignedByOwner NotSignedByOwner "not-signed-by-owner";
    } };
}
pub(crate) use device_trusts;

macro_rules! identity_alert_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None                  None                  "none";
        OwnIdentityReset      OwnIdentityReset      "own-identity-reset";
        SessionUnverified     SessionUnverified     "session-unverified";
        EncryptionNotSetUp    EncryptionNotSetUp    "encryption-not-set-up";
        NoLongerVerified      NoLongerVerified      "no-longer-verified";
        IdentityReset         IdentityReset         "identity-reset";
    } };
}
pub(crate) use identity_alert_kinds;

macro_rules! encryption_standings {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Checking     Checking     "checking";
        SetupRefused SetupRefused "setup-refused";
        SetupFailed  SetupFailed  "setup-failed";
        Unverified   Unverified   "unverified";
        Verified     Verified     "verified";
    } };
}
pub(crate) use encryption_standings;

macro_rules! recovery_statuses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Unknown    Unknown    "unknown";
        Off        Off        "off";
        On         On         "on";
        Incomplete Incomplete "incomplete";
    } };
}
pub(crate) use recovery_statuses;

macro_rules! encryption_steps {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Overview               Overview              "overview";
        EnteringKey            EnteringKey           "entering-key";
        Unlocked               Unlocked              "unlocked";
        UnlockedWithoutBackup  UnlockedWithoutBackup "unlocked-without-backup";
        ShowingKey(_)          ShowingKey            "showing-key";
        ConfirmingReset        ConfirmingReset       "confirming-reset";
        AuthenticatingSetup(_) AuthenticatingSetup   "authenticating-setup";
        AuthenticatingReset(_) AuthenticatingReset   "authenticating-reset";
        IdentityReset          IdentityReset         "identity-reset";
        ExportingKeys(_)       ExportingKeys         "exporting-keys";
        KeysExported{..}       KeysExported          "keys-exported";
        ImportingKeys(_)       ImportingKeys         "importing-keys";
        KeysImported{..}       KeysImported          "keys-imported";
    } };
}
pub(crate) use encryption_steps;

macro_rules! encryption_activities {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None              None              "none";
        Requesting        Requesting        "requesting";
        Unlocking         Unlocking         "unlocking";
        SettingUpRecovery SettingUpRecovery "setting-up-recovery";
        ChangingKey       ChangingKey       "changing-key";
        RetryingSetup     RetryingSetup     "retrying-setup";
        ResettingIdentity ResettingIdentity "resetting-identity";
        Authenticating    Authenticating    "authenticating";
        ChoosingFile      ChoosingFile      "choosing-file";
        ExportingKeys     ExportingKeys     "exporting-keys";
        ImportingKeys     ImportingKeys     "importing-keys";
    } };
}
pub(crate) use encryption_activities;

macro_rules! auth_stages {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Password Password "password";
        Approve  Approve  "approve";
        Browser  Browser  "browser";
    } };
}
pub(crate) use auth_stages;

macro_rules! account_steps {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Overview               Overview               "overview";
        ChangingPassword       ChangingPassword       "changing-password";
        ConfirmingDeactivation ConfirmingDeactivation "confirming-deactivation";
        Authenticating(_)      Authenticating         "authenticating";
    } };
}
pub(crate) use account_steps;

macro_rules! account_activities {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None             None             "none";
        Renaming         Renaming         "renaming";
        SigningOut       SigningOut       "signing-out";
        ChangingPassword ChangingPassword "changing-password";
        Deactivating     Deactivating     "deactivating";
    } };
}
pub(crate) use account_activities;

macro_rules! pending_moderations {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None  None  "none";
        Kick  Kick  "kick";
        Ban   Ban   "ban";
        Unban Unban "unban";
    } };
}
pub(crate) use pending_moderations;

macro_rules! member_row_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Member         Member         "member";
        InvitedHeading InvitedHeading "invited-heading";
    } };
}
pub(crate) use member_row_kinds;

macro_rules! room_row_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Room            Room            "room";
        InvitesHeading  InvitesHeading  "invites-heading";
        InvitesFolded   InvitesFolded   "invites-folded";
        InvitesUnfolded InvitesUnfolded "invites-unfolded";
        Invite          Invite          "invite";
        Removal         Removal         "removal";
        RoomsHeading    RoomsHeading    "rooms-heading";
        SpacesHeading   SpacesHeading   "spaces-heading";
        Space           Space           "space";
    } };
}
pub(crate) use room_row_kinds;

macro_rules! room_call_marks {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None  None  "none";
        Voice Voice "voice";
        Video Video "video";
    } };
}
pub(crate) use room_call_marks;

macro_rules! call_offers {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None    None    "none";
        Start   Start   "start";
        Join    Join    "join";
        InCall  InCall  "in-call";
        Refused Refused "refused";
    } };
}
pub(crate) use call_offers;

macro_rules! sheet_previews {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None      None      "none";
        Starting  Starting  "starting";
        Local     Local     "local";
        InCall    InCall    "in-call";
        Elsewhere Elsewhere "elsewhere";
        Failed    Failed    "failed";
    } };
}
pub(crate) use sheet_previews;

macro_rules! test_phases {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None     None     "none";
        Starting Starting "starting";
        Running  Running  "running";
        Failed   Failed   "failed";
        Lost     Lost     "lost";
    } };
}
pub(crate) use test_phases;

macro_rules! preview_troubles {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        NoCamera   NoCamera   "no-camera";
        NotStarted NotStarted "not-started";
        Stopped    Stopped    "stopped";
    } };
}
pub(crate) use preview_troubles;

macro_rules! call_refusals {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        NotBuilt          NotBuilt          "not-built";
        MediaBroken       MediaBroken       "media-broken";
        NoServer          NoServer          "no-server";
        ServerRefused     ServerRefused     "server-refused";
        NotAllowed        NotAllowed        "not-allowed";
        SessionUnverified SessionUnverified "session-unverified";
    } };
}
pub(crate) use call_refusals;

macro_rules! call_phases {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Joining      Joining      "joining";
        Calling      Calling      "calling";
        Connecting   Connecting   "connecting";
        Waiting      Waiting      "waiting";
        Live         Live         "live";
        Reconnecting Reconnecting "reconnecting";
    } };
}
pub(crate) use call_phases;

macro_rules! call_notices {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        None                  None                  "none";
        NotCalled             NotCalled             "not-called";
        NotAnnounced          NotAnnounced          "not-announced";
        MicrophoneFailed      MicrophoneFailed      "microphone-failed";
        CameraFailed          CameraFailed          "camera-failed";
        ScreenFailed          ScreenFailed          "screen-failed";
        ChoosingScreen        ChoosingScreen        "choosing-screen";
        SharingNeverStarted   SharingNeverStarted   "sharing-never-started";
        ConnectionPoor        ConnectionPoor        "connection-poor";
        CantDecrypt           CantDecrypt           "cant-decrypt";
        UnverifiedDevice      UnverifiedDevice      "unverified-device";
        AudioNotReceived      AudioNotReceived      "audio-not-received";
        VideoNotReceived      VideoNotReceived      "video-not-received";
        AudioVideoNotReceived AudioVideoNotReceived "audio-video-not-received";
        ScreenNotReceived     ScreenNotReceived     "screen-not-received";
        MediaNotReceived      MediaNotReceived      "media-not-received";
    } };
}
pub(crate) use call_notices;

macro_rules! call_mics {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        On       On       "on";
        Off      Off      "off";
        Starting Starting "starting";
        Failed   Failed   "failed";
    } };
}
pub(crate) use call_mics;

macro_rules! call_cameras {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Off      Off      "off";
        Starting Starting "starting";
        On       On       "on";
        Failed   Failed   "failed";
    } };
}
pub(crate) use call_cameras;

macro_rules! call_screens {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Off         Off         "off";
        Choosing    Choosing    "choosing";
        On          On          "on";
        Failed      Failed      "failed";
        Unavailable Unavailable "unavailable";
    } };
}
pub(crate) use call_screens;

macro_rules! call_screen_refusals {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        NotWayland NotWayland "not-wayland";
        NoPortal   NoPortal   "no-portal";
    } };
}
pub(crate) use call_screen_refusals;

macro_rules! call_device_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Microphone Microphone "microphone";
        Speaker    Speaker    "speaker";
        Camera     Camera     "camera";
    } };
}
pub(crate) use call_device_kinds;

macro_rules! call_device_statuses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        SystemDefault SystemDefault "system-default";
        Connected     Connected     "connected";
        Missing       Missing       "missing";
        Failed        Failed        "failed";
    } };
}
pub(crate) use call_device_statuses;

macro_rules! call_device_troubles {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Disconnected Disconnected "disconnected";
        Failed       Failed       "failed";
    } };
}
pub(crate) use call_device_troubles;

macro_rules! call_tile_marks {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Joining               Joining               "joining";
        WaitingForKeys        WaitingForKeys        "waiting-for-keys";
        Live                  Live                  "live";
        PoorConnection        PoorConnection        "poor-connection";
        AudioNotReceived      AudioNotReceived      "audio-not-received";
        VideoNotReceived      VideoNotReceived      "video-not-received";
        AudioVideoNotReceived AudioVideoNotReceived "audio-video-not-received";
        ScreenNotReceived     ScreenNotReceived     "screen-not-received";
        MediaNotReceived      MediaNotReceived      "media-not-received";
        ConnectionLost        ConnectionLost        "connection-lost";
        CantDecrypt           CantDecrypt           "cant-decrypt";
        UnverifiedDevice      UnverifiedDevice      "unverified-device";
        Unreachable           Unreachable           "unreachable";
    } };
}
pub(crate) use call_tile_marks;

macro_rules! roster_statuses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Loading Loading "loading";
        Ready   Ready   "ready";
        Failed  Failed  "failed";
    } };
}
pub(crate) use roster_statuses;

macro_rules! room_info_placements {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Closed Closed "closed";
        Dialog Dialog "dialog";
        Pane   Pane   "pane";
    } };
}
pub(crate) use room_info_placements;

macro_rules! room_kinds {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Room  Room  "room";
        Space Space "space";
    } };
}
pub(crate) use room_kinds;

macro_rules! room_accesses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        InviteOnly   InviteOnly   "invite-only";
        SpaceMembers SpaceMembers "space-members";
        Public       Public       "public";
    } };
}
pub(crate) use room_accesses;

macro_rules! create_room_statuses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Closed   Closed   "closed";
        Editing  Editing  "editing";
        Creating Creating "creating";
    } };
}
pub(crate) use create_room_statuses;

macro_rules! people_search_statuses {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Idle      Idle      "idle";
        Searching Searching "searching";
        Ready     Ready     "ready";
        Failed    Failed    "failed";
    } };
}
pub(crate) use people_search_statuses;

macro_rules! invitee_states {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Available Available "available";
        Picked    Picked    "picked";
        Joined    Joined    "joined";
        Invited   Invited   "invited";
        Banned    Banned    "banned";
        Sending   Sending   "sending";
        Failed    Failed    "failed";
    } };
}
pub(crate) use invitee_states;

macro_rules! log_levels {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Error Error "error";
        Warn  Warn  "warn";
        Info  Info  "info";
        Debug Debug "debug";
        Trace Trace "trace";
    } };
}
pub(crate) use log_levels;

macro_rules! variant_named {
    ($fn:ident $src:ident; $($rust:ident $ui:ident $lit:literal;)*) => {
        pub fn $fn(name: &str) -> Option<$src> {
            match name { $($lit => Some($src::$rust),)* _ => None }
        }
    };
}
pub(crate) use variant_named;

macro_rules! pack_surfaces {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        StickerGrid      StickerGrid      "sticker-grid";
        EmoteGrid        EmoteGrid        "emote-grid";
        EmoteSuggestions EmoteSuggestions "emote-suggestions";
    } };
}
pub(crate) use pack_surfaces;

macro_rules! pack_origins {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Personal Personal "personal";
        Room     Room     "room";
        Global   Global   "global";
    } };
}
pub(crate) use pack_origins;

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
        None              None              "none";
        NoSource          NoSource          "no-source";
        Download          Download          "download";
        TooLarge          TooLarge          "too-large";
        Storage           Storage           "storage";
        Unreadable        Unreadable        "unreadable";
        UnsupportedFormat UnsupportedFormat "unsupported-format";
        TooLargeToDisplay TooLargeToDisplay "too-large-to-display";
        Damaged           Damaged           "damaged";
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
        Location    Location    "location";
        Service     Service     "service";
        Utd         Utd         "utd";
        Unsupported Unsupported "unsupported";
        Poll        Poll        "poll";
    } };
}
pub(crate) use message_kinds;

macro_rules! poll_phases {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        Open   Open   "open";
        Sealed Sealed "sealed";
        Ended  Ended  "ended";
    } };
}
pub(crate) use poll_phases;

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
        Poll      Poll      "poll";
        Deleted   Deleted   "deleted";
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
        KnockRetracted     KnockRetracted     "knock-retracted";
        KnockDenied        KnockDenied        "knock-denied";
        NameSet            NameSet            "name-set";
        NameChanged        NameChanged        "name-changed";
        NameRemoved        NameRemoved        "name-removed";
        AvatarChanged      AvatarChanged      "avatar-changed";
        RoomName           RoomName           "room-name";
        RoomNameRemoved    RoomNameRemoved    "room-name-removed";
        RoomTopic          RoomTopic          "room-topic";
        RoomAvatar         RoomAvatar         "room-avatar";
        RoomCreated        RoomCreated        "room-created";
        UpgradedRoomCreated UpgradedRoomCreated "upgraded-room-created";
        RoomUpgraded       RoomUpgraded       "room-upgraded";
        Encryption         Encryption         "encryption";
        JoinPublic         JoinPublic         "join-public";
        JoinInvite         JoinInvite         "join-invite";
        JoinKnock          JoinKnock          "join-knock";
        JoinRestricted     JoinRestricted     "join-restricted";
        JoinKnockRestricted JoinKnockRestricted "join-knock-restricted";
        JoinRule           JoinRule           "join-rule";
        HistoryWorldReadable HistoryWorldReadable "history-world-readable";
        HistoryShared      HistoryShared      "history-shared";
        HistoryInvited     HistoryInvited     "history-invited";
        HistoryJoined      HistoryJoined      "history-joined";
        History            History            "history";
        GuestsAllowed      GuestsAllowed      "guests-allowed";
        GuestsForbidden    GuestsForbidden    "guests-forbidden";
        GuestAccess        GuestAccess        "guest-access";
        ServerAcl          ServerAcl          "server-acl";
        MainAddress        MainAddress        "main-address";
        MainAddressRemoved MainAddressRemoved "main-address-removed";
        AltAddresses       AltAddresses       "alt-addresses";
        MessagePinned      MessagePinned      "message-pinned";
        MessageUnpinned    MessageUnpinned    "message-unpinned";
        PinsChanged        PinsChanged        "pins-changed";
        MadeAdmin          MadeAdmin          "made-admin";
        MadeModerator      MadeModerator      "made-moderator";
        NoLongerAdmin      NoLongerAdmin      "no-longer-admin";
        NoLongerModerator  NoLongerModerator  "no-longer-moderator";
        PowerLevel         PowerLevel         "power-level";
        PowerLevels        PowerLevels        "power-levels";
        CallStarted        CallStarted        "call-started";
        CallNotification   CallNotification   "call-notification";
        CallVideoNotification CallVideoNotification "call-video-notification";
        CallDeclined       CallDeclined       "call-declined";
        VerificationRequested VerificationRequested "verification-requested";
    } };
}
pub(crate) use service_kinds;

macro_rules! message_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        unique_id set_unique_id "unique-id" text;
        local_id set_local_id "local-id" text;
        sender set_sender "sender" text;
        sender_id set_sender_id "sender-id" text;
        pronouns set_pronouns "pronouns" list;
        body set_body "body" text;
        styled set_styled "styled" styled;
        has_links set_has_links "has-links" flag;
        mentions_you set_mentions_you "mentions-you" flag;
        body_lines set_body_lines "body-lines" structs(BodyLine);
        emote_only set_emote_only "emote-only" flag;
        spoilers set_spoilers "spoilers" enumk(SpoilerState);
        timestamp set_timestamp "timestamp" text;
        sent_at set_sent_at "sent-at" text;
        message_type set_message_type "message-type" enumk(MessageKind);
        preview_kind set_preview_kind "preview-kind" enumk(MessagePreviewKind);
        preview_body set_preview_body "preview-body" text;
        unsupported_kind set_unsupported_kind "unsupported-kind" text;
        event_id set_event_id "event-id" text;
        sender_initial set_sender_initial "sender-initial" text;
        color_index set_color_index "color-index" int;
        is_own set_is_own "is-own" flag;
        edited set_edited "edited" flag;
        text_editable set_text_editable "text-editable" flag;
        edit_text set_edit_text "edit-text" text;
        pinned set_pinned "pinned" flag;
        first_unread set_first_unread "first-unread" flag;
        counts_as_unread set_counts_as_unread "counts-as-unread" flag;
        send_state set_send_state "send-state" enumk(SendState);
        send_progress set_send_progress "send-progress" ratio;
        delivery set_delivery "delivery" enumk(Delivery);
        shield set_shield "shield" enumk(Shield);
        shield_reason set_shield_reason "shield-reason" enumk(ShieldReason);
        decryption_failure set_decryption_failure "decryption-failure" enumk(DecryptionFailure);
        readers set_readers "readers" text;
        hidden_readers set_hidden_readers "hidden-readers" int;
        reader_count set_reader_count "reader-count" int;
        has_reply set_has_reply "has-reply" flag;
        reply_event_id set_reply_event_id "reply-event-id" text;
        reply_sender set_reply_sender "reply-sender" text;
        reply_kind set_reply_kind "reply-kind" enumk(MessagePreviewKind);
        reply_body set_reply_body "reply-body" text;
        service_kind set_service_kind "service-kind" enumk(ServiceKind);
        service_target set_service_target "service-target" text;
        service_reason set_service_reason "service-reason" text;
        media_state set_media_state "media-state" enumk(MediaState);
        media_failure set_media_failure "media-failure" enumk(MediaFailureKind);
        image_mimetype set_image_mimetype "image-mimetype" text;
        image_extension set_image_extension "image-extension" text;
        image_width set_image_width "image-width" int;
        image_height set_image_height "image-height" int;
        duration set_duration "duration" text;
        filename set_filename "filename" text;
        size set_size "size" text;
        map_link set_map_link "map-link" text;
        audio_kind set_audio_kind "audio-kind" enumk(AudioKind);
        waveform set_waveform "waveform" floats;
        has_avatar set_has_avatar "has-avatar" flag;
        needs_media set_needs_media "needs-media" flag;
        thumbnail set_thumbnail "thumbnail" image;
        avatar set_avatar "avatar" image;
        reactions set_reactions "reactions" structs(Reaction);
        all_reactions set_all_reactions "all-reactions" structs(Reaction);
        poll_phase set_poll_phase "poll-phase" enumk(PollPhase);
        poll_editable set_poll_editable "poll-editable" flag;
        poll_choices set_poll_choices "poll-choices" int;
        poll_voters set_poll_voters "poll-voters" int;
        poll_answers set_poll_answers "poll-answers" structs(PollAnswer);
    } };
}
pub(crate) use message_fields;

macro_rules! body_line_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        pieces set_pieces "pieces" structs(BodyPiece);
        words set_words "words" styled;
        gaps set_gaps "gaps" int;
    } };
}
pub(crate) use body_line_fields;

macro_rules! body_piece_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        styled set_styled "styled" styled;
        key set_key "key" text;
        art set_art "art" enumk(EmojiArt);
        image set_image "image" image;
        spaced set_spaced "spaced" flag;
    } };
}
pub(crate) use body_piece_fields;

macro_rules! poll_answer_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        id set_id "id" text;
        label set_label "label" text;
        count set_count "count" int;
        share set_share "share" ratio;
        mine set_mine "mine" flag;
        leading set_leading "leading" flag;
        votable set_votable "votable" flag;
        voters set_voters "voters" text;
        hidden_voters set_hidden_voters "hidden-voters" int;
    } };
}
pub(crate) use poll_answer_fields;

macro_rules! reaction_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        key set_key "key" text;
        label set_label "label" text;
        art set_art "art" enumk(EmojiArt);
        image set_image "image" image;
        count set_count "count" int;
        mine set_mine "mine" flag;
        send set_send "send" enumk(ReactionSend);
        overflow set_overflow "overflow" flag;
        reactors set_reactors "reactors" text;
        hidden_reactors set_hidden_reactors "hidden-reactors" int;
        avatars set_avatars "avatars" structs(Reactor);
    } };
}

macro_rules! reactor_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        user_id set_user_id "user-id" text;
        initial set_initial "initial" text;
        color_index set_color_index "color-index" int;
        avatar set_avatar "avatar" image;
        has_avatar set_has_avatar "has-avatar" flag;
    } };
}
pub(crate) use reaction_fields;
pub(crate) use reactor_fields;

macro_rules! room_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        id set_id "id" text;
        kind set_kind "kind" enumk(RoomRowKind);
        name set_name "name" text;
        initial set_initial "initial" text;
        color_index set_color_index "color-index" int;
        members set_members "members" int;
        alert set_alert "alert" flag;
        mention set_mention "mention" flag;
        hint set_hint "hint" flag;
        muted set_muted "muted" flag;
        last_message_sender set_last_message_sender "last-message-sender" text;
        last_message_kind set_last_message_kind "last-message-kind" enumk(MessagePreviewKind);
        last_message_body set_last_message_body "last-message-body" text;
        last_message_service_kind set_last_message_service_kind "last-message-service-kind" enumk(ServiceKind);
        last_message_service_target set_last_message_service_target "last-message-service-target" text;
        last_message_is_own set_last_message_is_own "last-message-is-own" flag;
        last_message_edited set_last_message_edited "last-message-edited" flag;
        last_message_time set_last_message_time "last-message-time" text;
        space_rooms set_space_rooms "space-rooms" int;
        space_parent set_space_parent "space-parent" text;
        invite_count set_invite_count "invite-count" int;
        invite_kind set_invite_kind "invite-kind" enumk(InviteKind);
        inviter set_inviter "inviter" text;
        inviter_id set_inviter_id "inviter-id" text;
        removal set_removal "removal" enumk(RemovalKind);
        removed_by set_removed_by "removed-by" text;
        call set_call "call" enumk(RoomCallMark);
        has_avatar set_has_avatar "has-avatar" flag;
        avatar set_avatar "avatar" image;
    } };
}
pub(crate) use room_fields;

macro_rules! space_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        id set_id "id" text;
        name set_name "name" text;
        alert set_alert "alert" flag;
        mention set_mention "mention" flag;
        hint set_hint "hint" flag;
        initial set_initial "initial" text;
        has_avatar set_has_avatar "has-avatar" flag;
        avatar set_avatar "avatar" image;
    } };
}
pub(crate) use space_fields;

macro_rules! space_child_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        id set_id "id" text;
        name set_name "name" text;
        initial set_initial "initial" text;
        color_index set_color_index "color-index" int;
        detail set_detail "detail" text;
        members set_members "members" int;
        is_space set_is_space "is-space" flag;
        children set_children "children" int;
        access set_access "access" enumk(ChildAccess);
        has_avatar set_has_avatar "has-avatar" flag;
        avatar set_avatar "avatar" image;
    } };
}
pub(crate) use space_child_fields;

macro_rules! member_row_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        user_id set_user_id "user-id" text;
        kind set_kind "kind" enumk(MemberRowKind);
        name set_name "name" text;
        initial set_initial "initial" text;
        color_index set_color_index "color-index" int;
        role set_role "role" enumk(MemberRole);
        has_avatar set_has_avatar "has-avatar" flag;
        avatar set_avatar "avatar" image;
    } };
}
pub(crate) use member_row_fields;

macro_rules! invitee_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        user_id set_user_id "user-id" text;
        name set_name "name" text;
        initial set_initial "initial" text;
        color_index set_color_index "color-index" int;
        state set_state "state" enumk(InviteeState);
        failure set_failure "failure" enumk(UserMessageKind);
        failure_detail set_failure_detail "failure-detail" text;
        has_avatar set_has_avatar "has-avatar" flag;
        avatar set_avatar "avatar" image;
    } };
}
pub(crate) use invitee_fields;

macro_rules! call_tile_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        key set_key "key" text;
        member set_member "member" text;
        user_id set_user_id "user-id" text;
        name set_name "name" text;
        device set_device "device" int;
        initial set_initial "initial" text;
        color_index set_color_index "color-index" int;
        has_avatar set_has_avatar "has-avatar" flag;
        avatar set_avatar "avatar" image;
        own set_own "own" flag;
        state set_state "state" enumk(CallTileMark);
        muted set_muted "muted" flag;
        speaking set_speaking "speaking" flag;
        feed set_feed "feed" text;
        has_frame set_has_frame "has-frame" flag;
        frame set_frame "frame" image;
        source_width set_source_width "source-width" int;
        source_height set_source_height "source-height" int;
        mirrored set_mirrored "mirrored" flag;
        screen set_screen "screen" flag;
    } };
}
pub(crate) use call_tile_fields;

macro_rules! call_person_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        key set_key "key" text;
        user_id set_user_id "user-id" text;
        name set_name "name" text;
        initial set_initial "initial" text;
        color_index set_color_index "color-index" int;
        has_avatar set_has_avatar "has-avatar" flag;
        avatar set_avatar "avatar" image;
        you set_you "you" flag;
        devices set_devices "devices" int;
        muted set_muted "muted" flag;
        sharing set_sharing "sharing" flag;
        sees_no_video set_sees_no_video "sees-no-video" flag;
        exposes_e2ee set_exposes_e2ee "exposes-e2ee" flag;
        state set_state "state" enumk(CallTileMark);
    } };
}
pub(crate) use call_person_fields;

macro_rules! call_device_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        key set_key "key" text;
        name set_name "name" text;
        status set_status "status" enumk(DeviceStatus);
        chosen set_chosen "chosen" flag;
    } };
}
pub(crate) use call_device_fields;

macro_rules! user_device_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        device_id set_device_id "device-id" text;
        name set_name "name" text;
        trust set_trust "trust" enumk(DeviceTrust);
        last_seen set_last_seen "last-seen" text;
        ip set_ip "ip" text;
    } };
}
pub(crate) use user_device_fields;

macro_rules! log_line_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        time set_time "time" text;
        level set_level "level" enumk(LogLevel);
        level_name set_level_name "level-name" text;
        target set_target "target" text;
        text set_text "text" text;
    } };
}
pub(crate) use log_line_fields;

macro_rules! pack_cell_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        key set_key "key" text;
        pack_id set_pack_id "pack-id" text;
        shortcode set_shortcode "shortcode" text;
        label set_label "label" text;
        media_state set_media_state "media-state" enumk(MediaState);
        image set_image "image" image;
    } };
}
pub(crate) use pack_cell_fields;

macro_rules! pack_tab_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        id set_id "id" text;
        title set_title "title" text;
        untitled set_untitled "untitled" flag;
        header_row set_header_row "header-row" int;
        icon set_icon "icon" image;
        has_icon set_has_icon "has-icon" flag;
    } };
}
pub(crate) use pack_tab_fields;

macro_rules! pack_row_fields {
    ($cb:ident $($pre:tt)*) => { $cb! { $($pre)*
        title set_title "title" text;
        untitled set_untitled "untitled" flag;
        pack_id set_pack_id "pack-id" text;
        origin set_origin "origin" enumk(PackOrigin);
        is_header set_is_header "is-header" flag;
        cells set_cells "cells" structs(PackCell);
    } };
}
pub(crate) use pack_row_fields;
