use std::cell::RefCell;
use std::time::Duration;

use slint::{ComponentHandle, Model, SharedString, Timer, TimerMode};

use super::backend::{UiBackend, UiEventContext};
use super::decode::{AvatarSlot, load_avatar_async};
use super::dto::call_offer_mark;
use super::notices::NoticeWords;
use super::present::{avatar_color_index, avatar_initials};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::ringtone::{self, Tone};
use super::{audio, video};
use crate::commands::view::{
    ActiveCall, AppViewState, CallPhase, CallView, JoinOffer, OfferView, RingView, RoomCallSummary,
};
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
        apply_ring::<B>(w, last_ring, next.ring.as_ref(), ctx.media, ctx.notifier);
    }
    if last.is_none_or(|last| last.offer != next.offer) {
        apply_offer(w, next.offer.as_ref());
    }
    let last_active = last.and_then(|last| last.active.as_ref());
    if last_active != next.active.as_ref() {
        apply_active::<B>(w, last_active, next.active.as_ref());
    }
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

fn apply_active<B: UiBackend>(w: &B::Window, last: Option<&ActiveCall>, next: Option<&ActiveCall>) {
    let Some(active) = next else {
        w.set_bool(BoolProp::ActiveVisible, false);
        B::models().call_tiles.set_vec(Vec::new());
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
    w.set_bool(BoolProp::ActiveEncrypted, active.encrypted);
    w.set_bool(BoolProp::ActiveVideo, active.video);
    w.set_call_phase(active.phase);
    w.set_call_mic(active.microphone);
    if last.is_none_or(|last| last.tiles != active.tiles) {
        let models = B::models();
        let tiles: Vec<B::CallTile> = active.tiles.iter().map(B::convert_call_tile).collect();
        if models.call_tiles.iter().ne(tiles.iter().cloned()) {
            models.call_tiles.set_vec(tiles);
        }
    }
    if last.is_none_or(|last| last.live_since_ms != active.live_since_ms) {
        follow_elapsed(w, active.live_since_ms);
    }
    w.set_bool(BoolProp::ActiveVisible, true);
    follow_ringback(w, last.map(|last| last.phase), active.phase);
}

fn follow_ringback<W: UiProps + ComponentHandle + 'static>(
    w: &W,
    last: Option<CallPhase>,
    next: CallPhase,
) {
    match (last == Some(CallPhase::Ringing), next == CallPhase::Ringing) {
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
    media: &dyn MediaCache,
    notifier: &dyn NotifierPort,
) {
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
        show_ring_avatar(w, next, media);
    }
    if fresh {
        audio::pause(w);
        video::set_playing(w, false);
        ringtone::play(&w.as_weak(), Tone::Incoming);
    }
    let was_desktop = last.is_some_and(|last| last.desktop && !fresh);
    match (was_desktop, next.desktop) {
        (false, true) => notifier.ring(compose_ring::<B>(w, next)),
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
    w.set_bool(BoolProp::RingVisible, true);
}

fn show_ring_avatar(w: &impl UiProps, ring: &RingView, media: &dyn MediaCache) {
    let path = ring
        .room_avatar_mxc
        .as_deref()
        .and_then(|mxc| media.room_avatar_path(mxc));
    w.apply_ring_avatar(load_avatar_async(path.as_deref(), AvatarSlot::Ring));
}

fn compose_ring<B: UiBackend>(w: &B::Window, ring: &RingView) -> RingNotice {
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
        answer_label: ring.answerable.then(|| {
            w.get_string(StringProp::NotificationAnswerLabel)
                .to_string()
        }),
        sound: ringtone::sounding().is_none(),
    }
}
