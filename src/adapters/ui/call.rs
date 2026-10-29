use std::cell::RefCell;
use std::collections::HashSet;
use std::slice;
use std::time::Duration;

use slint::{ComponentHandle, Image, Model, SharedString, Timer, TimerMode, VecModel};

use super::backend::{Models, UiBackend, UiEventContext};
use super::decode::{AvatarSlot, load_avatar_async, peek_avatar};
use super::dto::{
    call_connection_rows, call_failure_message, call_notice_mark, call_offer_mark,
    call_screen_mark, sheet_preview_mark, test_phase_mark,
};
use super::fields::{CallPersonFields, CallTileFields};
use super::notices::NoticeWords;
use super::present::{avatar_color_index, avatar_initials};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::apply_call_tiles;
use super::ringtone::{self, Tone};
use super::{audio, call_video, call_window, video};
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{
    ActiveCall, AppViewState, CallDevices, CallNotice, CallPerson, CallPhase, CallSheetView,
    CallTile, CallView, DeviceNews, FailedCall, JoinOffer, OfferView, RingView, RoomCallSummary,
    ScreenView, ShareNews, SheetPreview, TileKind,
};
use crate::domain::call::devices::DeviceKind;
use crate::domain::call::video::{FeedId, Framing};
use crate::domain::member_name::NamedUser;
use crate::ports::media::MediaCache;
use crate::ports::notifier::{NotifierPort, RingNotice};
use crate::util::unix_now_ms;

const ELAPSED_TICK: Duration = Duration::from_secs(1);
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3600;
const MILLIS_PER_SECOND: u64 = 1000;

