use std::sync::Arc;

use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{StickerView, Toast};
use crate::domain::models::{PackId, RoomId, StickerPacks};
use crate::ports::matrix::StickerPort;
use crate::ports::output::AppOutputPort;

const PREFETCH_BATCH: usize = 12;

pub(super) struct Stickers {
    output: Arc<dyn AppOutputPort>,
    tasks: TaskGroup,
}

impl Stickers {
    pub(super) fn new(output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            output,
            tasks: TaskGroup::new("stickers"),
        }
    }

    pub(super) fn select_room(
        &mut self,
        port: Arc<dyn StickerPort>,
        room_id: RoomId,
        generation: i32,
    ) {
        self.tasks.cancel_and_detach();
        self.publish(StickerView {
            generation,
            loading: true,
            ..StickerView::default()
        });

        let output = Arc::clone(&self.output);
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let work = load_catalog(port, output, room_id, generation);
            tokio::select! {
                () = cancel.cancelled() => {}
                () = work => {}
            }
        });
    }

    pub(super) fn send(
        &mut self,
        port: Arc<dyn StickerPort>,
        room_id: RoomId,
        pack: PackId,
        shortcode: String,
        reply_to: Option<String>,
    ) {
        let output = Arc::clone(&self.output);
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let work = async move {
                let result = port
                    .send_sticker(&room_id, &pack, &shortcode, reply_to.as_deref())
                    .await;
                if let Err(e) = result {
                    tracing::warn!("failed to send sticker: {e}");
                    show_toast(
                        output.as_ref(),
                        Toast::Error(UserMessage::new(UserMessageKind::SendMessageFailed)),
                    );
                }
            };
            tokio::select! {
                () = cancel.cancelled() => {}
                () = work => {}
            }
        });
    }

    pub(super) fn clear_room(&mut self) {
        self.tasks.cancel_and_detach();
        self.publish(StickerView::default());
    }

    pub(super) async fn restart(&mut self) {
        self.tasks.restart().await;
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }

    fn publish(&self, view: StickerView) {
        self.output
            .publish(Box::new(move |state| state.stickers = view));
    }
}

async fn load_catalog(
    port: Arc<dyn StickerPort>,
    output: Arc<dyn AppOutputPort>,
    room_id: RoomId,
    generation: i32,
) {
    let catalog = match port.catalog(&room_id).await {
        Ok(catalog) => catalog,
        Err(e) => {
            tracing::warn!(%room_id, "failed to load sticker packs: {e}");
            publish_packs(&output, generation, Arc::from(Vec::new()), 0, false);
            return;
        }
    };

    let room_encrypted = catalog.room_encrypted;
    let mxcs: Vec<String> = catalog
        .packs
        .iter()
        .flat_map(|pack| pack.images.iter().map(|image| image.mxc.clone()))
        .collect();
    let packs: StickerPacks = Arc::from(catalog.packs);

    tracing::debug!(
        %room_id,
        packs = packs.len(),
        stickers = mxcs.len(),
        "loaded the sticker catalog"
    );
    publish_packs(&output, generation, Arc::clone(&packs), 0, room_encrypted);

    let mut ready_images = 0;
    for batch in mxcs.chunks(PREFETCH_BATCH) {
        let landed = port.prefetch(batch).await;
        if landed > 0 {
            ready_images += landed;
            publish_packs(
                &output,
                generation,
                Arc::clone(&packs),
                ready_images,
                room_encrypted,
            );
        }
    }
}

fn publish_packs(
    output: &Arc<dyn AppOutputPort>,
    generation: i32,
    packs: StickerPacks,
    ready_images: usize,
    room_encrypted: bool,
) {
    output.publish(Box::new(move |state| {
        state.stickers = StickerView {
            generation,
            packs,
            ready_images,
            room_encrypted,
            loading: false,
        };
    }));
}
