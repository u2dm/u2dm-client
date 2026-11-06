mod sheet;

use serde::Serialize;

use self::sheet::{
    CallOfferDto, CallSheetDto, call_sheet, offer_refusal, offer_word, refusal_word,
};
use crate::adapters::demo::probe::names;
use crate::commands::view::{
    ActiveCall, CallDevices, CallFace, CallFailure, CallNotice, CallPerson, CallPhase, CallTile,
    CallView, CameraView, CantDecrypt, DeviceOption, HandView, LinkView, MicrophoneView,
    MissingMedia, ReactionView, ScreenView, ServerDetails, ShareNews, TileKind, TileState,
};
use crate::domain::call::reactions::Reaction;
use crate::domain::call::ring::RingKind;
use crate::domain::call::video::{FrameSize, ScreenRefusal, VideoKind};
use crate::domain::call::{CallIntent, RoomCall};
use crate::domain::member_name::NamedUser;
use crate::util::unix_now_ms;

#[derive(Serialize)]
pub(super) struct CallDto {
    ring: Option<RingDto>,
    here: Option<CallHereDto>,
    offer: Option<CallOfferDto>,
    sheet: Option<CallSheetDto>,
    active: Option<ActiveCallDto>,
    failed: Option<FailedCallDto>,
}

#[derive(Serialize)]
struct FailedCallDto {
    room_id: String,
    room_name: String,
    failure: &'static str,
    retry: bool,
}

