use std::sync::Arc;

use super::event::{AppEvent, StickersEvent};
use super::input::EventSender;
use super::send_lanes::SendLanes;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{StickerView, Toast};
use crate::domain::room::RoomId;
use crate::domain::sticker::PackId;
use crate::ports::matrix::{StickerCatalog, StickerPort};
use crate::ports::output::AppOutputPort;

const PREFETCH_BATCH: usize = 12;

pub(super) struct Stickers {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    shown: StickerView,
}

impl Stickers {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("stickers"),
            shown: StickerView::default(),
        }
    }

    pub(super) fn select_room(
        &mut self,
        port: Arc<dyn StickerPort>,
        room_id: RoomId,
        generation: i32,
    ) {
        self.tasks.cancel_and_detach();
        self.show(StickerView {
            generation,
            loading: true,
            ..StickerView::default()
        });

        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let work = load_catalog(port, events, room_id, generation);
            tokio::select! {
                () = cancel.cancelled() => {}
                () = work => {}
            }
        });
    }

    pub(super) fn catalog_loaded(&mut self, generation: i32, catalog: StickerCatalog) {
        if !self.loads(generation) {
            tracing::debug!(generation, "dropping a superseded sticker catalog");
            return;
        }
        self.show(StickerView {
            generation,
            packs: Arc::from(catalog.packs),
            ready_images: 0,
            room_encrypted: catalog.room_encrypted,
            loading: false,
        });
    }

    pub(super) fn images_ready(&mut self, generation: i32, ready: usize) {
        if !self.shows(generation) {
            tracing::debug!(generation, "dropping superseded sticker downloads");
            return;
        }
        let mut view = self.shown.clone();
        view.ready_images = view.ready_images.saturating_add(ready);
        self.show(view);
    }

    pub(super) fn send(
        &self,
        lanes: &mut SendLanes,
        port: Arc<dyn StickerPort>,
        room_id: RoomId,
        pack: PackId,
        shortcode: String,
        reply_to: Option<String>,
    ) {
        let output = Arc::clone(&self.output);
        lanes.spawn(room_id.clone(), async move {
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
        });
    }

    pub(super) fn clear_room(&mut self) {
        self.tasks.cancel_and_detach();
        self.show(StickerView::default());
    }

    pub(super) async fn restart(&mut self) {
        self.tasks.restart().await;
        self.shown = StickerView::default();
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }

    fn loads(&self, generation: i32) -> bool {
        self.shown.loading && self.shown.generation == generation
    }

    fn shows(&self, generation: i32) -> bool {
        !self.shown.loading && self.shown.generation == generation
    }

    fn show(&mut self, view: StickerView) {
        self.shown = view.clone();
        self.output
            .publish(Box::new(move |state| state.stickers = view));
    }
}

async fn load_catalog(
    port: Arc<dyn StickerPort>,
    events: EventSender,
    room_id: RoomId,
    generation: i32,
) {
    let catalog = match port.catalog(&room_id).await {
        Ok(catalog) => catalog,
        Err(e) => {
            tracing::warn!(%room_id, "failed to load sticker packs: {e}");
            let failed = StickersEvent::CatalogLoaded {
                generation,
                catalog: StickerCatalog::default(),
            };
            report(&events, failed);
            return;
        }
    };

    let mxcs: Vec<String> = catalog
        .packs
        .iter()
        .flat_map(|pack| pack.images.iter().map(|image| image.mxc.clone()))
        .collect();

    tracing::debug!(
        %room_id,
        packs = catalog.packs.len(),
        stickers = mxcs.len(),
        "loaded the sticker catalog"
    );
    let loaded = StickersEvent::CatalogLoaded {
        generation,
        catalog,
    };
    report(&events, loaded);

    for batch in mxcs.chunks(PREFETCH_BATCH) {
        let ready = port.prefetch(batch).await;
        if ready > 0 {
            report(&events, StickersEvent::ImagesReady { generation, ready });
        }
    }
}

fn report(events: &EventSender, event: StickersEvent) {
    drop(events.send(AppEvent::Stickers(event)));
}
