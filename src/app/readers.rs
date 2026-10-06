use std::collections::HashSet;
use std::sync::Arc;

use super::active_timeline::ActiveTimeline;
use super::event::{AppEvent, ReadersEvent};
use super::input::EventSender;
use super::space_index::AVATAR_BATCH;
use super::task_group::TaskGroup;
use crate::commands::view::{ReadersStatus, ReadersView};
use crate::domain::message::TimelineMessage;
use crate::domain::room::RoomId;
use crate::domain::room_info::{Reader, sort_readers};
use crate::domain::timeline::MessageReaders;
use crate::ports::matrix::RoomInfoPort;
use crate::ports::output::AppOutputPort;

const READER_PAGE: usize = 50;

type Readers = Arc<[Arc<Reader>]>;

enum Lookup {
    Locating,
    Naming(Arc<TimelineMessage>),
    Ready(ReaderPages),
    Unavailable,
}

struct ReaderPages {
    message: Arc<TimelineMessage>,
    readers: Readers,
    shown: usize,
    rows: Readers,
    has_more: bool,
    requested: HashSet<String>,
    avatars_ready: usize,
}

impl ReaderPages {
    fn new(message: Arc<TimelineMessage>, mut readers: Vec<Reader>) -> Self {
        sort_readers(&mut readers);
        Self {
            message,
            readers: readers.into_iter().map(Arc::new).collect(),
            shown: READER_PAGE,
            rows: Arc::from(Vec::new()),
            has_more: false,
            requested: HashSet::new(),
            avatars_ready: 0,
        }
    }

    fn reveal(&mut self) -> bool {
        let rows: Readers = self
            .readers
            .iter()
            .take(self.shown)
            .map(Arc::clone)
            .collect();
        let has_more = self.readers.len() > rows.len();
        if *self.rows == *rows && self.has_more == has_more {
            return false;
        }
        self.rows = rows;
        self.has_more = has_more;
        true
    }

    fn unrequested_avatars(&mut self) -> Vec<String> {
        let Self {
            rows, requested, ..
        } = self;
        rows.iter()
            .filter_map(|reader| reader.avatar_mxc.clone())
            .filter(|mxc| requested.insert(mxc.clone()))
            .collect()
    }
}

struct Shown {
    request: u64,
    event_id: String,
    lookup: Lookup,
}

impl Shown {
    fn view(&self, pages_landed: i32) -> ReadersView {
        let (status, pages) = match &self.lookup {
            Lookup::Locating | Lookup::Naming(_) => (ReadersStatus::Locating, None),
            Lookup::Ready(pages) => (ReadersStatus::Ready, Some(pages)),
            Lookup::Unavailable => (ReadersStatus::Unavailable, None),
        };
        ReadersView {
            status,
            message: pages.map(|pages| Arc::clone(&pages.message)),
            total: pages.map_or(0, |pages| pages.readers.len()),
            rows: pages.map_or_else(|| Arc::from(Vec::new()), |pages| Arc::clone(&pages.rows)),
            has_more: pages.is_some_and(|pages| pages.has_more),
            pages_landed,
            avatars_ready: pages.map_or(0, |pages| pages.avatars_ready),
        }
    }

    fn pages_mut(&mut self) -> Option<&mut ReaderPages> {
        match &mut self.lookup {
            Lookup::Ready(pages) => Some(pages),
            Lookup::Locating | Lookup::Naming(_) | Lookup::Unavailable => None,
        }
    }
}

pub(super) struct ReaderList {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    requests: u64,
    pages_landed: i32,
    shown: Option<Shown>,
}