#[derive(Serialize)]
struct CallNoticeDto {
    kind: &'static str,
    who: Option<String>,
    others: usize,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct ActiveCallDto {
    call: u64,
    room_id: String,
    room_alert: bool,
    room_mention: bool,
    room_hint: bool,
    phase: &'static str,
    live: bool,
    encrypted: bool,
    video: bool,
    microphone: &'static str,
    microphone_level: Option<u8>,
    camera: &'static str,
    screen: &'static str,
    screen_refusal: Option<&'static str>,
    hand: &'static str,
    hands_raised: usize,
    hand_news: Option<ShareNewsDto>,
    reaction: &'static str,
    reaction_news: Option<ReactionNewsDto>,
    share_preview: bool,
    share_change_cancelled: u64,
    notice: Option<CallNoticeDto>,
    stage: Option<CallTileDto>,
    stage_chosen: bool,
    view_chosen: bool,
    tiles: Vec<CallTileDto>,
    compact_self: Option<CallTileDto>,
    self_view: bool,
    follows_speaker: bool,
    faces_ready: u64,
    shares: Vec<String>,
    offered_share: Option<String>,
    started_share: Option<ShareNewsDto>,
    ended_share: Option<ShareNewsDto>,
    people: Vec<CallPersonDto>,
    devices: CallDevicesDto,
    connection: Option<Vec<ServerDetailsDto>>,
}

#[derive(Serialize)]
struct ServerDetailsDto {
    host: String,
    publishing: bool,
    state: &'static str,
    sent_kbps: Option<u32>,
    received_kbps: Option<u32>,
    loss_permille: Option<u16>,
    round_trip_ms: Option<u32>,
    sent_video: Vec<String>,
    received_video: Vec<String>,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct CallPersonDto {
    user_id: String,
    name: String,
    avatar: Option<String>,
    you: bool,
    devices: usize,
    muted: bool,
    muted_for_you: bool,
    hand: Option<usize>,
    reaction: Option<ReactionDto>,
    sharing: bool,
    sees_no_video: bool,
    exposes_e2ee: bool,
    state: &'static str,
    tile: Option<String>,
}

#[derive(Serialize)]
struct CallDevicesDto {
    microphones: Vec<DeviceOptionDto>,
    speakers: Vec<DeviceOptionDto>,
    cameras: Vec<DeviceOptionDto>,
    news: Option<DeviceNewsDto>,
}

#[derive(Serialize)]
struct DeviceOptionDto {
    key: Option<String>,
    name: String,
    status: &'static str,
    chosen: bool,
}

#[derive(Serialize)]
struct DeviceNewsDto {
    serial: u64,
    kind: &'static str,
    name: String,
    trouble: &'static str,
}

#[derive(Serialize)]
struct ShareNewsDto {
    serial: u64,
    who: String,
}

#[derive(Serialize)]
struct ReactionDto {
    emoji: String,
    name: String,
}

#[derive(Serialize)]
struct ReactionNewsDto {
    serial: u64,
    who: String,
    emoji: String,
    name: String,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct CallTileDto {
    key: String,
    member: String,
    name: String,
    device: u8,
    avatar: Option<String>,
    own: bool,
    kind: &'static str,
    state: &'static str,
    muted: bool,
    muted_for_you: bool,
    speaking: bool,
    hand: Option<usize>,
    reaction: Option<ReactionDto>,
    video: Option<String>,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct RingDto {
    serial: u64,
    room_id: String,
    room_name: String,
    direct: bool,
    caller: String,
    caller_id: String,
    video: bool,
    quiet: bool,
    desktop: bool,
    answerable: bool,
    refusal: Option<&'static str>,
    devices: Option<CallDevicesDto>,
}

#[derive(Serialize)]
struct CallHereDto {
    room_id: String,
    people: usize,
    faces: Vec<CallFaceDto>,
    faces_ready: u64,
    video: bool,
    joined_by_you: bool,
}

#[derive(Serialize)]
struct CallFaceDto {
    user_id: String,
    name: String,
    avatar: Option<String>,
    you: bool,
}

#[derive(Serialize)]
pub(super) struct RoomCallDto {
    participants: Vec<CallParticipantDto>,
    intent: Option<&'static str>,
    expires_in_s: Option<u64>,
}

#[derive(Serialize)]
struct CallParticipantDto {
    user_id: String,
    device_id: String,
    name: String,
}

pub(super) fn call(source: &CallView) -> CallDto {
    CallDto {
        ring: source.ring.as_ref().map(|ring| RingDto {
            serial: ring.serial,
            room_id: ring.room_id.to_string(),
            room_name: ring.room_name.clone(),
            direct: ring.direct,
            caller: ring.caller.shown(),
            caller_id: ring.caller.user_id.clone(),
            video: ring.video,
            quiet: ring.quiet,
            desktop: ring.desktop,
            answerable: ring.answerable,
            refusal: ring.refusal.map(refusal_word),
            devices: ring.devices.as_ref().map(call_devices),
        }),
        here: source.here.as_ref().map(|here| CallHereDto {
            room_id: here.room_id.to_string(),
            people: here.people,
            faces: here.faces.iter().map(call_face).collect(),
            faces_ready: here.faces_ready,
            video: here.video,
            joined_by_you: here.joined_by_you,
        }),
        offer: source.offer.as_ref().map(|offer| CallOfferDto {
            room_id: offer.room_id.to_string(),
            offer: offer_word(offer.offer),
            refusal: offer_refusal(offer.offer),
        }),
        sheet: source.sheet.as_ref().map(call_sheet),
        active: source.active.as_ref().map(active_call),
        failed: source.failed.as_ref().map(|failed| FailedCallDto {
            room_id: failed.room_id.to_string(),
            room_name: failed.room_name.clone(),
            failure: match failed.failure {
                CallFailure::ServerUnreachable => "server-unreachable",
                CallFailure::ServerRefused => "server-refused",
                CallFailure::JoinFailed => "join-failed",
                CallFailure::MediaFailed => "media-failed",
                CallFailure::CutOff => "cut-off",
                CallFailure::NotEncrypted => "not-encrypted",
            },
            retry: failed.retry,
        }),
    }
}

fn active_call(active: &ActiveCall) -> ActiveCallDto {
    ActiveCallDto {
        call: active.call,
        room_id: active.room_id.to_string(),
        room_alert: active.room_unread.alert,
        room_mention: active.room_unread.mention,
        room_hint: active.room_unread.hint,
        phase: match active.phase {
            CallPhase::Joining => "joining",
            CallPhase::Calling => "calling",
            CallPhase::Connecting => "connecting",
            CallPhase::Waiting => "waiting",
            CallPhase::Live => "live",
            CallPhase::Reconnecting => "reconnecting",
        },
        live: active.live_since_ms.is_some(),
        encrypted: active.encrypted,
        video: active.video,
        microphone: match active.microphone {
            MicrophoneView::On => "on",
            MicrophoneView::Off => "off",
            MicrophoneView::Starting => "starting",
            MicrophoneView::Failed => "failed",
        },
        microphone_level: active.microphone_level,
        camera: match active.camera {
            CameraView::Off => "off",
            CameraView::Starting => "starting",
            CameraView::On => "on",
            CameraView::Failed => "failed",
        },
        screen: match active.screen {
            ScreenView::Off => "off",
            ScreenView::Choosing => "choosing",
            ScreenView::On => "on",
            ScreenView::Failed => "failed",
            ScreenView::Unavailable(_) => "unavailable",
        },
        screen_refusal: match active.screen {
            ScreenView::Unavailable(ScreenRefusal::NotWayland) => Some("not-wayland"),
            ScreenView::Unavailable(ScreenRefusal::NoPortal) => Some("no-portal"),
            _ => None,
        },
        hand: match active.hand {
            HandView::Unavailable => "unavailable",
            HandView::Down => "down",
            HandView::Raising => "raising",
            HandView::Up => "up",
            HandView::Lowering => "lowering",
            HandView::RaiseFailed => "raise-failed",
            HandView::LowerFailed => "lower-failed",
        },
        hands_raised: active.hands_raised,
        hand_news: active.hand_news.as_ref().map(|news| ShareNewsDto {
            serial: news.serial,
            who: news.who.shown(),
        }),
        reaction: match active.reaction {
            ReactionView::Unavailable => "unavailable",
            ReactionView::Ready => "ready",
            ReactionView::Sending => "sending",
            ReactionView::Showing => "showing",
            ReactionView::Failed => "failed",
        },
        reaction_news: active.reaction_news.as_ref().map(|news| ReactionNewsDto {
            serial: news.serial,
            who: news.who.shown(),
            emoji: news.reaction.emoji.clone(),
            name: news.reaction.name.clone(),
        }),
        share_preview: active.share_preview,
        share_change_cancelled: active.share_change_cancelled,
        notice: active.notice.as_ref().map(call_notice),
        stage: active.stage.as_ref().map(call_tile),
        stage_chosen: active.stage_chosen,
        view_chosen: active.view_chosen,
        tiles: active.tiles.iter().map(call_tile).collect(),
        compact_self: active.compact_self.as_ref().map(call_tile),
        self_view: active.self_view,
        follows_speaker: active.follows_speaker,
        faces_ready: active.faces_ready,
        shares: active.shares.iter().map(NamedUser::shown).collect(),
        offered_share: active.offered_share.as_ref().map(NamedUser::shown),
        started_share: active.started_share.as_ref().map(share_news),
        ended_share: active.ended_share.as_ref().map(share_news),
        people: active.people.iter().map(call_person).collect(),
        devices: call_devices(&active.devices),
        connection: active
            .connection
            .as_ref()
            .map(|servers| servers.iter().map(server_details).collect()),
    }
}

fn server_details(server: &ServerDetails) -> ServerDetailsDto {
    let size = |size: FrameSize| format!("{}x{}", size.width, size.height);
    let kind = |kind: &VideoKind| match kind {
        VideoKind::Camera => "camera",
        VideoKind::Screen => "screen",
    };
    ServerDetailsDto {
        host: server.host.clone(),
        publishing: server.publishing,
        state: match server.state {
            LinkView::Connecting => "connecting",
            LinkView::Connected => "connected",
            LinkView::Reconnecting => "reconnecting",
            LinkView::Failed => "failed",
        },
        sent_kbps: server.sent_kbps,
        received_kbps: server.received_kbps,
        loss_permille: server.loss_permille,
        round_trip_ms: server.round_trip_ms,
        sent_video: server
            .sent_video
            .iter()
            .map(|(video, sent)| format!("{} {}", kind(video), size(*sent)))
            .collect(),
        received_video: server
            .received_video
            .iter()
            .map(|(name, video, received)| {
                format!("{} {} {}", name.shown(), kind(video), size(*received))
            })
            .collect(),
    }
}

fn call_devices(devices: &CallDevices) -> CallDevicesDto {
    let options = |options: &[DeviceOption]| -> Vec<DeviceOptionDto> {
        options
            .iter()
            .map(|option| DeviceOptionDto {
                key: option.key.as_ref().map(ToString::to_string),
                name: option.name.clone(),
                status: names::call_device_status(option.status),
                chosen: option.chosen,
            })
            .collect()
    };
    CallDevicesDto {
        microphones: options(&devices.options.microphone),
        speakers: options(&devices.options.speaker),
        cameras: options(&devices.options.camera),
        news: devices.news.as_ref().map(|news| DeviceNewsDto {
            serial: news.serial,
            kind: names::call_device_kind(news.kind),
            name: news.name.clone(),
            trouble: names::call_device_trouble(news.trouble),
        }),
    }
}

fn call_notice(notice: &CallNotice) -> CallNoticeDto {
    let (kind, who, others) = match notice {
        CallNotice::NotAnnounced(RingKind::Ring) => ("not-called", None, 0),
        CallNotice::NotAnnounced(RingKind::Notify) => ("not-announced", None, 0),
        CallNotice::MicrophoneFailed => ("microphone-failed", None, 0),
        CallNotice::CameraFailed => ("camera-failed", None, 0),
        CallNotice::ScreenFailed => ("screen-failed", None, 0),
        CallNotice::ChoosingScreen => ("choosing-screen", None, 0),
        CallNotice::SharingNeverStarted => ("sharing-never-started", None, 0),
        CallNotice::ConnectionPoor => ("connection-poor", None, 0),
        CallNotice::NotReceived { who, what, others } => {
            (not_received_word(*what), Some(who.shown()), *others)
        }
        CallNotice::CantDecrypt { who, why, others } => (
            match why {
                CantDecrypt::NoKey => "cant-decrypt",
                CantDecrypt::NotCrossSigned => "unverified-device",
            },
            Some(who.shown()),
            *others,
        ),
    };
    CallNoticeDto { kind, who, others }
}

fn reaction_dto(reaction: &Reaction) -> ReactionDto {
    ReactionDto {
        emoji: reaction.emoji.clone(),
        name: reaction.name.clone(),
    }
}

fn call_tile(tile: &CallTile) -> CallTileDto {
    CallTileDto {
        key: tile.key().to_string(),
        member: tile.member.to_string(),
        name: tile.name.shown(),
        device: tile.device,
        avatar: tile.avatar_mxc.clone(),
        own: tile.own,
        kind: match tile.kind {
            TileKind::Person => "person",
            TileKind::Screen => "screen",
        },
        state: tile_state_word(tile.state),
        muted: tile.muted,
        muted_for_you: tile.muted_for_you,
        speaking: tile.speaking,
        hand: tile.hand,
        reaction: tile.reaction.as_ref().map(reaction_dto),
        video: tile.video.as_ref().map(ToString::to_string),
    }
}

fn share_news(news: &ShareNews) -> ShareNewsDto {
    ShareNewsDto {
        serial: news.serial,
        who: news.who.shown(),
    }
}

fn call_face(face: &CallFace) -> CallFaceDto {
    CallFaceDto {
        user_id: face.user.user_id.clone(),
        name: face.user.shown(),
        avatar: face.avatar_mxc.clone(),
        you: face.you,
    }
}

fn call_person(person: &CallPerson) -> CallPersonDto {
    CallPersonDto {
        user_id: person.user.user_id.clone(),
        name: person.user.shown(),
        avatar: person.avatar_mxc.clone(),
        you: person.you,
        devices: person.devices,
        muted: person.muted,
        muted_for_you: person.muted_for_you,
        hand: person.hand,
        reaction: person.reaction.as_ref().map(reaction_dto),
        sharing: person.sharing,
        sees_no_video: person.sees_no_video,
        exposes_e2ee: person.exposes_e2ee,
        state: tile_state_word(person.state),
        tile: person.tile.as_ref().map(ToString::to_string),
    }
}

const fn not_received_word(what: MissingMedia) -> &'static str {
    match what {
        MissingMedia::Audio => "audio-not-received",
        MissingMedia::Video => "video-not-received",
        MissingMedia::AudioAndVideo => "audio-video-not-received",
        MissingMedia::Screen => "screen-not-received",
        MissingMedia::Other => "media-not-received",
    }
}

const fn tile_state_word(state: TileState) -> &'static str {
    match state {
        TileState::Joining => "joining",
        TileState::WaitingForKeys => "waiting-for-keys",
        TileState::Live => "live",
        TileState::PoorConnection => "poor-connection",
        TileState::VideoStalled => "video-stalled",
        TileState::NotReceived(what) => not_received_word(what),
        TileState::ConnectionLost => "connection-lost",
        TileState::CantDecrypt(CantDecrypt::NoKey) => "cant-decrypt",
        TileState::CantDecrypt(CantDecrypt::NotCrossSigned) => "unverified-device",
        TileState::Unreachable => "unreachable",
    }
}

pub(super) fn room_call(call: &RoomCall) -> RoomCallDto {
    let now = unix_now_ms();
    RoomCallDto {
        participants: call
            .members()
            .iter()
            .map(|member| CallParticipantDto {
                user_id: member.user.user_id.clone(),
                device_id: member.device.to_string(),
                name: member.user.shown(),
            })
            .collect(),
        intent: call.intent().map(|intent| match intent {
            CallIntent::Voice => "voice",
            CallIntent::Video => "video",
        }),
        expires_in_s: call
            .next_expiry_ms()
            .map(|expiry| expiry.saturating_sub(now) / 1000),
    }
}
