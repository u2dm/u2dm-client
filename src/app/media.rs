use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio::time;
use tokio_util::sync::CancellationToken;

use super::task_group::record_join;
use crate::commands::Effect;
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

    pub(super) fn open_media(&mut self, media: Arc<dyn MediaPort>, event_id: String) {
        self.reap_finished();

        let media_files = Arc::clone(&self.media_files);
        let output = Arc::clone(&self.output);
        let cancel = self.cancel.clone();
        self.tasks.spawn(async move {
            let work = async move {
                match media.download_media(&event_id, false).await {
                    Ok(data) => {
                        if let Err(e) = media_files.open_media(&event_id, &data).await {
                            tracing::warn!("failed to open media: {e}");
                            output
                                .emit(Effect::Toast(format!("Failed to open media: {e}")))
                                .await;
                        }
                    }
                    Err(e) => {
                        output
                            .emit(Effect::Toast(format!("Failed to download media: {e}")))
                            .await;
                    }
                }
            };
            tokio::select! {
                () = cancel.cancelled() => {}
                () = work => {}
            }
        });
    }

    pub(super) fn save_file(
        &mut self,
        media: Arc<dyn MediaPort>,
        event_id: String,
        filename: String,
    ) {
        self.reap_finished();

        let media_files = Arc::clone(&self.media_files);
        let output = Arc::clone(&self.output);
        let cancel = self.cancel.clone();
        self.tasks.spawn(async move {
            let work = async move {
                match media.download_media(&event_id, false).await {
                    Ok(data) => match media_files.save_file(&filename, &data).await {
                        Ok(Some(path)) => output.emit(Effect::FileSaved { path }).await,
                        Ok(None) => {}
                        Err(e) => {
                            output
                                .emit(Effect::Toast(format!("Failed to save file: {e}")))
                                .await;
                        }
                    },
                    Err(e) => {
                        output
                            .emit(Effect::Toast(format!("Failed to download file: {e}")))
                            .await;
                    }
                }
            };
            tokio::select! {
                () = cancel.cancelled() => {}
                () = work => {}
            }
        });
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
