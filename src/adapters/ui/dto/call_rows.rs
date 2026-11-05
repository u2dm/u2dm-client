use slint::{Image, SharedString};

use crate::adapters::ui::present::{avatar_color_index, name_initials};
use crate::adapters::ui::schema::{
    call_notices, call_offers, call_screens, call_tile_marks, define_ui_enum, sheet_previews,
    test_phases,
};
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{
    CallFace, CallFailure, CallNotice, CallPerson, CallTile, CantDecrypt, DeviceOption,
    DeviceStatus, JoinOffer, LinkView, MissingMedia, ScreenView, ServerDetails, SheetPreview,
    TestPhase, TileKind, TileState,
};
use crate::domain::call::reactions::Reaction as CallReaction;
use crate::domain::call::ring::RingKind;
use crate::domain::call::video::{FrameSize, VideoKind};
use crate::domain::member_name::NamedUser;
use crate::util::saturating_i32;

call_offers!(define_ui_enum CallOfferMark;);

sheet_previews!(define_ui_enum SheetPreviewMark;);

test_phases!(define_ui_enum TestPhaseMark;);

call_screens!(define_ui_enum CallScreenMark;);

call_tile_marks!(define_ui_enum CallTileMark;);

call_notices!(define_ui_enum CallNoticeMark;);

#[allow(clippy::struct_excessive_bools)]
pub struct CallTileDto {
    pub key: SharedString,
    pub member: SharedString,
    pub user_id: SharedString,
    pub name: SharedString,
    pub device: i32,
    pub initial: SharedString,
    pub color_index: i32,
    pub has_avatar: bool,
    pub avatar: Option<Image>,
    pub own: bool,
    pub state: CallTileMark,
    pub muted: bool,
    pub muted_for_you: bool,
    pub speaking: bool,
    pub hand: i32,
    pub reaction: SharedString,
    pub reaction_name: SharedString,
    pub feed: SharedString,
    pub has_frame: bool,
    pub frame: Option<Image>,
    pub source_width: i32,
    pub source_height: i32,
    pub mirrored: bool,
    pub screen: bool,
}

#[allow(clippy::struct_excessive_bools)]
pub struct CallPersonDto {
    pub key: SharedString,
    pub user_id: SharedString,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub has_avatar: bool,
    pub avatar: Option<Image>,
    pub you: bool,
    pub devices: i32,
    pub muted: bool,
    pub muted_for_you: bool,
    pub hand: i32,
    pub reaction: SharedString,
    pub reaction_name: SharedString,
    pub sharing: bool,
    pub sees_no_video: bool,
    pub exposes_e2ee: bool,
    pub state: CallTileMark,
}

#[allow(clippy::struct_excessive_bools)]
pub struct CallConnectionDto {
    pub video: bool,
    pub you: bool,
    pub screen: bool,
    pub host: SharedString,
    pub publishing: bool,
    pub state: LinkView,
    pub sent_kbps: i32,
    pub received_kbps: i32,
    pub loss_permille: i32,
    pub round_trip_ms: i32,
    pub width: i32,
    pub height: i32,
    pub name: SharedString,
}

pub struct CallDeviceDto {
    pub key: SharedString,
    pub name: SharedString,
    pub status: DeviceStatus,
    pub chosen: bool,
}

pub fn call_tile_to_dto(tile: &CallTile) -> CallTileDto {
    CallTileDto {
        key: SharedString::from(tile.key().to_string()),
        member: SharedString::from(tile.member.to_string()),
        user_id: SharedString::from(tile.name.user_id.as_str()),
        name: SharedString::from(tile.name.shown()),
        device: i32::from(tile.device),
        initial: SharedString::from(name_initials(&tile.name.name, &tile.name.user_id)),
        color_index: avatar_color_index(&tile.name.user_id),
        has_avatar: false,
        avatar: None,
        own: tile.own,
        state: call_tile_mark(tile.state),
        muted: tile.muted,
        muted_for_you: tile.muted_for_you,
        speaking: tile.speaking,
        hand: hand_position(tile.hand),
        reaction: reaction_emoji(tile.reaction.as_ref()),
        reaction_name: reaction_name(tile.reaction.as_ref()),
        feed: tile
            .video
            .as_ref()
            .map_or_else(SharedString::new, |feed| SharedString::from(feed.as_ref())),
        has_frame: false,
        frame: None,
        source_width: 0,
        source_height: 0,
        mirrored: tile.is_self_view(),
        screen: tile.kind == TileKind::Screen,
    }
}

fn hand_position(hand: Option<usize>) -> i32 {
    hand.map_or(0, saturating_i32)
}

fn reaction_emoji(reaction: Option<&CallReaction>) -> SharedString {
    reaction.map_or_else(SharedString::new, |reaction| {
        SharedString::from(reaction.emoji.as_str())
    })
}