thread_local! {
    static ELAPSED: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

pub fn apply_call<B: UiBackend>(
    w: &B::Window,
    last_view: Option<&AppViewState>,
    next: &CallView,
    ctx: &UiEventContext<'_, B>,
) {
    let last = last_view.map(|view| &view.call);
    if last == Some(next) {
        return;
    }
    if last.is_none_or(|last| last.here != next.here) {
        apply_here(w, next.here.as_ref());
    }
    let last_ring = last.and_then(|last| last.ring.as_ref());
    if last_ring != next.ring.as_ref() {
        let in_call = next.active.is_some();
        apply_ring::<B>(w, last_ring, next.ring.as_ref(), in_call, ctx);
    }
    if last.is_none_or(|last| last.offer != next.offer) {
        apply_offer(w, next.offer.as_ref());
    }
    if last.is_none_or(|last| last.sheet != next.sheet) {
        apply_sheet(w, next.sheet.as_ref());
    }
    let last_preview = last.and_then(previewed_feed);
    let next_preview = previewed_feed(next);
    if last.is_none() || last_preview != next_preview {
        call_video::follow_preview::<B>(w, next_preview);
    }
    let last_active = last.and_then(|last| last.active.as_ref());
    if last_active != next.active.as_ref() {
        apply_active::<B>(w, last_active, next.active.as_ref(), ctx.media);
        call_window::follow::<B>(w);
    }
    let last_devices = last.and_then(shown_devices);
    let next_devices = shown_devices(next);
    if last.is_none() || last_devices != next_devices {
        apply_device_rows::<B>(w, last_devices, next_devices);
    }
    if last.is_none_or(|last| last.failed != next.failed) {
        apply_failed(w, next.failed.as_ref());
    }
}

fn apply_failed(w: &impl UiProps, failed: Option<&FailedCall>) {
    let Some(failed) = failed else {
        w.set_call_failure(UserMessageKind::None);
        return;
    };
    w.set_string(
        StringProp::FailureRoomId,
        SharedString::from(failed.room_id.as_ref()),
    );
    w.set_string(
        StringProp::FailureDetail,
        SharedString::from(&failed.room_name),
    );
    w.set_bool(BoolProp::FailureRetry, failed.retry);
    w.set_call_failure(call_failure_message(failed.failure));
}

fn apply_notice(w: &impl UiProps, notice: Option<&CallNotice>) {
    let (name, others) = match notice {
        Some(
            CallNotice::CantDecrypt { who, others, .. }
            | CallNotice::NotReceived { who, others, .. },
        ) => (who.shown(), *others),
        _ => (String::new(), 0),
    };
    w.set_string(StringProp::NoticeName, SharedString::from(name));
    w.set_int(
        IntProp::NoticeOthers,
        i32::try_from(others).unwrap_or(i32::MAX),
    );
    w.set_call_notice(call_notice_mark(notice));
}

fn apply_offer(w: &impl UiProps, offer: Option<&OfferView>) {
    w.set_string(
        StringProp::OfferRoomId,
        offer.map_or_else(SharedString::new, |offer| {
            SharedString::from(offer.room_id.as_ref())
        }),
    );
    let join = offer.map(|offer| offer.offer);
    if let Some(JoinOffer::Refused(refusal)) = join {
        w.set_call_refusal(refusal);
    }
    w.set_call_offer(call_offer_mark(join));
}

fn apply_sheet(w: &impl UiProps, sheet: Option<&CallSheetView>) {
    let Some(sheet) = sheet else {
        w.set_bool(BoolProp::SheetVisible, false);
        apply_sheet_preview(w, None);
        w.set_call_sheet_test_phase(test_phase_mark(None));
        return;
    };
    w.set_string(
        StringProp::SheetRoomId,
        SharedString::from(sheet.room_id.as_ref()),
    );
    w.set_string(
        StringProp::SheetRoomName,
        SharedString::from(&sheet.room_name),
    );
    if let JoinOffer::Refused(refusal) = sheet.offer {
        w.set_call_sheet_refusal(refusal);
    }
    w.set_call_sheet_offer(call_offer_mark(Some(sheet.offer)));
    w.set_bool(BoolProp::SheetDirect, sheet.direct);
    w.set_bool(BoolProp::SheetVideo, sheet.video);
    w.set_int(
        IntProp::SheetPeople,
        i32::try_from(sheet.people).unwrap_or(i32::MAX),
    );
    apply_names(
        w,
        &sheet.others,
        [StringProp::SheetPeopleFirst, StringProp::SheetPeopleSecond],
        IntProp::SheetPeopleMore,
    );
    apply_names(
        w,
        &sheet.bridges,
        [
            StringProp::SheetBridgesFirst,
            StringProp::SheetBridgesSecond,
        ],
        IntProp::SheetBridgesMore,
    );
    w.set_bool(BoolProp::SheetMicrophone, sheet.proposed.microphone);
    w.set_bool(BoolProp::SheetCamera, sheet.proposed.camera);
    apply_sheet_preview(w, sheet.preview.as_ref());
    w.set_bool(BoolProp::SheetTests, sheet.tests.offered());
    let running = sheet.tests.running();
    if let Some(test) = running {
        w.set_call_sheet_test_kind(test.kind);
        w.set_int(IntProp::SheetTestLevel, i32::from(test.level));
    }
    w.set_call_sheet_test_phase(test_phase_mark(running.map(|test| test.phase)));
    let leaving = sheet.leaving.as_ref();
    w.set_string(
        StringProp::SheetLeavingId,
        leaving.map_or_else(SharedString::new, |leaving| {
            SharedString::from(leaving.room_id.as_ref())
        }),
    );
    w.set_string(
        StringProp::SheetLeavingName,
        leaving.map_or_else(SharedString::new, |leaving| {
            SharedString::from(&leaving.room_name)
        }),
    );
    w.set_bool(BoolProp::SheetEnded, sheet.ended);
    w.set_int(IntProp::SheetSerial, sheet.serial);
    w.set_bool(BoolProp::SheetVisible, true);
}

fn apply_sheet_preview(w: &impl UiProps, preview: Option<&SheetPreview>) {
    let call = match preview {
        Some(SheetPreview::InCall { room_name, .. } | SheetPreview::Elsewhere { room_name }) => {
            SharedString::from(room_name)
        }
        _ => SharedString::new(),
    };
    w.set_string(StringProp::SheetPreviewCall, call);
    w.set_string(
        StringProp::SheetPreviewFeed,
        preview
            .and_then(SheetPreview::feed)
            .map_or_else(SharedString::new, |feed| SharedString::from(feed.as_ref())),
    );
    if let Some(SheetPreview::Failed(trouble)) = preview {
        w.set_call_preview_trouble(*trouble);
    }
    w.set_call_sheet_preview(sheet_preview_mark(preview));
}

fn previewed_feed(call: &CallView) -> Option<&FeedId> {
    call.sheet
        .as_ref()
        .and_then(|sheet| sheet.preview.as_ref())
        .and_then(SheetPreview::feed)
}

fn apply_active<B: UiBackend>(
    w: &B::Window,
    last: Option<&ActiveCall>,
    next: Option<&ActiveCall>,
    media: &dyn MediaCache,
) {
    let Some(active) = next else {
        w.set_bool(BoolProp::ActiveVisible, false);
        w.set_bool(BoolProp::MicrophoneMetered, false);
        w.set_int(IntProp::ActiveCall, 0);
        w.set_int(IntProp::ShareCount, 0);
        w.set_int(IntProp::StartedShareSerial, 0);
        w.set_int(IntProp::EndedShareSerial, 0);
        w.set_int(IntProp::HandNewsSerial, 0);
        w.set_int(IntProp::HandsRaised, 0);
        w.set_int(IntProp::ReactionNewsSerial, 0);
        w.set_int(IntProp::ShareChangeCancelled, 0);
        w.set_int(IntProp::DeviceNewsSerial, 0);
        apply_names(
            w,
            &[],
            [StringProp::BridgesFirst, StringProp::BridgesSecond],
            IntProp::BridgesMore,
        );
        apply_notice(w, None);
        let models = B::models();
        models.call_stage.set_vec(Vec::new());
        models.call_tiles.set_vec(Vec::new());
        models.call_self.set_vec(Vec::new());
        models.call_people.set_vec(Vec::new());
        models.call_connections.set_vec(Vec::new());
        call_video::follow::<B>(w, &[]);
        stop_elapsed(w);
        ringtone::stop_if(Tone::Ringback);
        return;
    };
    if last.is_none() {
        audio::pause(w);
        video::set_playing(w, false);
    }
    w.set_string(
        StringProp::ActiveRoomId,
        SharedString::from(active.room_id.as_ref()),
    );
    w.set_string(
        StringProp::ActiveRoomName,
        SharedString::from(&active.room_name),
    );
    w.set_int(
        IntProp::ActiveCall,
        i32::try_from(active.call).unwrap_or(i32::MAX),
    );
    w.set_bool(BoolProp::ActiveRoomAlert, active.room_unread.alert);
    w.set_bool(BoolProp::ActiveRoomMention, active.room_unread.mention);
    w.set_bool(BoolProp::ActiveRoomHint, active.room_unread.hint);
    w.set_bool(BoolProp::ActiveEncrypted, active.encrypted);
    w.set_bool(BoolProp::VideoCall, active.video);
    let shows_video = active.video || shown_tiles(active).any(|tile| tile.video.is_some());
    w.set_bool(BoolProp::ActiveVideo, shows_video);
    w.set_call_phase(active.phase);
    w.set_call_mic(active.microphone);
    w.set_bool(
        BoolProp::MicrophoneMetered,
        active.microphone_level.is_some(),
    );
    w.set_int(
        IntProp::MicrophoneLevel,
        active.microphone_level.map_or(0, i32::from),
    );
    w.set_call_camera(active.camera);
    apply_hands(w, active);
    apply_reactions(w, active);
    w.set_call_screen(call_screen_mark(active.screen));
    if let ScreenView::Unavailable(refusal) = active.screen {
        w.set_call_screen_refusal(refusal);
    }
    apply_notice(w, active.notice.as_ref());
    apply_stage::<B>(w, last, active, media);
    apply_people::<B>(last.map(|last| last.people.as_ref()), &active.people, media);
    if last.map(|last| &last.connection) != Some(&active.connection) {
        B::models().call_connections.set_vec(
            active
                .connection
                .as_deref()
                .map(call_connection_rows)
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect::<Vec<B::CallConnection>>(),
        );
    }
    request_faces::<B>(active, media);
    apply_device_news(w, active.devices.news.as_ref());
    if last.is_none_or(|last| last.live_since_ms != active.live_since_ms) {
        follow_elapsed(w, active.live_since_ms);
    }
    w.set_bool(BoolProp::ActiveVisible, true);
    follow_ringback(w, last.map(|last| last.phase), active.phase);
}

fn shown_tiles(active: &ActiveCall) -> impl Iterator<Item = &CallTile> {
    active.stage.iter().chain(active.tiles.iter())
}

fn apply_stage<B: UiBackend>(
    w: &B::Window,
    last: Option<&ActiveCall>,
    active: &ActiveCall,
    media: &dyn MediaCache,
) {
    w.set_bool(BoolProp::SelfView, active.self_view);
    w.set_bool(BoolProp::FollowsSpeaker, active.follows_speaker);
    w.set_bool(BoolProp::ViewChosen, active.view_chosen);
    w.set_bool(BoolProp::SharePreview, active.share_preview);
    w.set_int(
        IntProp::ShareChangeCancelled,
        i32::try_from(active.share_change_cancelled).unwrap_or(i32::MAX),
    );
    w.set_bool(BoolProp::StageChosen, active.stage_chosen);
    w.set_string(StringProp::FirstSharer, shown_name(active.shares.first()));
    w.set_int(
        IntProp::ShareCount,
        i32::try_from(active.shares.len()).unwrap_or(i32::MAX),
    );
    w.set_string(
        StringProp::OfferedShare,
        shown_name(active.offered_share.as_ref()),
    );
    apply_share_news(
        w,
        active.started_share.as_ref(),
        StringProp::StartedShare,
        IntProp::StartedShareSerial,
    );
    apply_share_news(
        w,
        active.ended_share.as_ref(),
        StringProp::EndedShare,
        IntProp::EndedShareSerial,
    );
    apply_names(
        w,
        &active.bridges,
        [StringProp::BridgesFirst, StringProp::BridgesSecond],
        IntProp::BridgesMore,
    );
    let unchanged = last.is_some_and(|last| {
        last.stage == active.stage
            && last.tiles == active.tiles
            && last.compact_self == active.compact_self
    });
    if unchanged {
        return;
    }
    let models = B::models();
    let convert = |tile: &CallTile| {
        let mut row = B::convert_call_tile(tile);
        call_video::show_last::<B>(&mut row);
        if let Some(face) = face_of(tile.avatar_mxc.as_deref(), media) {
            row.set_avatar(face);
            row.set_has_avatar(true);
        }
        row
    };
    let last_stage = last.and_then(|last| last.stage.as_ref());
    apply_call_tiles(
        &models.call_stage,
        active.stage.as_slice(),
        last_stage.map_or(&[], slice::from_ref),
        &convert,
        &|row| row.key(),
    );
    apply_call_tiles(
        &models.call_tiles,
        &active.tiles,
        last.map_or(&[] as &[CallTile], |last| last.tiles.as_ref()),
        &convert,
        &|row| row.key(),
    );
    let last_self = last.and_then(|last| last.compact_self.as_ref());
    apply_call_tiles(
        &models.call_self,
        active.compact_self.as_slice(),
        last_self.map_or(&[], slice::from_ref),
        &convert,
        &|row| row.key(),
    );
    let feeds: Vec<(FeedId, Framing)> = shown_tiles(active)
        .filter_map(|tile| Some((tile.video.clone()?, framing(tile.kind))))
        .collect();
    call_video::follow::<B>(w, &feeds);
}

fn shown_name(who: Option<&NamedUser>) -> SharedString {
    who.map_or_else(SharedString::new, |who| SharedString::from(who.shown()))
}

fn apply_hands(w: &impl UiProps, active: &ActiveCall) {
    w.set_call_hand(active.hand);
    w.set_int(
        IntProp::HandsRaised,
        i32::try_from(active.hands_raised).unwrap_or(i32::MAX),
    );
    w.set_string(
        StringProp::HandNews,
        shown_name(active.hand_news.as_ref().map(|news| &news.who)),
    );
    w.set_int(
        IntProp::HandNewsSerial,
        active
            .hand_news
            .as_ref()
            .map_or(0, |news| i32::try_from(news.serial).unwrap_or(i32::MAX)),
    );
}

fn apply_reactions(w: &impl UiProps, active: &ActiveCall) {
    w.set_call_reaction(active.reaction);
    let news = active.reaction_news.as_ref();
    w.set_string(
        StringProp::ReactionNews,
        shown_name(news.map(|news| &news.who)),
    );
    w.set_string(
        StringProp::ReactionNewsEmoji,
        news.map_or_else(SharedString::new, |news| {
            SharedString::from(news.reaction.emoji.as_str())
        }),
    );
    w.set_string(
        StringProp::ReactionNewsName,
        news.map_or_else(SharedString::new, |news| {
            SharedString::from(news.reaction.name.as_str())
        }),
    );
    w.set_int(
        IntProp::ReactionNewsSerial,
        news.map_or(0, |news| i32::try_from(news.serial).unwrap_or(i32::MAX)),
    );
}

fn apply_share_news(w: &impl UiProps, news: Option<&ShareNews>, who: StringProp, serial: IntProp) {
    w.set_string(who, shown_name(news.map(|news| &news.who)));
    w.set_int(
        serial,
        news.map_or(0, |news| i32::try_from(news.serial).unwrap_or(i32::MAX)),
    );
}

fn shown_devices(call: &CallView) -> Option<&CallDevices> {
    if let Some(devices) = call.ring.as_ref().and_then(|ring| ring.devices.as_ref()) {
        return Some(devices);
    }
    match &call.sheet {
        Some(sheet) => sheet.devices.as_ref(),
        None => call.active.as_ref().map(|active| &active.devices),
    }
}

fn apply_device_rows<B: UiBackend>(
    w: &B::Window,
    last: Option<&CallDevices>,
    next: Option<&CallDevices>,
) {
    let models = B::models();
    for kind in DeviceKind::ALL {
        let options = next.map(|devices| devices.options.of(kind));
        if last.map(|last| last.options.of(kind)) == options {
            continue;
        }
        let rows: Vec<B::CallDevice> = options.map_or_else(Vec::new, |options| {
            options.iter().map(B::convert_call_device).collect()
        });
        device_model::<B>(&models, kind).set_vec(rows);
    }
    w.set_int(
        IntProp::MicrophoneChosen,
        chosen_index(next, DeviceKind::Microphone),
    );
    w.set_int(
        IntProp::CameraChosen,
        chosen_index(next, DeviceKind::Camera),
    );
    w.set_int(
        IntProp::SpeakerChosen,
        chosen_index(next, DeviceKind::Speaker),
    );
}

fn chosen_index(devices: Option<&CallDevices>, kind: DeviceKind) -> i32 {
    devices
        .and_then(|devices| {
            devices
                .options
                .of(kind)
                .iter()
                .position(|option| option.chosen)
        })
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(-1)
}

fn apply_device_news(w: &impl UiProps, news: Option<&DeviceNews>) {
    let Some(news) = news else {
        w.set_int(IntProp::DeviceNewsSerial, 0);
        return;
    };
    w.set_device_news_kind(news.kind);
    w.set_device_news_trouble(news.trouble);
    w.set_string(StringProp::DeviceNewsName, SharedString::from(&news.name));
    w.set_int(
        IntProp::DeviceNewsSerial,
        i32::try_from(news.serial).unwrap_or(i32::MAX),
    );
}

fn device_model<B: UiBackend>(models: &Models<B>, kind: DeviceKind) -> &VecModel<B::CallDevice> {
    match kind {
        DeviceKind::Microphone => &models.call_microphones,
        DeviceKind::Speaker => &models.call_speakers,
        DeviceKind::Camera => &models.call_cameras,
    }
}

fn apply_people<B: UiBackend>(
    last: Option<&[CallPerson]>,
    people: &[CallPerson],
    media: &dyn MediaCache,
) {
    if last == Some(people) {
        return;
    }
    let rows: Vec<B::CallPerson> = people
        .iter()
        .map(|person| {
            let mut row = B::convert_call_person(person);
            if let Some(face) = face_of(person.avatar_mxc.as_deref(), media) {
                row.set_avatar(face);
                row.set_has_avatar(true);
            }
            row
        })
        .collect();
    B::models().call_people.set_vec(rows);
}

fn face_of(mxc: Option<&str>, media: &dyn MediaCache) -> Option<Image> {
    let path = media.user_avatar_path(mxc?)?;
    peek_avatar(&path)
}

fn request_faces<B: UiBackend>(active: &ActiveCall, media: &dyn MediaCache) {
    let tiles = active
        .stage
        .iter()
        .chain(active.tiles.iter())
        .chain(active.compact_self.iter())
        .map(|tile| (tile.name.user_id.as_str(), tile.avatar_mxc.as_deref()));
    let people = active
        .people
        .iter()
        .map(|person| (person.user.user_id.as_str(), person.avatar_mxc.as_deref()));
    let mut asked: HashSet<&str> = HashSet::new();
    for (user_id, mxc) in tiles.chain(people) {
        let Some(mxc) = mxc else {
            continue;
        };
        if !asked.insert(user_id) {
            continue;
        }
        let path = media.user_avatar_path(mxc);
        let slot = AvatarSlot::CallFace(user_id.to_owned());
        if let Some(face) = load_avatar_async(path.as_deref(), slot) {
            show_faces::<B>(&HashSet::from([user_id]), &face);
        }
    }
}

pub fn show_faces<B: UiBackend>(users: &HashSet<&str>, face: &Image) {
    if users.is_empty() {
        return;
    }
    let models = B::models();
    for rows in [&models.call_stage, &models.call_tiles, &models.call_self] {
        for row in 0..rows.row_count() {
            let Some(mut entry) = rows.row_data(row) else {
                continue;
            };
            if entry.screen() || entry.has_avatar() || !users.contains(entry.user_id()) {
                continue;
            }
            entry.set_avatar(face.clone());
            entry.set_has_avatar(true);
            rows.set_row_data(row, entry);
        }
    }
    let people = &models.call_people;
    for row in 0..people.row_count() {
        let Some(mut entry) = people.row_data(row) else {
            continue;
        };
        if entry.has_avatar() || !users.contains(entry.user_id()) {
            continue;
        }
        entry.set_avatar(face.clone());
        entry.set_has_avatar(true);
        people.set_row_data(row, entry);
    }
}

fn apply_names(
    w: &impl UiProps,
    names: &[NamedUser],
    [first, second]: [StringProp; 2],
    more: IntProp,
) {
    let shown = |index: usize| {
        names
            .get(index)
            .map_or_else(SharedString::new, |name| SharedString::from(name.shown()))
    };
    w.set_string(first, shown(0));
    w.set_string(second, shown(1));
    w.set_int(
        more,
        i32::try_from(names.len().saturating_sub(2)).unwrap_or(i32::MAX),
    );
}

const fn framing(kind: TileKind) -> Framing {
    match kind {
        TileKind::Person => Framing::Cover,
        TileKind::Screen => Framing::Contain,
    }
}

fn follow_ringback<W: UiProps + ComponentHandle + 'static>(
    w: &W,
    last: Option<CallPhase>,
    next: CallPhase,
) {
    match (last == Some(CallPhase::Calling), next == CallPhase::Calling) {
        (false, true) => {
            ringtone::play(&w.as_weak(), Tone::Ringback);
        }
        (true, false) => ringtone::stop_if(Tone::Ringback),
        _ => {}
    }
}

