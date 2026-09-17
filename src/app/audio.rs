use std::path::PathBuf;
use std::sync::Arc;

use super::active_timeline::ActiveTimeline;
use super::event::AppEvent;
use super::input::EventSender;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::AudioEnd;
use crate::commands::view::{AudioView, NowPlaying, Toast, TrackFile};
use crate::domain::media::{AudioKind, WaveformNeed};
use crate::domain::room::RoomId;
use crate::domain::timeline::{AudioLookup, AudioTrack};
use crate::ports::matrix::MediaPort;
use crate::ports::output::AppOutputPort;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LocateFor {
    Click,
    Advance,
}

struct Locating {
    request: u64,
    room_id: RoomId,
    purpose: LocateFor,
}

pub(super) struct AudioController {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    issued: u64,
    locating: Option<Locating>,
    now_playing: Option<NowPlaying>,
}

impl AudioController {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("audio"),
            issued: 0,
            locating: None,
            now_playing: None,
        }
    }

    pub(super) fn play(&mut self, timeline: &ActiveTimeline, event_id: String) {
        if self
            .now_playing
            .as_ref()
            .is_some_and(|now| now.event_id == event_id)
        {
            return;
        }
        self.locate(timeline, AudioLookup::Event(event_id), LocateFor::Click);
    }

    fn locate(&mut self, timeline: &ActiveTimeline, lookup: AudioLookup, purpose: LocateFor) {
        self.issued = self.issued.saturating_add(1);
        let request = self.issued;
        match timeline.locate_audio(request, lookup) {
            Some(room_id) => {
                self.locating = Some(Locating {
                    request,
                    room_id,
                    purpose,
                });
            }
            None if purpose == LocateFor::Advance => self.close(),
            None => tracing::debug!("no open timeline to find that audio message in"),
        }
    }

    pub(super) fn located(
        &mut self,
        media: Arc<dyn MediaPort>,
        request: u64,
        track: Option<AudioTrack>,
    ) {
        let Some(locating) = self
            .locating
            .take_if(|locating| locating.request == request)
        else {
            tracing::debug!(request, "dropping a superseded audio lookup");
            return;
        };
        let Some(track) = track else {
            tracing::debug!(request, "the lookup found no audio message to play");
            if locating.purpose == LocateFor::Advance {
                self.close();
            }
            return;
        };
        let AudioTrack {
            event_id,
            sender,
            meta,
        } = track;
        self.now_playing = Some(NowPlaying {
            request,
            room_id: locating.room_id,
            event_id: event_id.clone(),
            sender,
            meta,
            file: TrackFile::Downloading,
        });
        let need = self
            .now_playing
            .as_ref()
            .map_or(WaveformNeed::Skip, |now| WaveformNeed::of(&now.meta));
        self.publish();
        self.fetch(media, request, event_id, need);
    }

    fn fetch(
        &mut self,
        media: Arc<dyn MediaPort>,
        request: u64,
        event_id: String,
        need: WaveformNeed,
    ) {
        self.tasks.cancel_and_detach();
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                fetched = media.materialize_audio(&event_id, need) => fetched.map_err(|e| {
                    tracing::warn!("failed to download audio: {e}");
                    UserMessageKind::MediaDownloadFailed
                }),
            };
            drop(events.send(AppEvent::AudioFetched { request, outcome }));
        });
    }

    pub(super) fn fetched(&mut self, request: u64, outcome: Result<PathBuf, UserMessageKind>) {
        let Some(now) = self
            .now_playing
            .as_mut()
            .filter(|now| now.request == request && now.file == TrackFile::Downloading)
        else {
            tracing::debug!(request, "dropping a superseded audio download");
            return;
        };
        match outcome {
            Ok(path) => {
                now.file = TrackFile::Ready(path);
                self.publish();
            }
            Err(kind) => {
                show_toast(self.output.as_ref(), Toast::Error(UserMessage::new(kind)));
                self.close();
            }
        }
    }

    pub(super) fn ended(&mut self, timeline: &ActiveTimeline, request: u64, end: AudioEnd) {
        let Some(now) = self
            .now_playing
            .as_ref()
            .filter(|now| now.request == request)
        else {
            tracing::debug!(request, "dropping the end of a superseded track");
            return;
        };
        let advances = end == AudioEnd::Finished
            && now.meta.kind == AudioKind::Voice
            && timeline.is_active_room(&now.room_id);
        match end {
            AudioEnd::Failed => {
                show_toast(
                    self.output.as_ref(),
                    Toast::Error(UserMessage::new(UserMessageKind::AudioPlaybackFailed)),
                );
                self.close();
            }
            AudioEnd::Finished if advances => {
                let lookup = AudioLookup::VoiceAfter(now.event_id.clone());
                self.locate(timeline, lookup, LocateFor::Advance);
            }
            AudioEnd::Finished => self.close(),
        }
    }

    pub(super) fn close(&mut self) {
        self.tasks.cancel_and_detach();
        self.locating = None;
        self.now_playing = None;
        self.publish();
    }

    pub(super) async fn restart(&mut self) {
        self.tasks.restart().await;
        self.locating = None;
        self.now_playing = None;
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }

    fn publish(&self) {
        let view = AudioView {
            now_playing: self.now_playing.clone(),
        };
        self.output
            .publish(Box::new(move |state| state.audio = view));
    }

    pub(super) fn abandon_lookup(&mut self) {
        if let Some(Locating {
            purpose: LocateFor::Advance,
            ..
        }) = self.locating.take()
        {
            self.close();
        }
    }
}
