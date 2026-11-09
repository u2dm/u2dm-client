use strum::Display as StrumDisplay;

use super::UiCommand;
use crate::commands::view::SettingsPage;
use crate::domain::call::devices::{DeviceKey, DeviceKind};
use crate::domain::call::media::MediaChoice;
use crate::domain::capabilities::AccountPage;
use crate::domain::encryption::KeyPassphrase;
use crate::domain::room::RoomId;
use crate::domain::settings::SettingChange;
use crate::domain::uia::AccountPassword;

#[derive(StrumDisplay)]
pub enum DepartureCommand {
    #[strum(to_string = "OpenRemoval({0})")]
    OpenRemoval(RoomId),
    CloseRemoval,
    #[strum(to_string = "ForgetRoom({0})")]
    ForgetRoom(RoomId),
    #[strum(to_string = "OpenLeaveSpace({0})")]
    OpenLeaveSpace(RoomId),
    CloseLeaveSpace,
    #[strum(to_string = "LeaveSpace({space_id}, {with_rooms})")]
    LeaveSpace {
        space_id: RoomId,
        with_rooms: bool,
    },
}

impl DepartureCommand {
    pub fn room_id(&self) -> Option<&RoomId> {
        match self {
            Self::OpenRemoval(room_id)
            | Self::ForgetRoom(room_id)
            | Self::OpenLeaveSpace(room_id)
            | Self::LeaveSpace {
                space_id: room_id, ..
            } => Some(room_id),
            Self::CloseRemoval | Self::CloseLeaveSpace => None,
        }
    }
}

#[derive(StrumDisplay)]
pub enum KeysCommand {
    StartIdentityReset,
    ResetIdentity,
    StartKeyExport,
    #[strum(to_string = "ExportRoomKeys(...)")]
    ExportRoomKeys(KeyPassphrase),
    StartKeyImport,
    #[strum(to_string = "ImportRoomKeys(...)")]
    ImportRoomKeys(KeyPassphrase),
}

#[derive(StrumDisplay)]
pub enum AccountCommand {
    OpenAccount,
    CloseAccount,
    ShowAccountOverview,
    #[strum(to_string = "RenameSession({device_id})")]
    RenameSession {
        device_id: String,
        name: String,
    },
    #[strum(to_string = "SignOutSession({0})")]
    SignOutSession(String),
    #[strum(to_string = "OpenAccountPage")]
    OpenAccountPage(AccountPage),
    StartPasswordChange,
    #[strum(to_string = "ChangePassword(...)")]
    ChangePassword {
        password: AccountPassword,
        sign_out_others: bool,
    },
    StartDeactivation,
    #[strum(to_string = "DeactivateAccount({erase})")]
    DeactivateAccount {
        erase: bool,
    },
}

#[derive(Clone, Copy, StrumDisplay)]
pub enum SettingsCommand {
    OpenSettings,
    CloseSettings,
    #[strum(to_string = "ShowSettingsPage")]
    ShowPage(SettingsPage),
    #[strum(to_string = "ChangeSetting")]
    Change(SettingChange),
    ClearDownloadedMedia,
}

#[derive(StrumDisplay)]
pub enum AuthCommand {
    #[strum(to_string = "AnswerAuthPassword(...)")]
    AnswerAuthPassword(AccountPassword),
    ContinueAuth,
    ReopenAuthPage,
    CancelAuth,
}