fn reaction_name(reaction: Option<&CallReaction>) -> SharedString {
    reaction.map_or_else(SharedString::new, |reaction| {
        SharedString::from(reaction.name.as_str())
    })
}

pub fn call_person_to_dto(person: &CallPerson) -> CallPersonDto {
    CallPersonDto {
        key: person.tile.as_ref().map_or_else(SharedString::new, |tile| {
            SharedString::from(tile.to_string())
        }),
        user_id: SharedString::from(person.user.user_id.as_str()),
        name: SharedString::from(person.user.shown()),
        initial: SharedString::from(name_initials(&person.user.name, &person.user.user_id)),
        color_index: avatar_color_index(&person.user.user_id),
        has_avatar: false,
        avatar: None,
        you: person.you,
        devices: saturating_i32(person.devices),
        muted: person.muted,
        muted_for_you: person.muted_for_you,
        hand: hand_position(person.hand),
        reaction: reaction_emoji(person.reaction.as_ref()),
        reaction_name: reaction_name(person.reaction.as_ref()),
        sharing: person.sharing,
        sees_no_video: person.sees_no_video,
        exposes_e2ee: person.exposes_e2ee,
        state: call_tile_mark(person.state),
    }
}

pub fn call_face_to_dto(face: &CallFace) -> CallPersonDto {
    CallPersonDto {
        key: SharedString::new(),
        user_id: SharedString::from(face.user.user_id.as_str()),
        name: SharedString::from(face.user.shown()),
        initial: SharedString::from(name_initials(&face.user.name, &face.user.user_id)),
        color_index: avatar_color_index(&face.user.user_id),
        has_avatar: false,
        avatar: None,
        you: face.you,
        devices: 0,
        muted: false,
        muted_for_you: false,
        hand: 0,
        reaction: SharedString::new(),
        reaction_name: SharedString::new(),
        sharing: false,
        sees_no_video: false,
        exposes_e2ee: false,
        state: CallTileMark::Live,
    }
}

pub fn call_connection_rows(servers: &[ServerDetails]) -> Vec<CallConnectionDto> {
    let known = |value: Option<u32>| value.map_or(-1, saturating_i32);
    let mut rows = Vec::new();
    for server in servers {
        rows.push(CallConnectionDto {
            video: false,
            you: false,
            screen: false,
            host: SharedString::from(&server.host),
            publishing: server.publishing,
            state: server.state,
            sent_kbps: known(server.sent_kbps),
            received_kbps: known(server.received_kbps),
            loss_permille: known(server.loss_permille.map(u32::from)),
            round_trip_ms: known(server.round_trip_ms),
            width: 0,
            height: 0,
            name: SharedString::new(),
        });
        for (kind, size) in &server.sent_video {
            rows.push(video_row(server, *kind, *size, None));
        }
        for (name, kind, size) in &server.received_video {
            rows.push(video_row(server, *kind, *size, Some(name)));
        }
    }
    rows
}

fn video_row(
    server: &ServerDetails,
    kind: VideoKind,
    size: FrameSize,
    sender: Option<&NamedUser>,
) -> CallConnectionDto {
    CallConnectionDto {
        video: true,
        you: sender.is_none(),
        screen: kind == VideoKind::Screen,
        host: SharedString::from(&server.host),
        publishing: server.publishing,
        state: server.state,
        sent_kbps: -1,
        received_kbps: -1,
        loss_permille: -1,
        round_trip_ms: -1,
        width: saturating_i32(size.width),
        height: saturating_i32(size.height),
        name: sender.map_or_else(SharedString::new, |sender| {
            SharedString::from(sender.shown())
        }),
    }
}

pub fn call_device_to_dto(option: &DeviceOption) -> CallDeviceDto {
    CallDeviceDto {
        key: option
            .key
            .as_ref()
            .map_or_else(SharedString::new, |key| SharedString::from(key.as_ref())),
        name: SharedString::from(&option.name),
        status: option.status,
        chosen: option.chosen,
    }
}

const fn call_tile_mark(state: TileState) -> CallTileMark {
    match state {
        TileState::Joining => CallTileMark::Joining,
        TileState::WaitingForKeys => CallTileMark::WaitingForKeys,
        TileState::Live => CallTileMark::Live,
        TileState::PoorConnection => CallTileMark::PoorConnection,
        TileState::VideoStalled => CallTileMark::VideoStalled,
        TileState::NotReceived(MissingMedia::Audio) => CallTileMark::AudioNotReceived,
        TileState::NotReceived(MissingMedia::Video) => CallTileMark::VideoNotReceived,
        TileState::NotReceived(MissingMedia::AudioAndVideo) => CallTileMark::AudioVideoNotReceived,
        TileState::NotReceived(MissingMedia::Screen) => CallTileMark::ScreenNotReceived,
        TileState::NotReceived(MissingMedia::Other) => CallTileMark::MediaNotReceived,
        TileState::ConnectionLost => CallTileMark::ConnectionLost,
        TileState::CantDecrypt(CantDecrypt::NoKey) => CallTileMark::CantDecrypt,
        TileState::CantDecrypt(CantDecrypt::NotCrossSigned) => CallTileMark::UnverifiedDevice,
        TileState::Unreachable => CallTileMark::Unreachable,
    }
}

