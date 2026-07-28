use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio::time;
use tokio_util::sync::CancellationToken;

use super::show_toast;
use super::task_group::record_join;
use crate::commands::{Toast, UserMessage, UserMessageKind};
use crate::ports::matrix::MediaPort;
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;

const GROUP: &str = "media";

pub(super) struct MediaActions {
    media_files: Arc<dyn MediaFilePort>,
    output: Arc<dyn AppOutputPort>,
    tasks: JoinSet<()>,
    cancel: CancellationToken,
}

impl MediaActions {
    pub(super) fn new(media_files: Arc<dyn MediaFilePort>, output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            media_files,
            output,
            tasks: JoinSet::new(),
            cancel: CancellationToken::new(),
        }
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
        self.reap_finished();

        let media_files = Arc::clone(&self.media_files);
        let output = Arc::clone(&self.output);
        let cancel = self.cancel.clone();
        self.tasks.spawn(async move {
            let work = async move {
                match media.download_media(&event_id, false).await {
                    Ok(data) => act(media_files, output, event_id, data).await,
                    Err(e) => {
                        show_toast(
                            output.as_ref(),
                            Toast::Error(UserMessage::about(download_failure, &e)),
                        );
                    }
                }
            };
            tokio::select! {
                () = cancel.cancelled() => {}
                () = work => {}
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
                    show_toast(
                        output.as_ref(),
                        Toast::Error(UserMessage::about(UserMessageKind::MediaOpenFailed, &e)),
                    );
                }
            },
        );
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
                        show_toast(
                            output.as_ref(),
                            Toast::Error(UserMessage::about(UserMessageKind::FileSaveFailed, &e)),
                        );
                    }
                }
            },
        );
    }

    pub(super) async fn cancel_and_drain(&mut self) {
        self.cancel.cancel();
        self.drain().await;
        self.cancel = CancellationToken::new();
    }

    pub(super) async fn clear_session(&self) {
        self.media_files.clear_session().await;
    }

    pub(super) async fn drain(&mut self) {
        if self.tasks.is_empty() {
            return;
        }

        let count = self.tasks.len();
        tracing::debug!("waiting for {count} in-flight task(s)");
        let result = time::timeout(Duration::from_secs(3), async {
            while let Some(joined) = self.tasks.join_next().await {
                record_join(GROUP, joined);
            }
        })
        .await;
        if result.is_err() {
            tracing::warn!(
                group = GROUP,
                stragglers = self.tasks.len(),
                "timed out waiting for in-flight tasks, aborting"
            );
            self.tasks.abort_all();
            while let Some(joined) = self.tasks.join_next().await {
                record_join(GROUP, joined);
            }
        }
    }

    fn reap_finished(&mut self) {
        while let Some(joined) = self.tasks.try_join_next() {
            record_join(GROUP, joined);
        }
    }
}