fn follow_elapsed<W: UiProps + ComponentHandle + 'static>(w: &W, since_ms: Option<u64>) {
    let Some(since_ms) = since_ms else {
        stop_elapsed(w);
        return;
    };
    w.set_string(StringProp::CallElapsed, elapsed_label(since_ms));
    let weak = w.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, ELAPSED_TICK, move || {
        if let Some(w) = weak.upgrade() {
            w.set_string(StringProp::CallElapsed, elapsed_label(since_ms));
        }
    });
    ELAPSED.with(|cell| *cell.borrow_mut() = Some(timer));
}

fn stop_elapsed(w: &impl UiProps) {
    ELAPSED.with(|cell| drop(cell.borrow_mut().take()));
    w.set_string(StringProp::CallElapsed, SharedString::new());
}

fn elapsed_label(since_ms: u64) -> SharedString {
    let seconds = unix_now_ms().saturating_sub(since_ms) / MILLIS_PER_SECOND;
    let hours = seconds / SECONDS_PER_HOUR;
    let minutes = seconds % SECONDS_PER_HOUR / SECONDS_PER_MINUTE;
    let seconds = seconds % SECONDS_PER_MINUTE;
    SharedString::from(if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    })
}

fn apply_here(w: &impl UiProps, here: Option<&RoomCallSummary>) {
    let Some(here) = here else {
        w.set_string(StringProp::HereRoomId, SharedString::new());
        w.set_int(IntProp::HerePeople, 0);
        w.set_bool(BoolProp::HereVideo, false);
        w.set_bool(BoolProp::HereJoined, false);
        return;
    };
    w.set_string(
        StringProp::HereRoomId,
        SharedString::from(here.room_id.as_ref()),
    );
    w.set_int(
        IntProp::HerePeople,
        i32::try_from(here.people).unwrap_or(i32::MAX),
    );
    w.set_bool(BoolProp::HereVideo, here.video);
    w.set_bool(BoolProp::HereJoined, here.joined_by_you);
}

