mod account;
mod calls;
mod people;
mod rooms;
mod settings;
mod timeline;

use serde::Serialize;

use self::account::{
    AccountDto, ConnectionDto, EncryptionDto, IdentityAlertDto, LifecycleDto, LogoutDto, account,
    connection, encryption, identity_alert, lifecycle, logout,
};
use self::calls::{CallDto, call};
use self::people::{
    MentionsDto, PresenceDto, ReadersDto, TypingDto, UserInfoDto, mentions, presence, readers,
    typing, user_info,
};
use self::rooms::{
    CreateRoomDto, DirectoryDto, InvitePeopleDto, InviteSheetDto, LeaveSpaceDto, RemovalSheetDto,
    RoomInfoDto, RoomLogDto, RoomMenuDto, RoomPreviewDto, SpaceIndexDto, UpgradeDto, create_room,
    directory, invite_people, invite_sheet, leave_space, removal_sheet, room_info, room_log,
    room_menu, room_preview, space_index, upgrade,
};
use self::settings::{SettingsDto, settings};
use self::timeline::{
    AttachmentDto, LinkDto, NotificationDto, NowPlayingDto, PacksDto, PaginationDto, PinnedDto,
    SourceDto, UnsentDto, VideoDto, attachment, audio, event_source, image_packs, link,
    notifications, pagination, pinned, unsent, video,
};
use super::names;
use crate::commands::messages::UserMessage;
use crate::commands::view::{AppViewState, Toast};

#[derive(Serialize)]
pub struct ViewDto {
    lifecycle: LifecycleDto,
    connection: ConnectionDto,
    directory: DirectoryDto,
    space_index: SpaceIndexDto,
    invite: InviteSheetDto,
    removal: RemovalSheetDto,
    leave_space: LeaveSpaceDto,
    room_preview: RoomPreviewDto,
    create_room: CreateRoomDto,
    invite_people: InvitePeopleDto,
    room_info: RoomInfoDto,
    room_menu: Option<RoomMenuDto>,
    user_info: UserInfoDto,
    pagination: PaginationDto,
    pinned: PinnedDto,
    image_packs: PacksDto,
    attachment: AttachmentDto,
    video: VideoDto,
    audio: Option<NowPlayingDto>,
    unsent: Option<UnsentDto>,
    identity_alert: IdentityAlertDto,
    upgrade: UpgradeDto,
    typing: TypingDto,
    presence: PresenceDto,
    call: CallDto,
    notifications: Vec<NotificationDto>,
    encryption: EncryptionDto,
    account: Option<AccountDto>,
    settings: SettingsDto,
    logout: Option<LogoutDto>,
    toast: ToastDto,
    message_link: LinkDto,
    room_link: LinkDto,
    source: SourceDto,
    readers: ReadersDto,
    mentions: MentionsDto,
    room_log: Option<RoomLogDto>,
}

#[derive(Serialize)]
struct MessageDto {
    kind: &'static str,
    detail: String,
}

#[derive(Serialize)]
struct ToastDto {
    kind: &'static str,
    detail: Option<String>,
}

pub fn view(source: &AppViewState) -> ViewDto {
    ViewDto {
        lifecycle: lifecycle(&source.lifecycle),
        connection: connection(&source.connection),
        directory: directory(&source.directory),
        space_index: space_index(&source.space_index),
        invite: invite_sheet(&source.invite),
        removal: removal_sheet(&source.removal),
        leave_space: leave_space(&source.leave_space),
        room_preview: room_preview(&source.room_preview),
        create_room: create_room(&source.create_room),
        invite_people: invite_people(&source.invite_people),
        room_info: room_info(&source.room_info),
        room_menu: source.room_menu.as_ref().map(room_menu),
        user_info: user_info(&source.user_info),
        pagination: pagination(source.pagination),
        pinned: pinned(&source.pinned),
        image_packs: image_packs(&source.image_packs),
        attachment: attachment(&source.attachment),
        video: video(&source.video),
        audio: audio(&source.audio),
        unsent: source.unsent.as_ref().map(unsent),
        identity_alert: identity_alert(&source.identity_alert),
        upgrade: upgrade(&source.upgrade),
        typing: typing(&source.typing),
        presence: presence(&source.presence),
        call: call(&source.call),
        notifications: notifications(&source.notifications),
        encryption: encryption(&source.encryption),
        account: account(&source.account),
        settings: settings(&source.settings),
        logout: logout(source.logout),
        toast: toast(&source.toast),
        message_link: link(&source.message_link),
        room_link: link(&source.room_link),
        source: event_source(&source.source),
        readers: readers(&source.readers),
        mentions: mentions(&source.mentions),
        room_log: source.room_log.as_ref().map(room_log),
    }
}

fn message(source: &UserMessage) -> MessageDto {
    MessageDto {
        kind: names::user_message_kind(source.kind),
        detail: source.detail.clone(),
    }
}

fn toast(source: &Toast) -> ToastDto {
    match source {
        Toast::None => ToastDto {
            kind: "none",
            detail: None,
        },
        Toast::Error(message) => ToastDto {
            kind: names::user_message_kind(message.kind),
            detail: Some(message.detail.clone()),
        },
        Toast::FileSaved(path) => ToastDto {
            kind: "file-saved",
            detail: Some(path.clone()),
        },
    }
}
