use std::sync::Arc;
use std::time::Duration;

use chrono::Timelike;
use slint::{Timer, TimerMode};

use super::backend::UiBackend;
use super::reconcile::apply_rooms;
use super::reduce::latest_rooms;
use crate::ports::media::MediaCache;

thread_local! {
    static MIDNIGHT_TIMER: Timer = Timer::default();
}

pub fn install_clock_invalidation<B: UiBackend>(media: Arc<dyn MediaCache>) {
    schedule_next::<B>(media);
}

fn schedule_next<B: UiBackend>(media: Arc<dyn MediaCache>) {
    let delay = duration_until_next_local_midnight();
    MIDNIGHT_TIMER.with(|timer| {
        timer.start(TimerMode::SingleShot, delay, move || {
            refresh_room_labels::<B>(media.as_ref());
            schedule_next::<B>(Arc::clone(&media));
        });
    });
}

fn refresh_room_labels<B: UiBackend>(media: &dyn MediaCache) {
    let Some(rooms) = latest_rooms() else {
        return;
    };
    B::with_models(|_timeline, rooms_model, _spaces, _subspaces| {
        apply_rooms(
            rooms_model,
            rooms.as_ref(),
            &|room| B::convert_room(room, media),
            &|entry| B::room_id(entry),
        );
    });
}

fn duration_until_next_local_midnight() -> Duration {
    let seconds_into_day = u64::from(chrono::Local::now().num_seconds_from_midnight());
    let seconds_remaining = 86_400_u64.saturating_sub(seconds_into_day).max(1);
    Duration::from_secs(seconds_remaining + 1)
}
