use std::future::Future;
use std::sync::Arc;

use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::Toast;
use crate::error::AppError;
use crate::ports::matrix::MediaPort;
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;

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
            match media.download_media(&event_id, false).await {
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

    pub(super) fn open_media(&mut self, media: Arc<dyn MediaPort>, event_id: String) {
        self.spawn_media_action(
            media,
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

    pub(super) fn open_video(&mut self, media: Arc<dyn MediaPort>, event_id: String) {
        let media_files = Arc::clone(&self.media_files);
        let output = Arc::clone(&self.output);
        self.spawn_cancellable(async move {
            let path = match media.materialize_video(&event_id).await {
                Ok(path) => path,
                Err(e) => {
                    tracing::warn!("failed to materialize video: {e}");
                    show_toast(
                        output.as_ref(),
                        Toast::Error(UserMessage::new(UserMessageKind::MediaDownloadFailed)),
                    );
                    return;
                }
            };
            if let Err(e) = media_files.open_path(&path).await {
                tracing::warn!("failed to open video: {e}");
                show_toast(
                    output.as_ref(),
                    Toast::Error(UserMessage::new(UserMessageKind::MediaOpenFailed)),
                );
            }
        });
    }

    pub(super) fn save_file(
        &mut self,
        media: Arc<dyn MediaPort>,
        event_id: String,
        filename: String,
    ) {
        self.spawn_media_action(
            media,
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
