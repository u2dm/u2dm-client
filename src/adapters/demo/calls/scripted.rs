use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::sleep;

use super::call_book::{CallBook, Joiner, book};
use super::news::{send_media, video_news};
use super::{
    DESIGN, EXPIRES_AFTER, JORDAN, KAI, NADIA, RUST, SARAH, SECOND_SHARER, Scenario, scenario,
};
use crate::adapters::demo::data;
use crate::domain::call::CallIntent;
use crate::domain::call::media::MediaEvent;
use crate::domain::call::ring::{CallSignal, Ring, RingId, RingKind};
use crate::domain::call::sfu::SfuId;
use crate::domain::call::video::VideoKind;
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::room::RoomId;
use crate::domain::user_info::UserId;
use crate::util::unix_now_ms;

pub const RING_AFTER: Duration = Duration::from_secs(3);

const ELSEWHERE_AFTER: Duration = Duration::from_secs(5);

const RING_ENDS_AFTER: Duration = Duration::from_secs(8);

const EXPIRY_LAG: Duration = Duration::from_secs(1);

const RING_LIFETIME_MS: u64 = 90_000;

const SARAH_CHAT: &str = "!sarah:demo.local";

const MATRIX_HQ: &str = "!matrixhq:demo.local";

const SCREEN_SHARE_ENDS_AFTER: Duration = Duration::from_secs(6);

const SCREEN_SHARE_RESTARTS_AFTER: Duration = Duration::from_secs(4);

const SECOND_SHARE_AFTER: Duration = Duration::from_secs(4);

const CAMERA_TOGGLER: &str = NADIA;

const CAMERA_TOGGLES_EVERY: Duration = Duration::from_secs(5);

const LEAVER: &str = JORDAN;

const MEMBER_LEAVES_AFTER: Duration = Duration::from_secs(15);

const INTENT_CHANGES_AFTER: Duration = Duration::from_secs(4);

const OTHER_OWN_DEVICE: &str = "DEMOOTHERDEVICE";

pub enum Step {
    Rooms,
    Signal(CallSignal),
}

pub type Change = Box<dyn FnOnce(&CallBook) -> Vec<Step> + Send>;

pub struct ScriptedChange {
    pub after: Duration,
    pub change: Change,
}

pub fn script() -> Vec<ScriptedChange> {
    let shaped = scenario();
    let mut changes = Vec::new();
    if shaped.ring {
        let intent = if shaped.ring_video {
            CallIntent::Video
        } else {
            CallIntent::Voice
        };
        changes.push(ScriptedChange {
            after: RING_AFTER,
            change: Box::new(move |book: &CallBook| {
                if !shaped.ring_unconfirmed {
                    book.join(SARAH_CHAT, Joiner::member(SARAH, intent));
                }
                vec![
                    Step::Rooms,
                    Step::Signal(ring(book, SARAH_CHAT, SARAH, intent, RingKind::Ring)),
                ]
            }),
        });
        script_ring_endings(shaped, &mut changes);
    }
    if shaped.active && shaped.expiring {
        changes.push(ScriptedChange {
            after: EXPIRES_AFTER + EXPIRY_LAG,
            change: Box::new(|_: &CallBook| vec![Step::Rooms]),
        });
    }
    if shaped.active && shaped.intent_changes {
        changes.push(ScriptedChange {
            after: INTENT_CHANGES_AFTER,
            change: Box::new(|book: &CallBook| {
                tracing::info!(user = KAI, "demo: a member's call intent turns to video");
                book.say_intent(RUST, KAI, CallIntent::Video);
                vec![Step::Rooms]
            }),
        });
    }
    if shaped.active && shaped.member_leaves {
        changes.push(ScriptedChange {
            after: MEMBER_LEAVES_AFTER,
            change: Box::new(|book: &CallBook| {
                tracing::info!(user = LEAVER, "demo: a member leaves the call");
                book.leave(DESIGN, LEAVER);
                vec![Step::Rooms]
            }),
        });
    }
    if shaped.group_call {
        changes.push(ScriptedChange {
            after: RING_AFTER,
            change: Box::new(|book: &CallBook| {
                book.join(MATRIX_HQ, Joiner::member(KAI, CallIntent::Voice));
                vec![
                    Step::Rooms,
                    Step::Signal(ring(
                        book,
                        MATRIX_HQ,
                        KAI,
                        CallIntent::Voice,
                        RingKind::Notify,
                    )),
                ]
            }),
        });
    }
    changes
}

