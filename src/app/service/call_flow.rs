use std::sync::Arc;

use super::AppService;
use crate::app::event::CallEvent;
use crate::commands::ui::CallCommand;
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

    pub(super) fn handle_call_command(&mut self, command: CallCommand) {
        let directory = &self.room_directory;
        match command {
            CallCommand::DeclineCall(room_id) => self.calls.decline(&room_id, directory),
            CallCommand::SilenceRing => self.calls.silence(directory),
            CallCommand::OpenCallSheet { room_id, video } => {
                self.calls.open_sheet(&room_id, video, directory);
            }
            CallCommand::CloseCallSheet => self.calls.close_sheet(directory),
            CallCommand::SheetCamera(on) => self.calls.sheet_camera(on, directory),
            CallCommand::TestCallDevice { kind, on } => self.calls.test_device(kind, on, directory),
            CallCommand::StartCall {
                room_id,
                video,
                media,
                leaving,
            } => self
                .calls
                .start_call(room_id, video, media, leaving, directory),
            CallCommand::JoinCall {
                room_id,
                media,
                leaving,
            } => self.calls.join_call(room_id, media, leaving, directory),
            CallCommand::AnswerCall {
                room_id,
                camera,
                leaving,
            } => self.calls.answer(room_id, camera, leaving, directory),
            CallCommand::LeaveCall => self.calls.leave_call(directory),
            CallCommand::RetryFailedCall => self.calls.retry_failed(directory),
            CallCommand::DismissFailedCall => self.calls.dismiss_failed(directory),
            CallCommand::AnnounceCallAgain => self.calls.announce_again(directory),
            CallCommand::SetMicrophone(on) => self.calls.set_microphone(on, directory),
            CallCommand::SetCamera(on) => self.calls.set_camera(on, directory),
            CallCommand::ShareScreen(on) => self.calls.set_screen_share(on, directory),
            CallCommand::ChangeScreenShare => self.calls.change_screen_share(directory),
            CallCommand::UseCallDevice { kind, key } => {
                self.calls.use_device(kind, key.as_ref(), directory);
            }
            CallCommand::RefreshCallDevices => self.calls.refresh_devices(),
            CallCommand::FocusCallTile(key) => self.calls.focus_call_tile(&key, directory),
            CallCommand::UnpinCallTile => self.calls.unpin_call_tile(directory),
            CallCommand::RingDevices(open) => self.calls.ring_devices(open, directory),
            CallCommand::ShowCallGallery => self.calls.show_call_gallery(directory),
            CallCommand::ViewCallShare => self.calls.view_call_share(directory),
            CallCommand::ShowSelfView(on) => self.calls.show_self_view(on, directory),
            CallCommand::ShowSharePreview(on) => self.calls.show_share_preview(on, directory),
            CallCommand::FollowSpeaker(on) => self.calls.follow_speaker(on, directory),
            CallCommand::MuteForMe { user_id, on } => {
                self.calls.mute_for_me(&user_id, on, directory);
            }
            CallCommand::WatchMicrophone(on) => self.calls.watch_microphone(on, directory),
            CallCommand::WatchConnections(on) => self.calls.watch_connections(on, directory),
            CallCommand::RaiseHand(on) => self.calls.raise_hand(on, directory),
            CallCommand::SendCallReaction(name) => self.calls.send_reaction(&name, directory),
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
