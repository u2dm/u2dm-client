use std::sync::Arc;

use super::AppService;
use crate::app::event::CallEvent;
use crate::domain::call::DeviceId;
use crate::domain::user_info::UserId;

impl AppService {
    pub(super) fn sync_call_room(&mut self) {
        let call_room = self.calls.joined_room().cloned();
        if call_room == self.call_room_subscribed {
            return;
        }
        if let Some(sync) = self.port(|a| &a.sync) {
            sync.set_call_room(call_room.as_ref());
            self.call_room_subscribed = call_room;
        }
    }

    pub(super) fn follow_calls(&mut self) {
        self.calls
            .selected(self.selection.room.as_ref(), &self.room_directory);
    }

    pub(super) fn handle_call(&mut self, event: CallEvent) {
        match event {
            CallEvent::Signal { session, signal } => {
                if self.lifecycle.runs(session) {
                    self.calls.signal(signal, &self.room_directory);
                }
            }
            CallEvent::Transports { session, found } => {
                if self.lifecycle.runs(session) {
                    self.calls.transports_found(found, &self.room_directory);
                }
            }
            CallEvent::FacesReady { session, ready } => {
                if self.lifecycle.runs(session) {
                    self.calls.faces_fetched(ready, &self.room_directory);
                }
            }
            CallEvent::RingWindow { ring, deadline } => {
                self.calls
                    .ring_window(&ring, deadline, &self.room_directory);
            }
            CallEvent::Declined { room_id, sent } => {
                if self.held_session.running().is_some() {
                    self.calls
                        .decline_settled(&room_id, sent, &self.room_directory);
                }
            }
            CallEvent::DevicesListed { listing, lists } => {
                self.calls
                    .devices_listed(listing, lists, &self.room_directory);
            }
            CallEvent::Preview { preview, outcome } => {
                self.calls
                    .preview_shown(preview, outcome, &self.room_directory);
            }
            CallEvent::DeviceTest { test, outcome } => {
                self.calls.test_heard(test, outcome, &self.room_directory);
            }
            event => {
                if event.call().is_some_and(|call| self.calls.is_current(call)) {
                    self.calls.call_event(event, &self.room_directory);
                } else {
                    tracing::debug!(
                        event = event.label(),
                        "an event of a call that ended, dropping"
                    );
                }
            }
        }
    }

    pub(super) fn begin_calls(&mut self, session: u64) {
        let Some((port, rtc, user, device)) = self.held_session.running().map(|a| {
            (
                Arc::clone(&a.call_signals),
                Arc::clone(&a.rtc_membership),
                UserId::new(a.session.user_id.clone()),
                DeviceId::new(a.session.device_id.clone()),
            )
        }) else {
            return;
        };
        self.calls
            .begin_session(&mut self.background, port, rtc, session, user, device);
        self.calls
            .trust_changed(self.encryption.is_verified(), &self.room_directory);
    }
}
