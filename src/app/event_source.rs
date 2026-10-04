use std::sync::Arc;

use super::active_timeline::ActiveTimeline;
use crate::commands::view::SourceState;
use crate::domain::timeline::EventSource;
use crate::ports::output::AppOutputPort;

struct Awaited {
    request: u64,
    event_id: String,
}

pub(super) struct EventSourceViewer {
    output: Arc<dyn AppOutputPort>,
    requests: u64,
    awaited: Option<Awaited>,
    shown: bool,
}

impl EventSourceViewer {
    pub(super) fn new(output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            output,
            requests: 0,
            awaited: None,
            shown: false,
        }
    }

    pub(super) fn open(&mut self, timeline: &ActiveTimeline, event_id: String) {
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        if timeline.locate_source(request, event_id.clone()) {
            self.awaited = Some(Awaited {
                request,
                event_id: event_id.clone(),
            });
            self.publish(SourceState::Locating { event_id });
        } else {
            self.awaited = None;
            self.publish(SourceState::Unavailable { event_id });
        }
    }

    pub(super) fn located(&mut self, request: u64, source: Option<Box<EventSource>>) {
        let Some(awaited) = self.awaited.take_if(|awaited| awaited.request == request) else {
            tracing::debug!(request, "dropping a source lookup nobody waits for");
            return;
        };
        self.publish(match source {
            Some(source) => SourceState::Ready(Arc::new(*source)),
            None => SourceState::Unavailable {
                event_id: awaited.event_id,
            },
        });
    }

    pub(super) fn retarget(&mut self, timeline: &ActiveTimeline) {
        if let Some(awaited) = self.awaited.take() {
            self.open(timeline, awaited.event_id);
        }
    }

    pub(super) fn close(&mut self) {
        self.awaited = None;
        if self.shown {
            self.publish(SourceState::Closed);
        }
    }

    pub(super) fn reset(&mut self) {
        self.awaited = None;
        self.shown = false;
    }

    fn publish(&mut self, state: SourceState) {
        self.shown = state != SourceState::Closed;
        self.output
            .publish(Box::new(move |view| view.source = state));
    }
}
