use std::collections::HashMap;
use std::sync::Arc;

use super::event::{AppEvent, PacksEvent};
use super::input::EventSender;
use super::send_lanes::SendLanes;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{PackView, Toast};
use crate::domain::image_pack::{PackId, PackOrigin};
use crate::domain::room::RoomId;
use crate::error::AppError;
use crate::ports::matrix::{ImagePackPort, PackCatalog};
use crate::ports::output::AppOutputPort;

const PREFETCH_BATCH: usize = 12;

pub(super) struct Packs {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    global_writes: TaskGroup,
    shown: PackView,
    room: Option<RoomId>,
    global_ops: HashMap<PackId, u64>,
    last_global_op: u64,
}

impl Packs {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("packs"),
            global_writes: TaskGroup::new("pack-global-writes"),
            shown: PackView::default(),
            room: None,
            global_ops: HashMap::new(),
            last_global_op: 0,
        }
    }

    pub(super) fn select_room(
        &mut self,
        port: Arc<dyn ImagePackPort>,
        room_id: RoomId,
        generation: i32,
    ) {
        self.tasks.cancel_and_detach();
        self.room = Some(room_id.clone());
        self.show(PackView {
            generation,
            loading: true,
            ..PackView::default()
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

    pub(super) fn catalog_loaded(&mut self, generation: i32, catalog: PackCatalog) {
        if !self.loads(generation) {
            tracing::debug!(generation, "dropping a superseded pack catalog");
            return;
        }
        self.show(PackView {
            generation,
            packs: Arc::from(catalog.packs),
            ready_images: 0,
            room_encrypted: catalog.room_encrypted,
            loading: false,
        });
    }

    pub(super) fn images_ready(&mut self, generation: i32, ready: usize) {
        if !self.shows(generation) {
            tracing::debug!(generation, "dropping superseded pack downloads");
            return;
        }
        let mut view = self.shown.clone();
        view.ready_images = view.ready_images.saturating_add(ready);
        self.show(view);
    }

    pub(super) fn send(
        &self,
        lanes: &mut SendLanes,
        port: Arc<dyn ImagePackPort>,
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
                let kind = match e {
                    AppError::SendNotAllowed => UserMessageKind::SendNotAllowed,
                    _ => UserMessageKind::SendMessageFailed,
                };
                show_toast(output.as_ref(), Toast::Error(UserMessage::new(kind)));
            }
        });
    }

    pub(super) fn set_global(&mut self, port: Arc<dyn ImagePackPort>, pack: PackId, global: bool) {
        let origin = PackOrigin::room_pack(global);
        let mut packs = self.shown.packs.to_vec();
        let Some(flipped) = packs
            .iter_mut()
            .find(|shown| shown.id == pack && shown.origin != PackOrigin::Personal)
        else {
            tracing::debug!(%pack, "only a shown room pack can be shared");
            return;
        };
        if flipped.origin == origin {
            return;
        }
        flipped.origin = origin;
        self.show(PackView {
            packs: Arc::from(packs),
            ..self.shown.clone()
        });

        self.last_global_op = self.last_global_op.wrapping_add(1);
        let op = self.last_global_op;
        self.global_ops.insert(pack.clone(), op);
        let events = self.events.clone();
        let cancel = self.global_writes.token();
        self.global_writes.spawn(async move {
            let written = tokio::select! {
                () = cancel.cancelled() => return,
                written = port.set_global(&pack, global) => written,
            };
            if let Err(e) = written {
                tracing::warn!(%pack, global, "failed to change where a pack is used: {e}");
                report(&events, PacksEvent::GlobalFailed { op, pack });
            }
        });
    }

    pub(super) fn global_failed(&mut self, port: Arc<dyn ImagePackPort>, op: u64, pack: &PackId) {
        if self.global_ops.get(pack) != Some(&op) {
            tracing::debug!(%pack, op, "dropping a superseded pack sharing failure");
            return;
        }
        self.global_ops.remove(pack);
        show_toast(
            self.output.as_ref(),
            Toast::Error(UserMessage::new(UserMessageKind::PackSettingFailed)),
        );
        if let Some(room_id) = self.room.clone() {
            self.select_room(port, room_id, self.shown.generation);
        }
    }

    pub(super) fn clear_room(&mut self) {
        self.tasks.cancel_and_detach();
        self.room = None;
        self.show(PackView::default());
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.tasks.restart(), self.global_writes.restart());
        self.shown = PackView::default();
        self.room = None;
        self.global_ops.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.tasks.shutdown(), self.global_writes.shutdown());
    }

    pub(super) fn emote_mxc(&self, pack: &PackId, shortcode: &str) -> Option<String> {
        self.shown
            .packs
            .iter()
            .find(|shown| shown.id == *pack)?
            .images
            .iter()
            .find(|image| image.shortcode == shortcode && image.usage.offers_emoticon())
            .map(|image| image.mxc.clone())
    }

    fn loads(&self, generation: i32) -> bool {
        self.shown.loading && self.shown.generation == generation
    }

    fn shows(&self, generation: i32) -> bool {
        !self.shown.loading && self.shown.generation == generation
    }

    fn show(&mut self, view: PackView) {
        self.shown = view.clone();
        self.output
            .publish(Box::new(move |state| state.image_packs = view));
    }
}

async fn load_catalog(
    port: Arc<dyn ImagePackPort>,
    events: EventSender,
    room_id: RoomId,
    generation: i32,
) {
    let catalog = match port.catalog(&room_id).await {
        Ok(catalog) => catalog,
        Err(e) => {
            tracing::warn!(%room_id, "failed to load image packs: {e}");
            let failed = PacksEvent::CatalogLoaded {
                generation,
                catalog: PackCatalog::default(),
            };
            report(&events, failed);
            return;
        }
    };

    let avatars = catalog.packs.iter().filter_map(|pack| pack.avatar.clone());
    let images = catalog
        .packs
        .iter()
        .flat_map(|pack| pack.images.iter().map(|image| image.mxc.clone()));
    let mxcs: Vec<String> = avatars.chain(images).collect();

    tracing::debug!(
        %room_id,
        packs = catalog.packs.len(),
        files = mxcs.len(),
        "loaded the pack catalog"
    );
    let loaded = PacksEvent::CatalogLoaded {
        generation,
        catalog,
    };
    report(&events, loaded);

    for batch in mxcs.chunks(PREFETCH_BATCH) {
        let ready = port.prefetch(batch).await;
        if ready > 0 {
            report(&events, PacksEvent::ImagesReady { generation, ready });
        }
    }
}

fn report(events: &EventSender, event: PacksEvent) {
    drop(events.send(AppEvent::Packs(event)));
}
