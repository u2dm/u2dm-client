use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio::time;

use crate::ports::matrix::MatrixPort;
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;

pub(super) struct MediaActions {
    matrix: Arc<dyn MatrixPort>,
    media_files: Arc<dyn MediaFilePort>,
    output: Arc<dyn AppOutputPort>,
    tasks: JoinSet<()>,
}

impl MediaActions {
    pub(super) fn new(
        matrix: Arc<dyn MatrixPort>,
        media_files: Arc<dyn MediaFilePort>,
        output: Arc<dyn AppOutputPort>,
    ) -> Self {
        Self {
            matrix,
            media_files,
            output,
            tasks: JoinSet::new(),
        }
    }

    pub(super) fn open_media(&mut self, event_id: String) {
        self.reap_finished();

        let matrix = Arc::clone(&self.matrix);
        let media_files = Arc::clone(&self.media_files);
        let output = Arc::clone(&self.output);
        self.tasks.spawn(async move {
            match matrix.download_media(&event_id, false).await {
                Ok(data) => {
                    if let Err(e) = media_files.open_media(&event_id, &data).await {
                        tracing::warn!("failed to open media: {e}");
                        output
                            .notify_error(format!("Failed to open media: {e}"))
                            .await;
                    }
                }
                Err(e) => {
                    output
                        .notify_error(format!("Failed to download media: {e}"))
                        .await;
                }
            }
        });
    }

    pub(super) fn save_file(&mut self, event_id: String, filename: String) {
        self.reap_finished();

        let matrix = Arc::clone(&self.matrix);
        let media_files = Arc::clone(&self.media_files);
        let output = Arc::clone(&self.output);
        self.tasks.spawn(async move {
            match matrix.download_media(&event_id, false).await {
                Ok(data) => match media_files.save_file(&filename, &data).await {
                    Ok(Some(path)) => output.file_saved(path).await,
                    Ok(None) => {}
                    Err(e) => {
                        output
                            .notify_error(format!("Failed to save file: {e}"))
                            .await;
                    }
                },
                Err(e) => {
                    output
                        .notify_error(format!("Failed to download file: {e}"))
                        .await;
                }
            }
        });
    }

    pub(super) async fn drain(&mut self) {
        if self.tasks.is_empty() {
            return;
        }

        let count = self.tasks.len();
        tracing::debug!("waiting for {count} in-flight task(s)");
        let result = time::timeout(Duration::from_secs(3), async {
            while self.tasks.join_next().await.is_some() {}
        })
        .await;
        if result.is_err() {
            tracing::warn!("timed out waiting for in-flight tasks, abandoning");
            self.tasks.abort_all();
        }
    }

    fn reap_finished(&mut self) {
        while self.tasks.try_join_next().is_some() {}
    }
}
