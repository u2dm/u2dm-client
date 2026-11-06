use tokio::sync::mpsc;
use tokio::time::sleep;

use super::call_book::{CallBook, Joiner, Watcher, sfu_id};
use super::scripted::{end_screen_share_later, share_screen_later, toggle_camera_later};
use super::{JORDAN, NADIA, SECOND_SHARER, SLOW_ANSWER, scenario};
use crate::domain::call::media::MediaEvent;
use crate::domain::call::sfu::SfuId;
use crate::domain::call::video::{FeedId, VideoKind};

const CAMERA_FEED_PREFIX: &str = "demo-camera-";

const SCREEN_FEED_PREFIX: &str = "demo-screen-";

const SCREEN_SHARER: &str = NADIA;

const PORTRAIT_CAMERA_OF: &str = JORDAN;

impl CallBook {
    pub fn watch(&self, room_id: &str, sfu: &str, events_tx: mpsc::UnboundedSender<MediaEvent>) {
        let present = self.joiners(room_id, |joiner| joiner.sfu == sfu && !joiner.is_own());
        if let Some(id) = sfu_id(sfu) {
            for joiner in present {
                send_media(
                    &events_tx,
                    MediaEvent::Present {
                        sfu: id.clone(),
                        identity: joiner.identity(),
                        present: true,
                    },
                );
                if let Some(camera) = camera_news(&id, joiner, true) {
                    send_media(&events_tx, camera);
                }
                if let Some(screen) = screen_news(&id, joiner, true) {
                    send_media(&events_tx, screen);
                    end_screen_share_later(&events_tx, &id, joiner);
                }
                share_screen_later(&events_tx, &id, joiner);
                toggle_camera_later(&events_tx, &id, joiner);
            }
        }
        if let Ok(mut watchers) = self.watchers.lock() {
            watchers.retain(|watcher| !watcher.events_tx.is_closed());
            watchers.push(Watcher {
                room_id: room_id.to_owned(),
                sfu: sfu.to_owned(),
                events_tx,
            });
        }
    }

    pub fn restart_news(&self, room_id: &str, sfu: &str) -> (Vec<MediaEvent>, Vec<MediaEvent>) {
        let (Some(id), true) = (sfu_id(sfu), scenario().full_reconnect) else {
            return (Vec::new(), Vec::new());
        };
        let here = self.joiners(room_id, |joiner| joiner.sfu == sfu && !joiner.is_own());
        let mut gone = Vec::new();
        let mut back = Vec::new();
        for joiner in here {
            let presence = |present| MediaEvent::Present {
                sfu: id.clone(),
                identity: joiner.identity(),
                present,
            };
            gone.extend(screen_news(&id, joiner, false));
            gone.extend(camera_news(&id, joiner, false));
            gone.push(presence(false));
            back.push(presence(true));
            back.extend(camera_news(&id, joiner, true));
            back.extend(screen_news(&id, joiner, true));
        }
        (gone, back)
    }

    pub(super) fn tell_watchers(&self, room_id: &str, joiner: Joiner, present: bool) {
        if joiner.is_own() {
            return;
        }
        let Some(id) = sfu_id(joiner.sfu) else {
            return;
        };
        let Ok(watchers) = self.watchers.lock() else {
            return;
        };
        for watcher in watchers
            .iter()
            .filter(|watcher| watcher.room_id == room_id && watcher.sfu == joiner.sfu)
        {
            send_media(
                &watcher.events_tx,
                MediaEvent::Present {
                    sfu: id.clone(),
                    identity: joiner.identity(),
                    present,
                },
            );
            if let Some(camera) = camera_news(&id, joiner, present) {
                send_media(&watcher.events_tx, camera);
            }
            if let Some(screen) = screen_news(&id, joiner, present) {
                send_media(&watcher.events_tx, screen);
            }
        }
    }
}

pub(super) fn camera_news(sfu: &SfuId, joiner: Joiner, present: bool) -> Option<MediaEvent> {
    let shaped = scenario();
    let withheld = shaped.unreceived_media && joiner.user == JORDAN;
    (shaped.cameras && !withheld).then(|| video_news(sfu, joiner, VideoKind::Camera, present))
}

fn screen_news(sfu: &SfuId, joiner: Joiner, present: bool) -> Option<MediaEvent> {
    let shaped = scenario();
    let shares_at_once = shaped.screen_share && joiner.user == SCREEN_SHARER;
    let shares_later = shaped.second_share && joiner.user == SECOND_SHARER;
    (joiner.on_main_device() && (shares_at_once || (shares_later && !present)))
        .then(|| video_news(sfu, joiner, VideoKind::Screen, present))
}

pub(super) fn video_news(
    sfu: &SfuId,
    joiner: Joiner,
    kind: VideoKind,
    present: bool,
) -> MediaEvent {
    let prefix = match kind {
        VideoKind::Camera => CAMERA_FEED_PREFIX,
        VideoKind::Screen => SCREEN_FEED_PREFIX,
    };
    MediaEvent::Video {
        sfu: sfu.clone(),
        identity: joiner.identity(),
        kind,
        feed: present.then(|| FeedId::new(feed_name(prefix, joiner))),
    }
}

fn feed_name(prefix: &str, joiner: Joiner) -> String {
    if joiner.on_main_device() {
        format!("{prefix}{}", joiner.user)
    } else {
        format!("{prefix}{}/{}", joiner.user, joiner.device)
    }
}

pub fn is_portrait(feed: &FeedId) -> bool {
    scenario().portrait && feed.as_ref() == format!("{CAMERA_FEED_PREFIX}{PORTRAIT_CAMERA_OF}")
}

pub(super) fn send_media(events_tx: &mpsc::UnboundedSender<MediaEvent>, event: MediaEvent) {
    if events_tx.send(event).is_err() {
        tracing::debug!("demo: the call media session is gone");
    }
}

pub async fn pause() {
    if scenario().slow {
        sleep(SLOW_ANSWER).await;
    }
}
