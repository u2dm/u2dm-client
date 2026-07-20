use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};

use super::common::SLINT_INFLIGHT;
use crate::commands::{AppViewState, Effect};
use crate::ports::media::MediaCache;

pub fn spawn_event_multiplexer(
    mut ui_rx: mpsc::Receiver<Effect>,
    mut view_rx: watch::Receiver<Arc<AppViewState>>,
    media_cache: Arc<dyn MediaCache>,
    post: impl Fn(Effect, Arc<dyn MediaCache>, OwnedSemaphorePermit) + Send + 'static,
) {
    tokio::spawn(async move {
        let sem = Arc::new(Semaphore::new(SLINT_INFLIGHT));
        let mut view_done = false;
        loop {
            let Ok(permit) = Arc::clone(&sem).acquire_owned().await else {
                break;
            };
            tokio::select! {
                biased;
                maybe = ui_rx.recv() => {
                    let Some(event) = maybe else { break };
                    post(event, Arc::clone(&media_cache), permit);
                }
                changed = view_rx.changed(), if !view_done => {
                    if changed.is_err() {
                        view_done = true;
                    } else {
                        let snapshot = view_rx.borrow_and_update().clone();
                        post(Effect::Snapshot(snapshot), Arc::clone(&media_cache), permit);
                    }
                }
            }
        }
    });
}
