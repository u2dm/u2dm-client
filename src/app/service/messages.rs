use std::sync::Arc;

use super::AppService;
use crate::app::event::{
    MentionsEvent, MessageActionEvent, PacksEvent, ReadersEvent, TimelineEvent,
};
use crate::app::mentions::Suggestion;
use crate::app::polls;
use crate::commands::ui::MessageDraft;
use crate::domain::emote::ComposerText;
use crate::domain::image_pack::PackId;
use crate::domain::media::AttachmentPick;
use crate::domain::message::{MessageEdit, PinChange};
use crate::domain::poll::PollDraft;
use crate::domain::room::RoomId;
use crate::domain::timeline::{AudioTrack, FailedSend, MessageReaders};
use crate::domain::user_info::UserId;
use crate::ports::matrix::AuthenticatedSession;

impl AppService {
    pub(super) fn page_readers(&mut self) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.readers.page(port);
        }
    }

    fn settle_readers_lookup(
        &mut self,
        room_id: RoomId,
        generation: i32,
        request: u64,
        readers: Option<Box<MessageReaders>>,
    ) {
        if !self.active_timeline.is_current(&room_id, generation) {
            return;
        }
        if let Some(port) = self.port(|a| &a.room_info) {
            self.readers.located(port, room_id, request, readers);
        }
    }

    pub(super) fn suggest_mentions(&mut self, room_id: RoomId, query: String, recent: &[UserId]) {
        if self.selection.room.as_ref() != Some(&room_id) {
            return;
        }
        let Some(port) = self.port(|a| &a.room_info) else {
            return;
        };
        let Some(room) = self.room_directory.room(room_id.as_ref()) else {
            return;
        };
        let suggestion = Suggestion {
            joined_count: room.member_count,
            offers_room: room.message_permissions.notify_room,
            own_user: self
                .held_session
                .running()
                .map(|session| session.session.user_id.clone())
                .unwrap_or_default(),
            room_id,
            query,
            recent: recent.iter().map(ToString::to_string).collect(),
        };
        self.mentions.suggest(port, suggestion);
    }

    pub(super) fn handle_mentions(&mut self, event: MentionsEvent) {
        match event {
            MentionsEvent::RosterLoaded {
                request,
                joined_count,
                roster,
            } => {
                if let Some(port) = self.port(|a| &a.room_info) {
                    self.mentions
                        .roster_loaded(port, request, joined_count, roster);
                }
            }
            MentionsEvent::AvatarsReady { request, ready } => {
                self.mentions.avatars_ready(request, ready);
            }
        }
    }

    pub(super) fn handle_packs(&mut self, event: PacksEvent) {
        match event {
            PacksEvent::CatalogLoaded {
                generation,
                catalog,
            } => self.packs.catalog_loaded(generation, catalog),
            PacksEvent::ImagesReady { generation, ready } => {
                self.packs.images_ready(generation, ready);
            }
            PacksEvent::GlobalFailed { op, pack } => {
                if let Some(port) = self.port(|a| &a.image_packs) {
                    self.packs.global_failed(port, op, &pack);
                }
            }
        }
    }

    pub(super) fn handle_readers(&mut self, event: ReadersEvent) {
        match event {
            ReadersEvent::Named { request, readers } => {
                if let Some(port) = self.port(|a| &a.room_info) {
                    self.readers.named(port, request, readers);
                }
            }
            ReadersEvent::AvatarsReady { request, ready } => {
                self.readers.avatars_ready(request, ready);
            }
        }
    }

    pub(super) fn handle_message_action(&mut self, event: MessageActionEvent) {
        match event {
            MessageActionEvent::LinkResolved { request, link } => {
                self.message_actions.link_resolved(request, link);
            }
            MessageActionEvent::PinSettled {
                request,
                event_id,
                outcome,
            } => self.pinned.pin_settled(request, &event_id, outcome),
            MessageActionEvent::DeletionSettled { event_id, outcome } => {
                self.message_actions.deletion_settled(&event_id, outcome);
            }
        }
    }

    pub(super) async fn handle_timeline_event(&mut self, event: TimelineEvent) {
        match event {
            TimelineEvent::Advanced {
                room_id,
                generation,
                advance,
            } => self
                .active_timeline
                .settle_read_position(&room_id, generation, advance),
            TimelineEvent::PaginationCompleted {
                room_id,
                generation,
                direction,
                outcome,
            } => self
                .active_timeline
                .complete_pagination(&room_id, generation, direction, outcome),
            TimelineEvent::Refocus {
                room_id,
                generation,
                focus,
            } => self.refocus_timeline(room_id, generation, focus).await,
            TimelineEvent::AudioLocated {
                room_id,
                generation,
                request,
                track,
            } => self.settle_audio_lookup(&room_id, generation, request, track),
            TimelineEvent::SourceLocated {
                room_id,
                generation,
                request,
                source,
            } => {
                if self.active_timeline.is_current(&room_id, generation) {
                    self.event_source.located(request, source);
                }
            }
            TimelineEvent::ReadersLocated {
                room_id,
                generation,
                request,
                readers,
            } => self.settle_readers_lookup(room_id, generation, request, readers),
        }
    }

    pub(super) async fn send_message(&mut self, room_id: RoomId, draft: MessageDraft) {
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        self.submissions
            .send(&mut self.send_lanes, timeline, room_id.clone(), draft);
        self.return_to_live_for_send(room_id).await;
    }

    pub(super) fn edit_message(&mut self, room_id: RoomId, edit: MessageEdit) {
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        self.submissions
            .edit(&mut self.send_lanes, timeline, room_id, edit);
    }

    pub(super) fn hold_unsaved_edit(&mut self, room_id: &RoomId, edit: MessageEdit) {
        if self.held_session.running().is_none() {
            tracing::debug!(%room_id, "dropping an unsaved edit from a session that ended");
            return;
        }
        self.submissions
            .unsaved(room_id, edit, self.selection.room.as_ref());
    }

    pub(super) fn resolve_failed_send(&mut self, local_id: String, action: FailedSend) {
        let Some((timeline, room_id)) = self.in_active_room(|a| &a.timeline) else {
            return;
        };
        self.active_timeline.spawn_resolve_failed_send(
            &mut self.operations,
            timeline,
            room_id,
            local_id,
            action,
        );
    }

    pub(super) async fn send_sticker(
        &mut self,
        room_id: RoomId,
        pack: PackId,
        shortcode: String,
        reply_to: Option<String>,
    ) {
        let Some(stickers) = self.port(|a| &a.image_packs) else {
            return;
        };
        self.packs.send(
            &mut self.send_lanes,
            stickers,
            room_id.clone(),
            pack,
            shortcode,
            reply_to,
        );
        self.return_to_live_for_send(room_id).await;
    }

    pub(super) async fn send_poll(&mut self, room_id: RoomId, draft: PollDraft) {
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        polls::send(
            &mut self.send_lanes,
            Arc::clone(&self.output),
            timeline,
            room_id.clone(),
            draft,
        );
        self.return_to_live_for_send(room_id).await;
    }

    pub(super) fn pick_attachment(&mut self, room_id: RoomId, pick: AttachmentPick) {
        self.attachments.pick(&mut self.operations, room_id, pick);
    }

    pub(super) async fn send_attachment(
        &mut self,
        room_id: RoomId,
        caption: ComposerText,
        as_document: bool,
        reply_to: Option<String>,
    ) {
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        let sent = self.attachments.send(
            &mut self.send_lanes,
            timeline,
            room_id.clone(),
            caption,
            as_document,
            reply_to,
        );
        if sent {
            self.return_to_live_for_send(room_id).await;
        }
    }

    fn in_active_room<P: ?Sized>(
        &self,
        pick: impl FnOnce(&AuthenticatedSession) -> &Arc<P>,
    ) -> Option<(Arc<P>, RoomId)> {
        let port = self.port(pick)?;
        let room_id = self.active_timeline.room_id()?.clone();
        Some((port, room_id))
    }

    pub(super) fn set_pack_global(&mut self, pack: PackId, global: bool) {
        if let Some(port) = self.port(|a| &a.image_packs) {
            self.packs.set_global(port, pack, global);
        }
    }

    pub(super) fn react_with_emote(&mut self, event_id: String, pack: &PackId, shortcode: String) {
        let Some(mxc) = self.packs.emote_mxc(pack, &shortcode) else {
            tracing::debug!(%event_id, %pack, "the reacted emote is no longer shown");
            return;
        };
        self.active_timeline
            .toggle_reaction(event_id, mxc, Some(shortcode));
    }

    pub(super) fn delete_message(&mut self, event_id: String) {
        let Some((port, room_id)) = self.in_active_room(|a| &a.timeline) else {
            return;
        };
        self.message_actions
            .delete(&mut self.send_lanes, port, room_id, event_id);
    }

    pub(super) fn change_pin(&mut self, event_id: String, change: PinChange) {
        let Some((port, room_id)) = self.in_active_room(|a| &a.pinned) else {
            return;
        };
        self.pinned.change(port, room_id, event_id, change);
    }

    pub(super) fn copy_message_link(&mut self, event_id: String) {
        let Some((port, room_id)) = self.in_active_room(|a| &a.room_info) else {
            return;
        };
        self.message_actions
            .copy_link(&mut self.operations, port, room_id, event_id);
    }

    pub(super) fn open_media(&mut self, event_id: String) {
        if let Some((media, room_id)) = self.in_active_room(|a| &a.media) {
            self.media.open_media(media, room_id, event_id);
        }
    }

    pub(super) fn open_video(&mut self, event_id: String) {
        let Some((media, room_id)) = self.in_active_room(|a| &a.media) else {
            return;
        };
        if cfg!(feature = "video") {
            self.video.open(media, room_id, event_id);
        } else {
            self.media.play_video_externally(media, room_id, event_id);
        }
    }

    pub(super) fn play_audio(&mut self, event_id: String) {
        if cfg!(feature = "video") {
            self.audio.play(&self.active_timeline, event_id);
        } else if let Some((media, room_id)) = self.in_active_room(|a| &a.media) {
            self.media.play_audio_externally(media, room_id, event_id);
        }
    }

    fn settle_audio_lookup(
        &mut self,
        room_id: &RoomId,
        generation: i32,
        request: u64,
        track: Option<Box<AudioTrack>>,
    ) {
        let Some(media) = self.port(|a| &a.media) else {
            return;
        };
        let track = track
            .filter(|_| self.active_timeline.is_current(room_id, generation))
            .map(|track| *track);
        self.audio.located(media, request, track);
    }

    pub(super) fn save_file(
        &mut self,
        event_id: String,
        filename: String,
        mimetype: Option<String>,
    ) {
        if let Some((media, room_id)) = self.in_active_room(|a| &a.media) {
            self.media
                .save_file(media, room_id, event_id, filename, mimetype);
        }
    }
}
