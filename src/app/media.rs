use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{Toast, VideoView};
use crate::domain::media::{MediaRendition, WaveformNeed};
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::matrix::MediaPort;
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;

fn publish_video(output: &dyn AppOutputPort, view: VideoView) {
    output.publish(Box::new(move |state| state.video = view));
}

pub(super) struct MediaActions {
    media_files: Arc<dyn MediaFilePort>,
    output: Arc<dyn AppOutputPort>,
    tasks: TaskGroup,
}

impl MediaActions {
    pub(super) fn new(media_files: Arc<dyn MediaFilePort>, output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            media_files,
            output,
            tasks: TaskGroup::new("media"),
        }
    }

    fn spawn_cancellable<Fut>(&mut self, work: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = work => {}
            }
        });
    }

    fn spawn_media_action<F, Fut>(
        &mut self,
        media: Arc<dyn MediaPort>,
        room_id: RoomId,
        event_id: String,
        download_failure: UserMessageKind,
        act: F,
    ) where
        F: FnOnce(Arc<dyn MediaFilePort>, Arc<dyn AppOutputPort>, String, Vec<u8>) -> Fut
            + Send
            + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let media_files = Arc::clone(&self.media_files);
        let output = Arc::clone(&self.output);
        self.spawn_cancellable(async move {
            match media
                .download_media(&room_id, &event_id, MediaRendition::FullFile)
                .await
            {
                Ok(data) => act(media_files, output, event_id, data).await,
                Err(e) => {
                    tracing::warn!("failed to download media: {e}");
                    show_toast(
                        output.as_ref(),
                        Toast::Error(UserMessage::new(download_failure)),
                    );
                }
            }
        });
    }

    pub(super) fn open_media(
        &mut self,
        media: Arc<dyn MediaPort>,
        room_id: RoomId,
        event_id: String,
    ) {
        self.spawn_media_action(
            media,
            room_id,
            event_id,
            UserMessageKind::MediaDownloadFailed,
            |media_files, output, event_id, data| async move {
                if let Err(e) = media_files.open_media(&event_id, &data).await {
                    tracing::warn!("failed to open media: {e}");
                    let kind = match e {
                        AppError::UnviewableMedia => UserMessageKind::MediaNotViewable,
                        _ => UserMessageKind::MediaOpenFailed,
                    };
                    show_toast(output.as_ref(), Toast::Error(UserMessage::new(kind)));
                }
            },
        );
    }

    pub(super) fn open_video(
        &mut self,
        media: Arc<dyn MediaPort>,
        room_id: RoomId,
        event_id: String,
    ) {
        if !cfg!(feature = "video") {
            self.play_externally(async move { media.materialize_video(&room_id, &event_id).await });
            return;
        }
        let output = Arc::clone(&self.output);
        publish_video(
            output.as_ref(),
            VideoView {
                visible: true,
                loading: true,
                ..VideoView::default()
            },
        );
        self.spawn_cancellable(async move {
            let opened = match media.materialize_video(&room_id, &event_id).await {
                Ok(path) => VideoView {
                    visible: true,
                    loading: false,
                    path: Some(path),
                    error: UserMessageKind::None,
                },
                Err(e) => {
                    tracing::warn!("failed to materialize video: {e}");
                    VideoView {
                        visible: true,
                        loading: false,
                        path: None,
                        error: UserMessageKind::MediaDownloadFailed,
                    }
                }
            };
            publish_video(output.as_ref(), opened);
        });
    }

    pub(super) fn play_audio_externally(
        &mut self,
        media: Arc<dyn MediaPort>,
        room_id: RoomId,
        event_id: String,
    ) {
        self.play_externally(async move {
            media
                .materialize_audio(&room_id, &event_id, WaveformNeed::Skip)
                .await
        });
    }

    fn play_externally<Fut>(&mut self, fetch: Fut)
    where
        Fut: Future<Output = Result<PathBuf>> + Send + 'static,
    {
        let media_files = Arc::clone(&self.media_files);
        let output = Arc::clone(&self.output);
        self.spawn_cancellable(async move {
            let outcome = match fetch.await {
                Ok(path) => media_files.open_path(&path).await,
                Err(e) => Err(e),
            };
            if let Err(e) = outcome {
                tracing::warn!("failed to play media externally: {e}");
                show_toast(
                    output.as_ref(),
                    Toast::Error(UserMessage::new(UserMessageKind::MediaOpenFailed)),
                );
            }
        });
    }

    pub(super) fn close_video(&mut self) {
        publish_video(self.output.as_ref(), VideoView::default());
    }

    pub(super) fn save_file(
        &mut self,
        media: Arc<dyn MediaPort>,
        room_id: RoomId,
        event_id: String,
        filename: String,
    ) {
        self.spawn_media_action(
            media,
            room_id,
            event_id,
            UserMessageKind::FileDownloadFailed,
            move |media_files, output, _event_id, data| async move {
                match media_files.save_file(&filename, &data).await {
                    Ok(Some(path)) => show_toast(output.as_ref(), Toast::FileSaved(path)),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!("failed to save file: {e}");
                        show_toast(
                            output.as_ref(),
                            Toast::Error(UserMessage::new(UserMessageKind::FileSaveFailed)),
                        );
                    }
                }
            },
        );
    }

    pub(super) async fn cancel_and_drain(&mut self) {
        self.tasks.restart().await;
    }

    pub(super) async fn clear_session(&self) {
        self.media_files.clear_session().await;
    }

    pub(super) async fn drain(&mut self) {
        self.tasks.drain().await;
    }
}