fn script_ring_endings(shaped: Scenario, changes: &mut Vec<ScriptedChange>) {
    if shaped.declined_elsewhere {
        changes.push(ScriptedChange {
            after: RING_AFTER + ELSEWHERE_AFTER,
            change: Box::new(|_: &CallBook| {
                vec![Step::Signal(CallSignal::Declined {
                    room_id: RoomId::new(SARAH_CHAT),
                    ring: ring_id(SARAH_CHAT),
                    by: UserId::new(data::own_user()),
                })]
            }),
        });
    }
    if shaped.answered_elsewhere {
        changes.push(ScriptedChange {
            after: RING_AFTER + ELSEWHERE_AFTER,
            change: Box::new(|book: &CallBook| {
                book.join(
                    SARAH_CHAT,
                    Joiner {
                        device: OTHER_OWN_DEVICE,
                        ..Joiner::member(data::own_user(), CallIntent::Voice)
                    },
                );
                vec![Step::Rooms]
            }),
        });
    }
    if shaped.ring_ends {
        changes.push(ScriptedChange {
            after: RING_AFTER + RING_ENDS_AFTER,
            change: Box::new(|book: &CallBook| {
                book.leave(SARAH_CHAT, SARAH);
                vec![Step::Rooms]
            }),
        });
    }
}

fn ring_id(room_id: &str) -> RingId {
    RingId::new(format!("$demo-ring-{room_id}"))
}

fn ring(
    book: &CallBook,
    room_id: &str,
    caller: &str,
    intent: CallIntent,
    kind: RingKind,
) -> CallSignal {
    CallSignal::Rang(Box::new(Ring {
        id: ring_id(room_id),
        room_id: RoomId::new(room_id),
        caller: NamedUser::new(
            caller,
            MemberName::new(data::person_name(caller).as_deref(), false),
        ),
        session: book.session_of(room_id, caller),
        intent: Some(intent),
        kind,
        expires_ms: unix_now_ms().saturating_add(RING_LIFETIME_MS),
    }))
}

pub fn partner_of(room_id: &str) -> Option<&'static str> {
    (room_id == SARAH_CHAT).then_some(SARAH)
}

pub fn answer_in(room_id: &str) {
    if let Some(partner) = partner_of(room_id) {
        book().join(room_id, Joiner::member(partner, CallIntent::Voice));
    }
}

pub(super) fn share_screen_later(
    events_tx: &mpsc::UnboundedSender<MediaEvent>,
    sfu: &SfuId,
    joiner: Joiner,
) {
    if !(scenario().second_share && joiner.user == SECOND_SHARER && joiner.on_main_device()) {
        return;
    }
    let events_tx = events_tx.clone();
    let started = video_news(sfu, joiner, VideoKind::Screen, true);
    tokio::spawn(async move {
        sleep(SECOND_SHARE_AFTER).await;
        tracing::info!(user = joiner.user, "demo: a second screen share starts");
        send_media(&events_tx, started);
    });
}

pub(super) fn toggle_camera_later(
    events_tx: &mpsc::UnboundedSender<MediaEvent>,
    sfu: &SfuId,
    joiner: Joiner,
) {
    let shaped = scenario();
    if !(shaped.cameras
        && shaped.camera_toggles
        && joiner.user == CAMERA_TOGGLER
        && joiner.on_main_device())
    {
        return;
    }
    let events_tx = events_tx.clone();
    let sfu = sfu.clone();
    tokio::spawn(async move {
        let mut on = true;
        while !events_tx.is_closed() {
            sleep(CAMERA_TOGGLES_EVERY).await;
            on = !on;
            tracing::info!(user = joiner.user, on, "demo: a camera switches");
            send_media(&events_tx, video_news(&sfu, joiner, VideoKind::Camera, on));
        }
    });
}

pub(super) fn end_screen_share_later(
    events_tx: &mpsc::UnboundedSender<MediaEvent>,
    sfu: &SfuId,
    joiner: Joiner,
) {
    if !scenario().screen_share_ends {
        return;
    }
    let events_tx = events_tx.clone();
    let ended = MediaEvent::Video {
        sfu: sfu.clone(),
        identity: joiner.identity(),
        kind: VideoKind::Screen,
        feed: None,
    };
    let restarted = scenario()
        .screen_share_restarts
        .then(|| video_news(sfu, joiner, VideoKind::Screen, true));
    tokio::spawn(async move {
        sleep(SCREEN_SHARE_ENDS_AFTER).await;
        tracing::info!(user = joiner.user, "demo: a screen share ends");
        send_media(&events_tx, ended);
        if let Some(restarted) = restarted {
            sleep(SCREEN_SHARE_RESTARTS_AFTER).await;
            tracing::info!(user = joiner.user, "demo: a screen share starts again");
            send_media(&events_tx, restarted);
        }
    });
}
