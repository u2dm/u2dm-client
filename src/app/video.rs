use std::path::PathBuf;
use std::sync::Arc;

use super::event::AppEvent;
use super::input::EventSender;
use super::task_group::TaskGroup;
use crate::commands::messages::UserMessageKind;
use crate::commands::view::VideoView;
use crate::domain::room::RoomId;
use crate::ports::matrix::MediaPort;
use crate::ports::output::AppOutputPort;

pub(super) struct VideoController {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    issued: u64,
    downloading: Option<u64>,
}

impl VideoController {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("video"),
            issued: 0,
            downloading: None,
        }
    }

    pub(super) fn open(&mut self, media: Arc<dyn MediaPort>, room_id: RoomId, event_id: String) {
        self.tasks.cancel_and_detach();
        self.issued = self.issued.saturating_add(1);
        let request = self.issued;
        self.downloading = Some(request);
        self.publish(VideoView {
            visible: true,
            loading: true,
            ..VideoView::default()
        });
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                fetched = media.materialize_video(&room_id, &event_id) => fetched.map_err(|e| {
                    tracing::warn!("failed to materialize video: {e}");
                    UserMessageKind::MediaDownloadFailed
                }),
            };
            drop(events.send(AppEvent::VideoFetched { request, outcome }));
        });
    }

    pub(super) fn fetched(&mut self, request: u64, outcome: Result<PathBuf, UserMessageKind>) {
        if self
            .downloading
            .take_if(|downloading| *downloading == request)
            .is_none()
        {
            tracing::debug!(request, "dropping a superseded video download");
            return;
        }
        let (path, error) = match outcome {
            Ok(path) => (Some(path), UserMessageKind::None),
            Err(error) => (None, error),
        };
        self.publish(VideoView {
            visible: true,
            loading: false,
            path,
            error,
        });
    }

    pub(super) fn close(&mut self) {
        self.tasks.cancel_and_detach();
        self.downloading = None;
        self.publish(VideoView::default());
    }

    pub(super) async fn restart(&mut self) {
        self.tasks.restart().await;
        self.downloading = None;
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }

    fn publish(&self, view: VideoView) {
        self.output
            .publish(Box::new(move |state| state.video = view));
    }
}
