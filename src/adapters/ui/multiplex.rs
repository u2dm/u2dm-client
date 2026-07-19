use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};

use super::common::SLINT_INFLIGHT;
use crate::commands::UiEvent;
use crate::domain::models::{ConnectionStatus, Room, Space};
use crate::ports::media::MediaCache;

#[allow(clippy::too_many_arguments)]
pub fn spawn_event_multiplexer(
    mut ui_rx: mpsc::Receiver<UiEvent>,
    mut rooms_rx: watch::Receiver<Arc<[Room]>>,
    mut spaces_rx: watch::Receiver<Arc<[Space]>>,
    mut subspaces_rx: watch::Receiver<Arc<[Space]>>,
    mut connection_rx: watch::Receiver<ConnectionStatus>,
    mut status_rx: watch::Receiver<String>,
    media_cache: Arc<dyn MediaCache>,
    post: impl Fn(UiEvent, Arc<dyn MediaCache>, OwnedSemaphorePermit) + Send + 'static,
) {
    tokio::spawn(async move {
        let sem = Arc::new(Semaphore::new(SLINT_INFLIGHT));
        let mut rooms_done = false;
        let mut spaces_done = false;
        let mut subspaces_done = false;
        let mut connection_done = false;
        let mut status_done = false;
        loop {
            let Ok(permit) = Arc::clone(&sem).acquire_owned().await else {
                break;
            };
            tokio::select! {
                maybe = ui_rx.recv() => {
                    let Some(event) = maybe else { break };
                    post(event, Arc::clone(&media_cache), permit);
                }
                changed = rooms_rx.changed(), if !rooms_done => {
                    if changed.is_err() {
                        rooms_done = true;
                    } else {
                        let rooms = rooms_rx.borrow_and_update().clone();
                        post(UiEvent::Rooms(rooms), Arc::clone(&media_cache), permit);
                    }
                }
                changed = spaces_rx.changed(), if !spaces_done => {
                    if changed.is_err() {
                        spaces_done = true;
                    } else {
                        let spaces = spaces_rx.borrow_and_update().clone();
                        post(UiEvent::Spaces(spaces), Arc::clone(&media_cache), permit);
                    }
                }
                changed = subspaces_rx.changed(), if !subspaces_done => {
                    if changed.is_err() {
                        subspaces_done = true;
                    } else {
                        let subspaces = subspaces_rx.borrow_and_update().clone();
                        post(UiEvent::Subspaces(subspaces), Arc::clone(&media_cache), permit);
                    }
                }
                changed = connection_rx.changed(), if !connection_done => {
                    if changed.is_err() {
                        connection_done = true;
                    } else {
                        let status = connection_rx.borrow_and_update().clone();
                        post(UiEvent::ConnectionStatus(status), Arc::clone(&media_cache), permit);
                    }
                }
                changed = status_rx.changed(), if !status_done => {
                    if changed.is_err() {
                        status_done = true;
                    } else {
                        let message = status_rx.borrow_and_update().clone();
                        post(UiEvent::Status(message), Arc::clone(&media_cache), permit);
                    }
                }
            }
        }
    });
}
