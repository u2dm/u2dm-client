use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use slint::{ComponentHandle, LogicalSize, Model};
use tokio::sync::oneshot;

use super::activity;
use super::audio;
use super::backend::UiBackend;
use super::call_video::Host;
use super::fields::{
    BodyLineFields, BodyPieceFields, CallDeviceFields, CallPersonFields, CallTileFields,
    LogLineFields, MessageFields, PollAnswerFields, ReactionFields,
};
use super::props::{BoolProp, EnumProp, IntProp, StringProp, UiProps};
use super::ringtone;
use super::{call_video, call_window, context_press, touchpad_pinch};
use crate::domain::call::video::FrameSize;

#[derive(Serialize)]
pub struct ReactionRowDump {
    pub key: String,
    pub label: String,
    pub art: String,
    pub count: i32,
    pub mine: bool,
    pub send: String,
    pub overflow: bool,
    pub reactors: String,
    pub hidden_reactors: i32,
}

#[derive(Serialize)]
pub struct BodyEmojiDump {
    pub line: usize,
    pub key: String,
    pub art: String,
}

#[derive(Serialize)]
pub struct PollAnswerRowDump {
    pub id: String,
    pub label: String,
    pub count: i32,
    pub share: f32,
    pub mine: bool,
    pub leading: bool,
    pub votable: bool,
    pub voters: String,
    pub hidden_voters: i32,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct TimelineRowDump {
    pub row: usize,
    pub unique_id: String,
    pub local_id: String,
    pub event_id: String,
    pub sender: String,
    pub sender_id: String,
    pub body: String,
    pub has_links: bool,
    pub mentions_you: bool,
    pub emote_only: bool,
    pub spoilers: String,
    pub preview_body: String,
    pub body_lines: Vec<usize>,
    pub body_emoji: Vec<BodyEmojiDump>,
    pub timestamp: String,
    pub sent_at: String,
    pub message_type: String,
    pub preview_kind: String,
    pub service_kind: String,
    pub service_target: String,
    pub service_reason: String,
    pub media_state: String,
    pub media_failure: String,
    pub send_state: String,
    pub send_progress: f32,
    pub delivery: String,
    pub shield: String,
    pub shield_reason: String,
    pub decryption_failure: String,
    pub readers: String,
    pub hidden_readers: i32,
    pub reader_count: i32,
    pub is_own: bool,
    pub edited: bool,
    pub text_editable: bool,
    pub edit_text: String,
    pub pinned: bool,
    pub first_unread: bool,
    pub needs_media: bool,
    pub has_avatar: bool,
    pub image_width: i32,
    pub image_height: i32,
    pub duration: String,
    pub filename: String,
    pub size: String,
    pub map_link: String,
    pub unsupported_kind: String,
    pub audio_kind: String,
    pub waveform: Vec<f32>,
    pub has_reply: bool,
    pub reply_event_id: String,
    pub reply_sender: String,
    pub reply_body: String,
    pub reactions: Vec<ReactionRowDump>,
    pub poll_phase: String,
    pub poll_editable: bool,
    pub poll_choices: i32,
    pub poll_voters: i32,
    pub poll_answers: Vec<PollAnswerRowDump>,
}

#[derive(Serialize)]
pub struct AudioDump {
    pub state: &'static str,
    pub output: &'static str,
    pub event_id: String,
    pub room_id: String,
    pub kind: String,
    pub sender: String,
    pub title: String,
    pub position_ms: i32,
    pub duration_ms: i32,
}

#[derive(Serialize)]
pub struct PinnedDump {
    pub count: i32,
    pub index: i32,
    pub event_id: String,
    pub kind: String,
    pub body: String,
}

#[derive(Serialize)]
pub struct PollPermissionsDump {
    pub vote: bool,
    pub end: bool,
    pub start: bool,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct MessagePermissionsDump {
    pub send_messages: bool,
    pub react: bool,
    pub send_stickers: bool,
    pub delete_own: bool,
    pub delete_others: bool,
    pub pin: bool,
}

#[derive(Serialize)]
pub struct RoomMenuDump {
    pub key: String,
    pub room_id: String,
    pub shown: bool,
}

#[derive(Serialize)]
pub struct RoomLogDump {
    pub visible: bool,
    pub room_id: String,
    pub name: String,
    pub rows: usize,
    pub dropped: i32,
    pub lines_landed: i32,
    pub newest: Vec<String>,
}

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

#[derive(Serialize)]
pub struct TimelineDump {
    pub selected_room_id: String,
    pub selected_room_name: String,
    pub selected_room_encrypted: bool,
    pub selected_room_readable_by_anyone: bool,
    pub poll_permissions: PollPermissionsDump,
    pub message_permissions: MessagePermissionsDump,
    pub generation: i32,
    pub timeline_token: i32,
    pub prepend_token: i32,
    pub anchor_index: i32,
    pub focus_event_id: String,
    pub pinned: PinnedDump,
    pub rows: Vec<TimelineRowDump>,
    pub audio: AudioDump,
    pub room_menu: RoomMenuDump,
    pub room_log: RoomLogDump,
    pub call: CallDump,
    pub title: String,
}

type Requester = Box<dyn Fn(oneshot::Sender<TimelineDump>) + Send + Sync>;

static REQUESTER: OnceLock<Requester> = OnceLock::new();

fn install(requester: Requester) {
    if REQUESTER.set(requester).is_err() {
        tracing::debug!("the timeline dump was already installed");
    }
}

#[derive(Clone, Copy)]
pub enum Poke {
    ToggleAudio,
    SeekAudio(Duration),
    WindowFocus(bool),
    WindowSize(f32, f32),
    UserActivity(bool),
    SwipeTravel(i32),
    TouchpadPinch(i32),
    ContextPress,
    CloseCallWindow,
}

type Poker = Box<dyn Fn(Poke) + Send + Sync>;

static POKER: OnceLock<Poker> = OnceLock::new();

fn install_pokes(poker: Poker) {
    if POKER.set(poker).is_err() {
        tracing::debug!("the probe pokes were already installed");
    }
}

pub fn poke(poke: Poke) -> bool {
    POKER.get().is_some_and(|poker| {
        poker(poke);
        true
    })
}

pub fn request() -> Option<oneshot::Receiver<TimelineDump>> {
    let requester = REQUESTER.get()?;
    let (tx, rx) = oneshot::channel();
    requester(tx);
    Some(rx)
}

pub fn install_probe<B: UiBackend>(window: &B::Window) {
    let weak = window.as_weak();
    let dump_weak = weak.clone();
    install(Box::new(move |reply| {
        let queued = dump_weak.upgrade_in_event_loop(move |window| {
            drop(reply.send(collect::<B>(&window)));
        });
        if let Err(e) = queued {
            tracing::debug!("the timeline dump could not reach the event loop: {e}");
        }
    }));
    install_pokes(Box::new(move |poke| {
        let queued = weak.upgrade_in_event_loop(move |window| match poke {
            Poke::ToggleAudio => audio::toggle(&window),
            Poke::SeekAudio(position) => audio::seek(&window, position),
            Poke::WindowFocus(focused) => {
                window.set_bool(BoolProp::WindowFocused, focused);
                window.window().request_redraw();
            }
            Poke::WindowSize(width, height) => {
                window.window().set_size(LogicalSize::new(width, height));
            }
            Poke::UserActivity(active) => activity::report(&window, active),
            Poke::SwipeTravel(px) => window.set_int(IntProp::SwipeTravel, px),
            Poke::TouchpadPinch(permille) => touchpad_pinch::publish::<B>(&window, permille),
            Poke::ContextPress => context_press::announce(&window),
            Poke::CloseCallWindow => call_window::request_close::<B>(),
        });
        if let Err(e) = queued {
            tracing::debug!("a probe poke could not reach the event loop: {e}");
        }
    }));
}

fn collect<B: UiBackend>(window: &B::Window) -> TimelineDump {
    let rows = B::with_timeline(|model| {
        model
            .iter()
            .enumerate()
            .map(|(index, entry)| timeline_row::<B>(index, &entry))
            .collect()
    });
    TimelineDump {
        selected_room_id: window.get_string(StringProp::SelectedRoomId).to_string(),
        selected_room_name: window.get_string(StringProp::SelectedRoomName).to_string(),
        selected_room_encrypted: window.get_bool(BoolProp::SelectedRoomEncrypted),
        selected_room_readable_by_anyone: window.get_bool(BoolProp::SelectedRoomReadableByAnyone),
        poll_permissions: PollPermissionsDump {
            vote: window.get_bool(BoolProp::MayVote),
            end: window.get_bool(BoolProp::MayEndPolls),
            start: window.get_bool(BoolProp::MayStartPolls),
        },
        message_permissions: MessagePermissionsDump {
            send_messages: window.get_bool(BoolProp::MaySendMessages),
            react: window.get_bool(BoolProp::MayReact),
            send_stickers: window.get_bool(BoolProp::MaySendStickers),
            delete_own: window.get_bool(BoolProp::MayDeleteOwn),
            delete_others: window.get_bool(BoolProp::MayDeleteOthers),
            pin: window.get_bool(BoolProp::MayPin),
        },
        generation: window.get_int(IntProp::SelectedGeneration),
        timeline_token: window.get_int(IntProp::TimelineToken),
        prepend_token: window.get_int(IntProp::PrependToken),
        anchor_index: window.get_int(IntProp::AnchorIndex),
        focus_event_id: window.get_string(StringProp::FocusEventId).to_string(),
        pinned: pinned_view(window),
        rows,
        audio: audio_view(window),
        room_menu: room_menu_view(window),
        room_log: room_log_view::<B>(window),
        call: call_view::<B>(window),
        title: window.get_string(StringProp::WindowTitleText).to_string(),
    }
}

const NEWEST_LOG_ROWS: usize = 12;

fn room_log_view<B: UiBackend>(window: &B::Window) -> RoomLogDump {
    let lines = Rc::clone(&B::models().room_log);
    let rows = lines.row_count();
    let newest = lines
        .iter()
        .skip(rows.saturating_sub(NEWEST_LOG_ROWS))
        .map(|line| {
            format!(
                "{} {} {}: {}",
                line.time(),
                line.level_name(),
                line.target(),
                line.text()
            )
        })
        .collect();
    RoomLogDump {
        visible: window.get_bool(BoolProp::RoomLogVisible),
        room_id: window.get_string(StringProp::RoomLogRoomId).to_string(),
        name: window.get_string(StringProp::RoomLogName).to_string(),
        rows,
        dropped: window.get_int(IntProp::RoomLogDropped),
        lines_landed: window.get_int(IntProp::RoomLogLinesLanded),
        newest,
    }
}

fn call_view<B: UiBackend>(window: &B::Window) -> CallDump {
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
        sheet_people: format!(
            "{} | {} | +{}",
            window.get_string(StringProp::SheetPeopleFirst),
            window.get_string(StringProp::SheetPeopleSecond),
            window.get_int(IntProp::SheetPeopleMore)
        ),
        bridges: format!(
            "{} | {} | +{}",
            window.get_string(StringProp::BridgesFirst),
            window.get_string(StringProp::BridgesSecond),
            window.get_int(IntProp::BridgesMore)
        ),
        sheet_bridges: format!(
            "{} | {} | +{}",
            window.get_string(StringProp::SheetBridgesFirst),
            window.get_string(StringProp::SheetBridgesSecond),
            window.get_int(IntProp::SheetBridgesMore)
        ),
        microphones: call_devices::<B>(&B::models().call_microphones),
        speakers: call_devices::<B>(&B::models().call_speakers),
        cameras: call_devices::<B>(&B::models().call_cameras),
        device_news: window.get_enum(EnumProp::DeviceNewsTrouble).to_string(),
        device_news_name: window.get_string(StringProp::DeviceNewsName).to_string(),
        device_news_serial: window.get_int(IntProp::DeviceNewsSerial),
    }
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

fn room_menu_view(window: &impl UiProps) -> RoomMenuDump {
    let key = window.get_string(StringProp::RoomMenuKey).to_string();
    let room_id = window.get_string(StringProp::RoomMenuRoomId).to_string();
    RoomMenuDump {
        shown: !key.is_empty() && key == room_id,
        key,
        room_id,
    }
}

fn pinned_view(window: &impl UiProps) -> PinnedDump {
    PinnedDump {
        count: window.get_int(IntProp::PinnedCount),
        index: window.get_int(IntProp::PinnedIndex),
        event_id: window.get_string(StringProp::PinnedEventId).to_string(),
        kind: window.get_enum(EnumProp::PinnedKind).to_string(),
        body: window.get_string(StringProp::PinnedBody).to_string(),
    }
}

fn timeline_row<B: UiBackend>(row: usize, entry: &B::Message) -> TimelineRowDump {
    TimelineRowDump {
        row,
        unique_id: entry.unique_id().to_owned(),
        local_id: entry.local_id().to_owned(),
        event_id: entry.event_id().to_owned(),
        sender: entry.sender().to_owned(),
        sender_id: entry.sender_id().to_owned(),
        body: entry.body().to_owned(),
        has_links: entry.has_links(),
        mentions_you: entry.mentions_you(),
        emote_only: entry.emote_only(),
        spoilers: entry.spoilers().to_owned(),
        preview_body: entry.preview_body().to_owned(),
        body_lines: entry
            .body_lines()
            .iter()
            .map(|line| line.pieces().row_count())
            .collect(),
        body_emoji: body_emoji::<B>(entry),
        timestamp: entry.timestamp().to_owned(),
        sent_at: entry.sent_at().to_owned(),
        message_type: entry.message_type().to_owned(),
        preview_kind: entry.preview_kind().to_owned(),
        service_kind: entry.service_kind().to_owned(),
        service_target: entry.service_target().to_owned(),
        service_reason: entry.service_reason().to_owned(),
        media_state: entry.media_state().to_owned(),
        media_failure: entry.media_failure().to_owned(),
        send_state: entry.send_state().to_owned(),
        send_progress: entry.send_progress(),
        delivery: entry.delivery().to_owned(),
        shield: entry.shield().to_owned(),
        shield_reason: entry.shield_reason().to_owned(),
        decryption_failure: entry.decryption_failure().to_owned(),
        readers: entry.readers().to_owned(),
        hidden_readers: entry.hidden_readers(),
        reader_count: entry.reader_count(),
        is_own: entry.is_own(),
        edited: entry.edited(),
        text_editable: entry.text_editable(),
        edit_text: entry.edit_text().to_owned(),
        pinned: entry.pinned(),
        first_unread: entry.first_unread(),
        needs_media: entry.needs_media(),
        has_avatar: entry.has_avatar(),
        image_width: entry.image_width(),
        image_height: entry.image_height(),
        duration: entry.duration().to_owned(),
        filename: entry.filename().to_owned(),
        size: entry.size().to_owned(),
        map_link: entry.map_link().to_owned(),
        unsupported_kind: entry.unsupported_kind().to_owned(),
        audio_kind: entry.audio_kind().to_owned(),
        waveform: entry.waveform(),
        has_reply: entry.has_reply(),
        reply_event_id: entry.reply_event_id().to_owned(),
        reply_sender: entry.reply_sender().to_owned(),
        reply_body: entry.reply_body().to_owned(),
        reactions: entry
            .reactions()
            .iter()
            .map(|reaction| reaction_row::<B>(&reaction))
            .collect(),
        poll_phase: entry.poll_phase().to_owned(),
        poll_editable: entry.poll_editable(),
        poll_choices: entry.poll_choices(),
        poll_voters: entry.poll_voters(),
        poll_answers: entry
            .poll_answers()
            .iter()
            .map(|answer| poll_answer_row::<B>(&answer))
            .collect(),
    }
}

fn body_emoji<B: UiBackend>(entry: &B::Message) -> Vec<BodyEmojiDump> {
    entry
        .body_lines()
        .iter()
        .enumerate()
        .flat_map(|(index, line)| {
            line.pieces()
                .iter()
                .filter(|piece| !piece.key().is_empty())
                .map(|piece| BodyEmojiDump {
                    line: index,
                    key: piece.key().to_owned(),
                    art: piece.art().to_owned(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn poll_answer_row<B: UiBackend>(entry: &B::PollAnswer) -> PollAnswerRowDump {
    PollAnswerRowDump {
        id: entry.id().to_owned(),
        label: entry.label().to_owned(),
        count: entry.count(),
        share: entry.share(),
        mine: entry.mine(),
        leading: entry.leading(),
        votable: entry.votable(),
        voters: entry.voters().to_owned(),
        hidden_voters: entry.hidden_voters(),
    }
}

fn reaction_row<B: UiBackend>(entry: &B::Reaction) -> ReactionRowDump {
    ReactionRowDump {
        key: entry.key().to_owned(),
        label: entry.label().to_owned(),
        art: entry.art().to_owned(),
        count: entry.count(),
        mine: entry.mine(),
        send: entry.send().to_owned(),
        overflow: entry.overflow(),
        reactors: entry.reactors().to_owned(),
        hidden_reactors: entry.hidden_reactors(),
    }
}

fn audio_view(window: &impl UiProps) -> AudioDump {
    let state = match (
        window.get_bool(BoolProp::AudioVisible),
        window.get_bool(BoolProp::AudioLoading),
        window.get_bool(BoolProp::AudioPlaying),
    ) {
        (false, _, _) => "hidden",
        (true, true, _) => "loading",
        (true, false, true) => "playing",
        (true, false, false) => "paused",
    };
    AudioDump {
        state,
        output: if window.get_bool(BoolProp::AudioSilent) {
            "silent"
        } else {
            "device"
        },
        event_id: window.get_string(StringProp::AudioEventId).to_string(),
        room_id: window.get_string(StringProp::AudioRoomId).to_string(),
        kind: window.get_enum(EnumProp::AudioKind).to_string(),
        sender: window.get_string(StringProp::AudioSender).to_string(),
        title: window.get_string(StringProp::AudioTitle).to_string(),
        position_ms: window.get_int(IntProp::AudioPositionMs),
        duration_ms: window.get_int(IntProp::AudioDurationMs),
    }
}
