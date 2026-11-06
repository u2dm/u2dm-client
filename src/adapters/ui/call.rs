mod devices;
mod faces;
mod ring;
mod sheet;

use std::cell::RefCell;
use std::slice;
use std::time::Duration;

use slint::{ComponentHandle, SharedString, Timer, TimerMode};

use self::devices::{apply_device_news, apply_device_rows, shown_devices};
pub(super) use self::faces::show_faces;
use self::faces::{
    apply_here_faces, apply_names, apply_people, face_of, faces_in_call, request_faces,
};
use self::ring::{apply_ring, follow_ringback};
use self::sheet::{apply_offer, apply_sheet, previewed_feed};
use super::backend::{UiBackend, UiEventContext};
use super::dto::{
    call_connection_rows, call_failure_message, call_notice_mark, call_screen_mark,
    call_tile_to_dto,
};
use super::fields::CallTileFields;
use super::present::duration_label;
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::apply_call_tiles;
use super::ringtone::{self, Tone};
use super::{audio, call_video, call_window, video};
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{
    ActiveCall, AppViewState, CallNotice, CallTile, CallView, FailedCall, RoomCallSummary,
    ScreenView, ServerDetails, ShareNews, TileKind,
};
use crate::domain::call::video::{FeedId, Framing};
use crate::domain::member_name::NamedUser;
use crate::ports::media::MediaCache;
use crate::util::{saturating_i32, unix_now_ms};

const ELAPSED_TICK: Duration = Duration::from_secs(1);

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
        apply_here::<B>(w, next.here.as_ref(), ctx.media);
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
    w.set_text(StringProp::FailureRoomId, failed.room_id.as_ref());
    w.set_text(StringProp::FailureDetail, &failed.room_name);
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
    w.set_text(StringProp::NoticeName, name);
    w.set_int(IntProp::NoticeOthers, saturating_i32(others));
    w.set_call_notice(call_notice_mark(notice));
}

fn apply_active<B: UiBackend>(
    w: &B::Window,
    last: Option<&ActiveCall>,
    next: Option<&ActiveCall>,
    media: &dyn MediaCache,
) {
    let Some(active) = next else {
        clear_active::<B>(w);
        return;
    };
    if last.is_none() {
        audio::pause(w);
        video::set_playing(w, false);
    }
    w.set_text(StringProp::ActiveRoomId, active.room_id.as_ref());
    w.set_text(StringProp::ActiveRoomName, &active.room_name);
    w.set_int(IntProp::ActiveCall, saturating_i32(active.call));
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
        apply_connections::<B>(active.connection.as_deref());
    }
    request_faces::<B>(faces_in_call(active), media);
    apply_device_news(w, active.devices.news.as_ref());
    if last.is_none_or(|last| last.live_since_ms != active.live_since_ms) {
        follow_elapsed(w, active.live_since_ms);
    }
    w.set_bool(BoolProp::ActiveVisible, true);
    follow_ringback(w, last.map(|last| last.phase), active.phase);
}

fn clear_active<B: UiBackend>(w: &B::Window) {
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
}

fn apply_connections<B: UiBackend>(servers: Option<&[ServerDetails]>) {
    B::models().call_connections.set_vec(
        servers
            .map(call_connection_rows)
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect::<Vec<B::CallConnection>>(),
    );
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
        saturating_i32(active.share_change_cancelled),
    );
    w.set_bool(BoolProp::StageChosen, active.stage_chosen);
    apply_shares(w, active);
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
        let mut row = B::CallTile::from(call_tile_to_dto(tile));
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

fn apply_shares(w: &impl UiProps, active: &ActiveCall) {
    w.set_text(StringProp::FirstSharer, shown_name(active.shares.first()));
    w.set_int(IntProp::ShareCount, saturating_i32(active.shares.len()));
    w.set_text(
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
}

fn shown_name(who: Option<&NamedUser>) -> String {
    who.map(NamedUser::shown).unwrap_or_default()
}

fn apply_hands(w: &impl UiProps, active: &ActiveCall) {
    w.set_call_hand(active.hand);
    w.set_int(IntProp::HandsRaised, saturating_i32(active.hands_raised));
    w.set_text(
        StringProp::HandNews,
        shown_name(active.hand_news.as_ref().map(|news| &news.who)),
    );
    w.set_int(
        IntProp::HandNewsSerial,
        active
            .hand_news
            .as_ref()
            .map_or(0, |news| saturating_i32(news.serial)),
    );
}

fn apply_reactions(w: &impl UiProps, active: &ActiveCall) {
    w.set_call_reaction(active.reaction);
    let news = active.reaction_news.as_ref();
    w.set_text(
        StringProp::ReactionNews,
        shown_name(news.map(|news| &news.who)),
    );
    w.set_text(
        StringProp::ReactionNewsEmoji,
        news.map_or("", |news| news.reaction.emoji.as_str()),
    );
    w.set_text(
        StringProp::ReactionNewsName,
        news.map_or("", |news| news.reaction.name.as_str()),
    );
    w.set_int(
        IntProp::ReactionNewsSerial,
        news.map_or(0, |news| saturating_i32(news.serial)),
    );
}

fn apply_share_news(w: &impl UiProps, news: Option<&ShareNews>, who: StringProp, serial: IntProp) {
    w.set_text(who, shown_name(news.map(|news| &news.who)));
    w.set_int(serial, news.map_or(0, |news| saturating_i32(news.serial)));
}

const fn framing(kind: TileKind) -> Framing {
    match kind {
        TileKind::Person => Framing::Cover,
        TileKind::Screen => Framing::Contain,
    }
}

fn follow_elapsed<W: UiProps + ComponentHandle + 'static>(w: &W, since_ms: Option<u64>) {
    let Some(since_ms) = since_ms else {
        stop_elapsed(w);
        return;
    };
    w.set_text(StringProp::CallElapsed, elapsed_label(since_ms));
    let weak = w.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, ELAPSED_TICK, move || {
        if let Some(w) = weak.upgrade() {
            w.set_text(StringProp::CallElapsed, elapsed_label(since_ms));
        }
    });
    ELAPSED.with(|cell| *cell.borrow_mut() = Some(timer));
}

fn stop_elapsed(w: &impl UiProps) {
    ELAPSED.with(|cell| drop(cell.borrow_mut().take()));
    w.set_string(StringProp::CallElapsed, SharedString::new());
}

fn elapsed_label(since_ms: u64) -> String {
    let elapsed = Duration::from_millis(unix_now_ms().saturating_sub(since_ms));
    duration_label(elapsed)
}

fn apply_here<B: UiBackend>(w: &B::Window, here: Option<&RoomCallSummary>, media: &dyn MediaCache) {
    let Some(here) = here else {
        w.set_string(StringProp::HereRoomId, SharedString::new());
        w.set_int(IntProp::HerePeople, 0);
        w.set_bool(BoolProp::HereVideo, false);
        w.set_bool(BoolProp::HereJoined, false);
        B::models().call_here_faces.set_vec(Vec::new());
        return;
    };
    apply_here_faces::<B>(&here.faces, media);
    w.set_text(StringProp::HereRoomId, here.room_id.as_ref());
    w.set_int(IntProp::HerePeople, saturating_i32(here.people));
    w.set_bool(BoolProp::HereVideo, here.video);
    w.set_bool(BoolProp::HereJoined, here.joined_by_you);
}