fn apply_ring<B: UiBackend>(
    w: &B::Window,
    last: Option<&RingView>,
    next: Option<&RingView>,
    in_call: bool,
    ctx: &UiEventContext<'_, B>,
) {
    let notifier = ctx.notifier;
    let Some(next) = next else {
        stop_ringing(w, last, notifier);
        return;
    };
    if let Some(last) = last.filter(|last| last.room_id != next.room_id) {
        notifier.stop_ringing(&last.room_id);
    }
    let fresh = last.is_none_or(|last| last.serial != next.serial);
    show_card(w, next);
    if last.is_none_or(|last| fresh || last.room_avatar_mxc != next.room_avatar_mxc) {
        show_ring_avatar(w, next, ctx.media);
    }
    if next.quiet {
        ringtone::stop_if(Tone::Incoming);
    } else if fresh || last.is_some_and(|last| last.quiet) {
        audio::pause(w);
        video::set_playing(w, false);
        ringtone::play(&w.as_weak(), Tone::Incoming);
    }
    let was_desktop = last.is_some_and(|last| last.desktop && !fresh);
    match (was_desktop, next.desktop) {
        (false, true) => notifier.ring(compose_ring::<B>(w, next, in_call)),
        (true, false) => notifier.stop_ringing(&next.room_id),
        _ => {}
    }
}