#[derive(StrumDisplay)]
pub enum CallCommand {
    #[strum(to_string = "DeclineCall({0})")]
    DeclineCall(RoomId),
    SilenceRing,
    #[strum(to_string = "OpenCallSheet({room_id})")]
    OpenCallSheet {
        room_id: RoomId,
        video: bool,
    },
    CloseCallSheet,
    #[strum(to_string = "SheetCamera({0})")]
    SheetCamera(bool),
    #[strum(to_string = "TestCallDevice({kind}, {on})")]
    TestCallDevice {
        kind: DeviceKind,
        on: bool,
    },
    #[strum(to_string = "StartCall({room_id})")]
    StartCall {
        room_id: RoomId,
        video: bool,
        media: MediaChoice,
        leaving: Option<RoomId>,
    },
    #[strum(to_string = "JoinCall({room_id})")]
    JoinCall {
        room_id: RoomId,
        media: MediaChoice,
        leaving: Option<RoomId>,
    },
    #[strum(to_string = "AnswerCall({room_id})")]
    AnswerCall {
        room_id: RoomId,
        camera: bool,
        leaving: Option<RoomId>,
    },
    LeaveCall,
    RetryFailedCall,
    DismissFailedCall,
    AnnounceCallAgain,
    #[strum(to_string = "SetMicrophone({0})")]
    SetMicrophone(bool),
    #[strum(to_string = "SetCamera({0})")]
    SetCamera(bool),
    #[strum(to_string = "ShareScreen({0})")]
    ShareScreen(bool),
    ChangeScreenShare,
    #[strum(to_string = "UseCallDevice({kind})")]
    UseCallDevice {
        kind: DeviceKind,
        key: Option<DeviceKey>,
    },
    RefreshCallDevices,
    #[strum(to_string = "FocusCallTile({0})")]
    FocusCallTile(String),
    UnpinCallTile,
    #[strum(to_string = "RingDevices({0})")]
    RingDevices(bool),
    ShowCallGallery,
    ViewCallShare,
    #[strum(to_string = "ShowSelfView({0})")]
    ShowSelfView(bool),
    #[strum(to_string = "ShowSharePreview({0})")]
    ShowSharePreview(bool),
    #[strum(to_string = "FollowSpeaker({0})")]
    FollowSpeaker(bool),
    #[strum(to_string = "WatchMicrophone({0})")]
    WatchMicrophone(bool),
    #[strum(to_string = "WatchConnections({0})")]
    WatchConnections(bool),
    #[strum(to_string = "MuteForMe({on})")]
    MuteForMe {
        user_id: String,
        on: bool,
    },
    #[strum(to_string = "RaiseHand({0})")]
    RaiseHand(bool),
    #[strum(to_string = "SendCallReaction({0})")]
    SendCallReaction(String),
}

impl CallCommand {
    pub fn room_id(&self) -> Option<&RoomId> {
        match self {
            Self::DeclineCall(room_id)
            | Self::OpenCallSheet { room_id, .. }
            | Self::StartCall { room_id, .. }
            | Self::JoinCall { room_id, .. }
            | Self::AnswerCall { room_id, .. } => Some(room_id),
            Self::SilenceRing
            | Self::CloseCallSheet
            | Self::SheetCamera(_)
            | Self::TestCallDevice { .. }
            | Self::LeaveCall
            | Self::RetryFailedCall
            | Self::DismissFailedCall
            | Self::AnnounceCallAgain
            | Self::SetMicrophone(_)
            | Self::SetCamera(_)
            | Self::ShareScreen(_)
            | Self::ChangeScreenShare
            | Self::UseCallDevice { .. }
            | Self::RefreshCallDevices
            | Self::FocusCallTile(_)
            | Self::UnpinCallTile
            | Self::RingDevices(_)
            | Self::ShowCallGallery
            | Self::ViewCallShare
            | Self::ShowSelfView(_)
            | Self::ShowSharePreview(_)
            | Self::FollowSpeaker(_)
            | Self::MuteForMe { .. }
            | Self::RaiseHand(_)
            | Self::SendCallReaction(_)
            | Self::WatchMicrophone(_)
            | Self::WatchConnections(_) => None,
        }
    }
}

macro_rules! command_groups {
    ($($variant:ident($group:ident),)*) => {
        $(
            impl From<$group> for UiCommand {
                fn from(command: $group) -> Self {
                    Self::$variant(command)
                }
            }
        )*
    };
}

command_groups! {
    Departure(DepartureCommand),
    Keys(KeysCommand),
    Account(AccountCommand),
    Settings(SettingsCommand),
    Auth(AuthCommand),
    Call(CallCommand),
}
