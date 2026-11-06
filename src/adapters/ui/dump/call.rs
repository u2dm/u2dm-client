use serde::Serialize;
use slint::Model;

use crate::adapters::ui::backend::UiBackend;
use crate::adapters::ui::call_video::Host;
use crate::adapters::ui::fields::{CallDeviceFields, CallPersonFields, CallTileFields};
use crate::adapters::ui::props::{BoolProp, EnumProp, IntProp, StringProp, UiProps};
use crate::adapters::ui::{call_video, ringtone};
use crate::domain::call::video::FrameSize;

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct CallDump {
    pub card: bool,
    pub card_room_id: String,
    pub card_caller: String,
    pub card_video: bool,
    pub card_quiet: bool,
    pub ringtone: Option<String>,
    pub pill: bool,
    pub pill_people: i32,
    pub pill_video: bool,
    pub pill_faces: Vec<CallPersonDump>,
    pub band: bool,
    pub dock: bool,
    pub elapsed: String,
    pub notice: String,
    pub failure: String,
    pub failure_room_id: String,
    pub failure_retry: bool,
    pub card_answerable: bool,
    pub card_refusal: String,
    pub sheet: bool,
    pub sheet_room_id: String,
    pub sheet_offer: String,
    pub sheet_microphone: bool,
    pub sheet_camera: bool,
    pub sheet_leaving: String,
    pub sheet_preview: String,
    pub sheet_preview_feed: String,
    pub sheet_preview_call: String,
    pub sheet_preview_frames: u64,
    pub sheet_tests: bool,
    pub sheet_test_kind: String,
    pub sheet_test_phase: String,
    pub sheet_test_level: i32,
    pub stage: Vec<CallTileDump>,
    pub stage_chosen: bool,
    pub view_chosen: bool,
    pub follows_speaker: bool,
    pub tiles: Vec<CallTileDump>,
    pub compact_self: Vec<CallTileDump>,
    pub media_shown: bool,
    pub window: bool,
    pub window_media_shown: bool,
    pub attached: Vec<String>,
    pub self_view: bool,
    pub share_preview: bool,
    pub share_change_cancelled: i32,
    pub share_count: i32,
    pub first_sharer: String,
    pub offered_share: String,
    pub started_share: String,
    pub started_share_serial: i32,
    pub ended_share: String,
    pub ended_share_serial: i32,
    pub room_news: Vec<&'static str>,
    pub people: Vec<CallPersonDump>,
    pub sheet_people: String,
    pub bridges: String,
    pub sheet_bridges: String,
    pub microphones: Vec<CallDeviceDump>,
    pub speakers: Vec<CallDeviceDump>,
    pub cameras: Vec<CallDeviceDump>,
    pub device_news: String,
    pub device_news_name: String,
    pub device_news_serial: i32,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct CallPersonDump {
    pub key: String,
    pub name: String,
    pub has_avatar: bool,
    pub you: bool,
    pub devices: i32,
    pub muted: bool,
    pub sharing: bool,
    pub sees_no_video: bool,
    pub exposes_e2ee: bool,
    pub state: String,
}

#[derive(Serialize)]
pub struct CallDeviceDump {
    pub key: String,
    pub name: String,
    pub status: String,
    pub chosen: bool,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct CallTileDump {
    pub key: String,
    pub member: String,
    pub feed: String,
    pub has_frame: bool,
    pub frame_serial: u64,
    pub frame_width: u32,
    pub frame_height: u32,
    pub source_width: i32,
    pub source_height: i32,
    pub wanted_width: u32,
    pub wanted_height: u32,
    pub own: bool,
    pub has_avatar: bool,
    pub mirrored: bool,
    pub screen: bool,
}

pub(super) fn call_view<B: UiBackend>(window: &B::Window) -> CallDump {
    let here = window.get_string(StringProp::HereRoomId);
    let selected = window.get_string(StringProp::SelectedRoomId);
    let active = window.get_bool(BoolProp::ActiveVisible);
    let failure = window.get_enum(EnumProp::CallFailure).to_string();
    let failed = failure != "none";
    let answerable = window.get_bool(BoolProp::RingAnswerable);
    CallDump {
        card: window.get_bool(BoolProp::RingVisible),
        card_room_id: window.get_string(StringProp::RingRoomId).to_string(),
        card_caller: window.get_string(StringProp::RingCaller).to_string(),
        card_video: window.get_bool(BoolProp::RingVideo),
        card_quiet: window.get_bool(BoolProp::RingQuiet),
        ringtone: ringtone::sounding().map(|tone| format!("{tone:?}")),
        pill: !here.is_empty() && here == selected,
        pill_people: window.get_int(IntProp::HerePeople),
        pill_video: window.get_bool(BoolProp::HereVideo),
        pill_faces: call_people::<B>(&B::models().call_here_faces),
        band: active,
        dock: active || failed,
        elapsed: window.get_string(StringProp::CallElapsed).to_string(),
        notice: window.get_enum(EnumProp::CallNotice).to_string(),
        failure,
        failure_room_id: window.get_string(StringProp::FailureRoomId).to_string(),
        failure_retry: window.get_bool(BoolProp::FailureRetry),
        card_answerable: answerable,
        card_refusal: if answerable {
            String::new()
        } else {
            window.get_enum(EnumProp::RingRefusal).to_string()
        },
        sheet: window.get_bool(BoolProp::SheetVisible),
        sheet_room_id: window.get_string(StringProp::SheetRoomId).to_string(),
        sheet_offer: window.get_enum(EnumProp::CallSheetOffer).to_string(),
        sheet_microphone: window.get_bool(BoolProp::SheetMicrophone),
        sheet_camera: window.get_bool(BoolProp::SheetCamera),
        sheet_leaving: window.get_string(StringProp::SheetLeavingId).to_string(),
        sheet_preview: window.get_enum(EnumProp::CallSheetPreview).to_string(),
        sheet_preview_feed: window.get_string(StringProp::SheetPreviewFeed).to_string(),
        sheet_preview_call: window.get_string(StringProp::SheetPreviewCall).to_string(),
        sheet_tests: window.get_bool(BoolProp::SheetTests),
        sheet_test_kind: window.get_enum(EnumProp::CallSheetTestKind).to_string(),
        sheet_test_phase: window.get_enum(EnumProp::CallSheetTestPhase).to_string(),
        sheet_test_level: window.get_int(IntProp::SheetTestLevel),
        sheet_preview_frames: call_video::frames_of(
            &window.get_string(StringProp::SheetPreviewFeed),
        ),
        stage: call_tiles::<B>(&B::models().call_stage),
        stage_chosen: window.get_bool(BoolProp::StageChosen),
        view_chosen: window.get_bool(BoolProp::ViewChosen),
        follows_speaker: window.get_bool(BoolProp::FollowsSpeaker),
        tiles: call_tiles::<B>(&B::models().call_tiles),
        compact_self: call_tiles::<B>(&B::models().call_self),
        media_shown: call_video::media_is_shown(Host::Main),
        window: window.get_bool(BoolProp::CallWindowed),
        window_media_shown: call_video::media_is_shown(Host::CallWindow),
        attached: call_video::attached(),
        self_view: window.get_bool(BoolProp::SelfView),
        share_preview: window.get_bool(BoolProp::SharePreview),
        share_change_cancelled: window.get_int(IntProp::ShareChangeCancelled),
        share_count: window.get_int(IntProp::ShareCount),
        first_sharer: window.get_string(StringProp::FirstSharer).to_string(),
        offered_share: window.get_string(StringProp::OfferedShare).to_string(),
        started_share: window.get_string(StringProp::StartedShare).to_string(),
        started_share_serial: window.get_int(IntProp::StartedShareSerial),
        ended_share: window.get_string(StringProp::EndedShare).to_string(),
        ended_share_serial: window.get_int(IntProp::EndedShareSerial),
        room_news: [
            (BoolProp::ActiveRoomAlert, "alert"),
            (BoolProp::ActiveRoomMention, "mention"),
            (BoolProp::ActiveRoomHint, "hint"),
        ]
        .into_iter()
        .filter_map(|(prop, news)| window.get_bool(prop).then_some(news))
        .collect(),
        people: call_people::<B>(&B::models().call_people),
        sheet_people: name_line(
            window,
            [StringProp::SheetPeopleFirst, StringProp::SheetPeopleSecond],
            IntProp::SheetPeopleMore,
        ),
        bridges: name_line(
            window,
            [StringProp::BridgesFirst, StringProp::BridgesSecond],
            IntProp::BridgesMore,
        ),
        sheet_bridges: name_line(
            window,
            [
                StringProp::SheetBridgesFirst,
                StringProp::SheetBridgesSecond,
            ],
            IntProp::SheetBridgesMore,
        ),
        microphones: call_devices::<B>(&B::models().call_microphones),
        speakers: call_devices::<B>(&B::models().call_speakers),
        cameras: call_devices::<B>(&B::models().call_cameras),
        device_news: window.get_enum(EnumProp::DeviceNewsTrouble).to_string(),
        device_news_name: window.get_string(StringProp::DeviceNewsName).to_string(),
        device_news_serial: window.get_int(IntProp::DeviceNewsSerial),
    }
}

fn name_line(window: &impl UiProps, [first, second]: [StringProp; 2], more: IntProp) -> String {
    format!(
        "{} | {} | +{}",
        window.get_string(first),
        window.get_string(second),
        window.get_int(more)
    )
}

fn call_devices<B: UiBackend>(rows: &impl Model<Data = B::CallDevice>) -> Vec<CallDeviceDump> {
    rows.iter()
        .map(|row| CallDeviceDump {
            key: row.key().to_owned(),
            name: row.name().to_owned(),
            status: row.status().to_owned(),
            chosen: row.chosen(),
        })
        .collect()
}

fn call_people<B: UiBackend>(rows: &impl Model<Data = B::CallPerson>) -> Vec<CallPersonDump> {
    rows.iter()
        .map(|person| CallPersonDump {
            key: person.key().to_owned(),
            name: person.name().to_owned(),
            has_avatar: person.has_avatar(),
            you: person.you(),
            devices: person.devices(),
            muted: person.muted(),
            sharing: person.sharing(),
            sees_no_video: person.sees_no_video(),
            exposes_e2ee: person.exposes_e2ee(),
            state: person.state().to_owned(),
        })
        .collect()
}

fn call_tiles<B: UiBackend>(rows: &impl Model<Data = B::CallTile>) -> Vec<CallTileDump> {
    rows.iter().map(|tile| call_tile::<B>(&tile)).collect()
}

fn call_tile<B: UiBackend>(tile: &B::CallTile) -> CallTileDump {
    let frame = tile.frame().size();
    let wanted = call_video::wanted_of(tile.feed()).unwrap_or(FrameSize::new(0, 0));
    CallTileDump {
        key: tile.key().to_owned(),
        member: tile.member().to_owned(),
        feed: tile.feed().to_owned(),
        has_frame: tile.has_frame(),
        frame_serial: call_video::frames_of(tile.feed()),
        frame_width: frame.width,
        frame_height: frame.height,
        source_width: tile.source_width(),
        source_height: tile.source_height(),
        wanted_width: wanted.width,
        wanted_height: wanted.height,
        own: tile.own(),
        has_avatar: tile.has_avatar(),
        mirrored: tile.mirrored(),
        screen: tile.screen(),
    }
}