impl ReaderList {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("readers"),
            requests: 0,
            pages_landed: 0,
            shown: None,
        }
    }

    pub(super) fn open(&mut self, timeline: &ActiveTimeline, event_id: String) {
        self.tasks.cancel_and_detach();
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        let lookup = if timeline.locate_readers(request, event_id.clone()) {
            Lookup::Locating
        } else {
            Lookup::Unavailable
        };
        self.shown = Some(Shown {
            request,
            event_id,
            lookup,
        });
        self.publish();
    }

    pub(super) fn located(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        room_id: RoomId,
        request: u64,
        readers: Option<Box<MessageReaders>>,
    ) {
        let Some(shown) = self
            .shown
            .as_mut()
            .filter(|shown| shown.request == request && matches!(shown.lookup, Lookup::Locating))
        else {
            tracing::debug!(request, "dropping a readers lookup nobody waits for");
            return;
        };
        let Some(found) = readers else {
            shown.lookup = Lookup::Unavailable;
            self.publish();
            return;
        };
        let MessageReaders { message, readers } = *found;
        let message = Arc::new(message);
        if readers.is_empty() {
            shown.lookup = Lookup::Ready(ReaderPages::new(message, Vec::new()));
            self.reveal(port);
            return;
        }
        shown.lookup = Lookup::Naming(message);
        self.spawn_naming(port, room_id, request, readers);
    }

    pub(super) fn named(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        request: u64,
        readers: Vec<Reader>,
    ) {
        let Some(shown) = self.shown.as_mut().filter(|shown| shown.request == request) else {
            tracing::debug!(request, "dropping reader names nobody waits for");
            return;
        };
        let Lookup::Naming(message) = &shown.lookup else {
            return;
        };
        shown.lookup = Lookup::Ready(ReaderPages::new(Arc::clone(message), readers));
        self.reveal(port);
    }

    pub(super) fn page(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(pages) = self
            .shown
            .as_mut()
            .and_then(Shown::pages_mut)
            .filter(|pages| pages.has_more)
        else {
            return;
        };
        pages.shown = pages.shown.saturating_add(READER_PAGE);
        self.reveal(port);
    }

    pub(super) fn avatars_ready(&mut self, request: u64, ready: usize) {
        let Some(pages) = self
            .shown
            .as_mut()
            .filter(|shown| shown.request == request)
            .and_then(Shown::pages_mut)
        else {
            return;
        };
        pages.avatars_ready = pages.avatars_ready.saturating_add(ready);
        self.publish();
    }

    pub(super) fn retarget(&mut self, timeline: &ActiveTimeline) {
        let Some(event_id) = self
            .shown
            .as_ref()
            .filter(|shown| matches!(shown.lookup, Lookup::Locating))
            .map(|shown| shown.event_id.clone())
        else {
            return;
        };
        self.open(timeline, event_id);
    }

    pub(super) fn close(&mut self) {
        if self.shown.take().is_none() {
            return;
        }
        self.tasks.cancel_and_detach();
        self.publish();
    }

    pub(super) async fn restart(&mut self) {
        self.tasks.restart().await;
        self.shown = None;
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }

    fn reveal(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        let request = shown.request;
        let Some(pages) = shown.pages_mut() else {
            return;
        };
        if pages.reveal() {
            self.pages_landed = self.pages_landed.wrapping_add(1);
        }
        let wanted = pages.unrequested_avatars();
        self.publish();
        self.spawn_avatars(port, request, wanted);
    }

    fn spawn_naming(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        room_id: RoomId,
        request: u64,
        user_ids: Vec<String>,
    ) {
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let named = tokio::select! {
                () = cancel.cancelled() => return,
                named = port.readers(&room_id, &user_ids) => named,
            };
            let readers = named.unwrap_or_else(|e| {
                tracing::warn!(%room_id, "failed to read who the readers are: {e}");
                user_ids.into_iter().map(Reader::unknown).collect()
            });
            drop(events.send(AppEvent::Readers(ReadersEvent::Named { request, readers })));
        });
    }

    fn spawn_avatars(&mut self, port: Arc<dyn RoomInfoPort>, request: u64, mxcs: Vec<String>) {
        if mxcs.is_empty() {
            return;
        }
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            for batch in mxcs.chunks(AVATAR_BATCH) {
                let ready = tokio::select! {
                    () = cancel.cancelled() => return,
                    ready = port.fetch_avatars(batch) => ready,
                };
                if ready > 0 {
                    let landed = ReadersEvent::AvatarsReady { request, ready };
                    drop(events.send(AppEvent::Readers(landed)));
                }
            }
        });
    }

    fn publish(&self) {
        let view = self
            .shown
            .as_ref()
            .map_or_else(ReadersView::default, |shown| shown.view(self.pages_landed));
        self.output
            .publish(Box::new(move |state| state.readers = view));
    }
}
