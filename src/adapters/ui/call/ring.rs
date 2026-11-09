use slint::ComponentHandle;

use crate::adapters::ui::backend::{UiBackend, UiEventContext};
use crate::adapters::ui::decode::{AvatarSlot, load_avatar_async};
use crate::adapters::ui::notices::NoticeWords;
use crate::adapters::ui::present::{avatar_color_index, avatar_initials};
use crate::adapters::ui::props::{BoolProp, ImageProp, IntProp, StringProp, UiProps};
use crate::adapters::ui::ringtone::{self, Tone};
use crate::adapters::ui::{audio, video};
use crate::commands::view::{CallPhase, RingView};
use crate::ports::media::MediaCache;
use crate::ports::notifier::{NotifierPort, RingNotice};
use crate::util::saturating_i32;

pub(super) fn follow_ringback<W: UiProps + ComponentHandle + 'static>(
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

pub(super) fn apply_ring<B: UiBackend>(
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
    if !next.sounds {
        ringtone::stop_if(Tone::Incoming);
    } else if fresh || last.is_some_and(|last| !last.sounds) {
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
    w.set_text(StringProp::RingRoomId, ring.room_id.as_ref());
    w.set_text(StringProp::RingRoomName, &ring.room_name);
    w.set_text(StringProp::RingCaller, ring.caller.shown());
    w.set_text(StringProp::RingInitial, avatar_initials(&ring.room_name));
    w.set_int(
        IntProp::RingColorIndex,
        avatar_color_index(ring.room_id.as_ref()),
    );
    w.set_bool(BoolProp::RingVideo, ring.video);
    w.set_bool(BoolProp::RingDirect, ring.direct);
    w.set_bool(BoolProp::RingAnswerable, ring.answerable);
    w.set_bool(BoolProp::RingQuiet, ring.quiet);
    w.set_bool(BoolProp::RingSounds, ring.sounds);
    if let Some(refusal) = ring.refusal {
        w.set_ring_refusal(refusal);
    }
    w.set_int(IntProp::RingSerial, saturating_i32(ring.serial));
    w.set_bool(BoolProp::RingVisible, true);
}

fn show_ring_avatar(w: &impl UiProps, ring: &RingView, media: &dyn MediaCache) {
    let path = ring
        .room_avatar_mxc
        .as_deref()
        .and_then(|mxc| media.room_avatar_path(mxc));
    w.set_image(
        ImageProp::RingAvatar,
        load_avatar_async(path.as_deref(), AvatarSlot::Ring),
    );
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
        sound: ring.sounds && ringtone::sounding().is_none(),
    }
}