pub const fn call_screen_mark(screen: ScreenView) -> CallScreenMark {
    match screen {
        ScreenView::Off => CallScreenMark::Off,
        ScreenView::Choosing => CallScreenMark::Choosing,
        ScreenView::On => CallScreenMark::On,
        ScreenView::Failed => CallScreenMark::Failed,
        ScreenView::Unavailable(_) => CallScreenMark::Unavailable,
    }
}

pub const fn call_notice_mark(notice: Option<&CallNotice>) -> CallNoticeMark {
    match notice {
        None => CallNoticeMark::None,
        Some(CallNotice::NotAnnounced(RingKind::Ring)) => CallNoticeMark::NotCalled,
        Some(CallNotice::NotAnnounced(RingKind::Notify)) => CallNoticeMark::NotAnnounced,
        Some(CallNotice::MicrophoneFailed) => CallNoticeMark::MicrophoneFailed,
        Some(CallNotice::CameraFailed) => CallNoticeMark::CameraFailed,
        Some(CallNotice::ScreenFailed) => CallNoticeMark::ScreenFailed,
        Some(CallNotice::ChoosingScreen) => CallNoticeMark::ChoosingScreen,
        Some(CallNotice::SharingNeverStarted) => CallNoticeMark::SharingNeverStarted,
        Some(CallNotice::ConnectionPoor) => CallNoticeMark::ConnectionPoor,
        Some(CallNotice::NotReceived { what, .. }) => match what {
            MissingMedia::Audio => CallNoticeMark::AudioNotReceived,
            MissingMedia::Video => CallNoticeMark::VideoNotReceived,
            MissingMedia::AudioAndVideo => CallNoticeMark::AudioVideoNotReceived,
            MissingMedia::Screen => CallNoticeMark::ScreenNotReceived,
            MissingMedia::Other => CallNoticeMark::MediaNotReceived,
        },
        Some(CallNotice::CantDecrypt {
            why: CantDecrypt::NoKey,
            ..
        }) => CallNoticeMark::CantDecrypt,
        Some(CallNotice::CantDecrypt {
            why: CantDecrypt::NotCrossSigned,
            ..
        }) => CallNoticeMark::UnverifiedDevice,
    }
}

pub const fn call_failure_message(failure: CallFailure) -> UserMessageKind {
    match failure {
        CallFailure::ServerUnreachable => UserMessageKind::CallServerUnreachable,
        CallFailure::ServerRefused => UserMessageKind::CallServerRefused,
        CallFailure::JoinFailed => UserMessageKind::CallJoinFailed,
        CallFailure::MediaFailed => UserMessageKind::CallMediaFailed,
        CallFailure::CutOff => UserMessageKind::CallCutOff,
        CallFailure::NotEncrypted => UserMessageKind::CallNotEncrypted,
    }
}

pub const fn call_offer_mark(offer: Option<JoinOffer>) -> CallOfferMark {
    match offer {
        None => CallOfferMark::None,
        Some(JoinOffer::Start) => CallOfferMark::Start,
        Some(JoinOffer::Join) => CallOfferMark::Join,
        Some(JoinOffer::InCall) => CallOfferMark::InCall,
        Some(JoinOffer::Refused(_)) => CallOfferMark::Refused,
    }
}

pub const fn sheet_preview_mark(preview: Option<&SheetPreview>) -> SheetPreviewMark {
    match preview {
        None => SheetPreviewMark::None,
        Some(SheetPreview::Starting) => SheetPreviewMark::Starting,
        Some(SheetPreview::Local(_)) => SheetPreviewMark::Local,
        Some(SheetPreview::InCall { .. }) => SheetPreviewMark::InCall,
        Some(SheetPreview::Elsewhere { .. }) => SheetPreviewMark::Elsewhere,
        Some(SheetPreview::Failed(_)) => SheetPreviewMark::Failed,
    }
}

pub const fn test_phase_mark(phase: Option<TestPhase>) -> TestPhaseMark {
    match phase {
        None => TestPhaseMark::None,
        Some(TestPhase::Starting) => TestPhaseMark::Starting,
        Some(TestPhase::Running) => TestPhaseMark::Running,
        Some(TestPhase::Failed) => TestPhaseMark::Failed,
        Some(TestPhase::Lost) => TestPhaseMark::Lost,
    }
}