fn stop_ringing(w: &impl UiProps, last: Option<&RingView>, notifier: &dyn NotifierPort) {
    w.set_bool(BoolProp::RingVisible, false);
    ringtone::stop_if(Tone::Incoming);
    if let Some(last) = last {
        notifier.stop_ringing(&last.room_id);
    }
}

fn show_card(w: &impl UiProps, ring: &RingView) {
    w.set_string(
        StringProp::RingRoomId,
        SharedString::from(ring.room_id.as_ref()),
    );
    w.set_string(
        StringProp::RingRoomName,
        SharedString::from(&ring.room_name),
    );
    w.set_string(
        StringProp::RingCaller,
        SharedString::from(ring.caller.shown()),
    );
    w.set_string(
        StringProp::RingInitial,
        SharedString::from(avatar_initials(&ring.room_name)),
    );
    w.set_int(
        IntProp::RingColorIndex,
        avatar_color_index(ring.room_id.as_ref()),
    );
    w.set_bool(BoolProp::RingVideo, ring.video);
    w.set_bool(BoolProp::RingDirect, ring.direct);
    w.set_bool(BoolProp::RingAnswerable, ring.answerable);
    w.set_bool(BoolProp::RingQuiet, ring.quiet);
    if let Some(refusal) = ring.refusal {
        w.set_ring_refusal(refusal);
    }
    w.set_int(
        IntProp::RingSerial,
        i32::try_from(ring.serial).unwrap_or(i32::MAX),
    );
    w.set_bool(BoolProp::RingVisible, true);
}

fn show_ring_avatar(w: &impl UiProps, ring: &RingView, media: &dyn MediaCache) {
    let path = ring
        .room_avatar_mxc
        .as_deref()
        .and_then(|mxc| media.room_avatar_path(mxc));
    w.apply_ring_avatar(load_avatar_async(path.as_deref(), AvatarSlot::Ring));
}

fn compose_ring<B: UiBackend>(w: &B::Window, ring: &RingView, in_call: bool) -> RingNotice {
    let caller = ring.caller.shown();
    let body = B::notification_text(
        w,
        &NoticeWords::Ring {
            video: ring.video,
            caller: &caller,
        },
    );
    RingNotice {
        key: ring.room_id.clone(),
        summary: ring.room_name.clone(),
        body: body.to_string(),
        open_label: w.get_string(StringProp::NotificationOpenLabel).to_string(),
        decline_label: w
            .get_string(StringProp::NotificationDeclineLabel)
            .to_string(),
        answer_label: (ring.answerable && !in_call).then(|| {
            let label = if ring.video {
                StringProp::NotificationAnswerWithoutCameraLabel
            } else {
                StringProp::NotificationAnswerLabel
            };
            w.get_string(label).to_string()
        }),
        sound: ringtone::sounding().is_none(),
    }
}
