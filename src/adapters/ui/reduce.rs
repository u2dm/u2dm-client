use std::collections::HashSet;
use std::mem;
use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use super::audio;
use super::backend::{UiBackend, UiEventContext, apply_sticker_art, enrich_message};
use super::decode::{AvatarSlot, load_attachment_preview, load_avatar_async, request_sticker};
use super::dto::{
    GRID_COLUMNS, StickerArt, StickerPackDto, StickerRowDto, audio_row_update, sticker_art,
    sticker_grid, sticker_needle,
};
use super::fields::{MessageFields, RoomFields, SpaceFields};
use super::present::{
    VerifyStep, duration_label, file_extension, user_initial, verification_cancellation,
};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::{
    RowReplacements, apply_rooms, apply_spaces, apply_timeline_patch, index_sticker_grid,
    retain_awaited_downloads,
};
use super::rows::patch_rows_by_id;
use super::session::{begin_session, with_session};
use super::splice_model::SpliceModel;
use super::video;
use crate::commands::effects::{Effect, VerificationActivity, VerificationUpdate};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{
    AppViewState, AttachmentView, AudioView, DirectoryView, LifecycleView, NowPlaying,
    PaginationView, StickerView, Toast, TrackFile, UnsentMessage, VideoView,
};
use crate::domain::room::{RoomId, RoomList};
use crate::domain::timeline::{TimelinePatch, TimelineStatus};
use crate::domain::verification::VerificationEvent as DomainVerificationEvent;
use crate::ports::media::MediaCache;
use crate::util::format_bytes;

const NO_ANCHOR: i32 = -1;

#[derive(Default)]
pub struct RoomCursor {
    active_generation: i32,
    adopted_generation: i32,
    timeline_token: i32,
    prepend_token: i32,
    focus_event_id: Option<String>,
}

impl RoomCursor {
    pub fn active_generation(&self) -> i32 {
        self.active_generation
    }
}

fn active_generation() -> i32 {
    with_session(|session| session.room.active_generation())
}

fn adopt_selected_generation(generation: i32) -> bool {
    with_session(|session| {
        let previous = mem::replace(&mut session.room.active_generation, generation);
        previous != generation
    })
}

fn is_new_generation(generation: i32) -> bool {
    with_session(|session| session.room.adopted_generation != generation)
}

fn readopt_timeline(w: &impl UiProps, generation: i32) {
    let next = with_session(|session| {
        let room = &mut session.room;
        room.adopted_generation = generation;
        room.timeline_token = room.timeline_token.wrapping_add(1);
        room.timeline_token
    });
    w.set_int(IntProp::TimelineToken, next);
}

fn next_prepend_token() -> i32 {
    with_session(|session| {
        let room = &mut session.room;
        room.prepend_token = room.prepend_token.wrapping_add(1);
        room.prepend_token
    })
}

fn set_focus(w: &impl UiProps, event_id: Option<&str>) {
    with_session(|session| session.room.focus_event_id = event_id.map(ToOwned::to_owned));
    w.set_string(
        StringProp::FocusEventId,
        SharedString::from(event_id.unwrap_or_default()),
    );
}

fn publish_room_cursor(w: &impl UiProps) {
    let (generation, timeline_token, prepend_token, focus) = with_session(|session| {
        let room = &session.room;
        (
            room.active_generation,
            room.timeline_token,
            room.prepend_token,
            room.focus_event_id.clone(),
        )
    });
    w.set_int(IntProp::SelectedGeneration, generation);
    w.set_int(IntProp::TimelineToken, timeline_token);
    w.set_int(IntProp::PrependToken, prepend_token);
    w.set_string(
        StringProp::FocusEventId,
        SharedString::from(focus.unwrap_or_default()),
    );
}

pub(super) fn latest_rooms() -> Option<RoomList> {
    with_session(|session| {
        session
            .snapshot
            .as_ref()
            .map(|view| Arc::clone(&view.directory.rooms))
    })
}

pub(super) fn set_sticker_query<B: UiBackend>(query: &str, media: &dyn MediaCache) {
    let needle = sticker_needle(query);
    let stickers = with_session(|session| {
        if session.sticker_needle == needle {
            return None;
        }
        session.sticker_needle = needle;
        session.snapshot.as_ref().map(|view| view.stickers.clone())
    });
    if let Some(stickers) = stickers {
        rebuild_sticker_grid::<B>(&stickers, media);
    }
}

pub fn dispatch_effect<B: UiBackend>(w: &B::Window, event: Effect, ctx: &UiEventContext<'_, B>) {
    match event {
        Effect::Snapshot(view) => apply_snapshot::<B>(w, &view, ctx),
        Effect::SelectedRoom {
            id,
            name,
            member_count,
            generation,
        } => {
            if adopt_selected_generation(generation) {
                set_focus(w, None);
            }
            w.set_int(IntProp::SelectedGeneration, generation);
            w.set_string(StringProp::SelectedRoomId, SharedString::from(id.as_ref()));
            w.set_string(StringProp::SelectedRoomName, SharedString::from(&name));
            w.set_int(
                IntProp::SelectedRoomMembers,
                i32::try_from(member_count).unwrap_or(i32::MAX),
            );
            let pagination =
                with_session(|session| session.snapshot.as_ref().map(|view| view.pagination))
                    .unwrap_or_default();
            sync_timeline_chrome(w, &pagination);
        }
        Effect::Timeline {
            room_id,
            generation,
            patch,
        } => apply_timeline::<B>(w, &room_id, generation, patch, ctx),
        Effect::TimelineFocus {
            room_id,
            generation,
            event_id,
            row,
        } => {
            if is_active(w, &room_id, generation) {
                let anchor = i32::try_from(row).unwrap_or(NO_ANCHOR);
                tracing::debug!(anchor, generation, event_id, %room_id, "publishing the focus anchor");
                set_focus(w, Some(&event_id));
                w.set_int(IntProp::AnchorIndex, anchor);
                readopt_timeline(w, generation);
            }
        }
        Effect::TimelineStatus {
            room_id,
            generation,
            status,
        } => {
            if is_active(w, &room_id, generation) {
                apply_timeline_status(w, status);
            }
        }
        Effect::Verification(update) => apply_verification(w, &update),
        Effect::SessionReset(view) => {
            let models = begin_session::<B>(w);
            let fresh = UiEventContext::<B> {
                models: &models,
                media: ctx.media,
            };
            apply_snapshot::<B>(w, &Arc::new(*view), &fresh);
            clear_selected_room(w);
            reset_verification(w);
            w.clear_text_inputs();
        }
    }
}

fn apply_timeline<B: UiBackend>(
    w: &B::Window,
    room_id: &RoomId,
    generation: i32,
    patch: Box<TimelinePatch>,
    ctx: &UiEventContext<'_, B>,
) {
    let selected = w.get_string(StringProp::SelectedRoomId);
    let matches = is_active(w, room_id, generation);
    tracing::debug!(
        patch = patch.label(),
        %room_id,
        generation,
        %selected,
        matches,
        "dispatch_effect received Timeline event"
    );
    if !matches {
        return;
    }

    let opens_room = patch.opens_room();
    let opens_new_timeline = opens_room && is_new_generation(generation);
    let replaces_loaded_window = opens_room && !opens_new_timeline;

    if opens_room {
        apply_timeline_status(w, TimelineStatus::Ready);
    }
    if opens_new_timeline {
        let anchor = unread_anchor_row(&patch);
        tracing::debug!(
            anchor,
            generation,
            label = patch.label(),
            %room_id,
            "publishing the unread anchor"
        );
        set_focus(w, None);
        w.set_int(IntProp::AnchorIndex, anchor);
        readopt_timeline(w, generation);
    }
    if patch.is_prepend() {
        w.set_int(IntProp::PrependToken, next_prepend_token());
    }

    let anchor_row_moved = patch.shifts_rows() || replaces_loaded_window;
    apply_timeline_patch(
        &ctx.models.timeline,
        *patch,
        &|m| B::convert_message(m, ctx.media),
        &|entry, delta| enrich_message::<B>(entry, delta, ctx.media),
        &|entry| entry.unique_id(),
    );
    if anchor_row_moved {
        w.set_int(IntProp::AnchorIndex, anchor_row::<B>(&ctx.models.timeline));
    }
}

fn apply_snapshot<B: UiBackend>(
    w: &B::Window,
    view: &Arc<AppViewState>,
    ctx: &UiEventContext<'_, B>,
) {
    let previous = with_session(|session| session.snapshot.clone());
    let last = previous.as_deref();
    let AppViewState {
        lifecycle,
        connection,
        directory,
        pagination,
        stickers,
        attachment,
        video,
        audio,
        unsent,
        toast,
    } = view.as_ref();
    let DirectoryView {
        rooms,
        spaces,
        subspaces,
        scope,
        space_id,
        subspace_id,
        direct_flags,
    } = directory;

    apply_lifecycle(w, last.map(|l| &l.lifecycle), lifecycle);
    if last.is_none_or(|l| l.connection != *connection) {
        w.set_connection_state(connection);
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.directory.rooms, rooms)) {
        apply_rooms(
            &ctx.models.rooms,
            rooms.as_ref(),
            last.map_or(&[], |l| l.directory.rooms.as_ref()),
            ctx.media,
            &|room| B::convert_room(room, ctx.media),
            &|entry| entry.id(),
        );
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.directory.spaces, spaces)) {
        apply_spaces(
            &ctx.models.spaces,
            spaces.as_ref(),
            ctx.media,
            &|space| B::convert_space(space, ctx.media),
            &|entry| entry.id(),
        );
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.directory.subspaces, subspaces)) {
        apply_spaces(
            &ctx.models.subspaces,
            subspaces.as_ref(),
            ctx.media,
            &|space| B::convert_space(space, ctx.media),
            &|entry| entry.id(),
        );
    }
    if last.is_none_or(|l| l.directory.scope != *scope) {
        w.set_room_scope(*scope);
    }
    if last.is_none_or(|l| l.directory.space_id != *space_id) {
        w.set_string(StringProp::SelectedSpaceId, SharedString::from(space_id));
    }
    if last.is_none_or(|l| l.directory.subspace_id != *subspace_id) {
        w.set_string(
            StringProp::SelectedSubspaceId,
            SharedString::from(subspace_id),
        );
    }
    if last.is_none_or(|l| l.directory.direct_flags != *direct_flags) {
        w.set_bool(BoolProp::DirectAlert, direct_flags.alert);
        w.set_bool(BoolProp::DirectMention, direct_flags.mention);
        w.set_bool(BoolProp::DirectHint, direct_flags.hint);
    }
    if last.is_none_or(|l| l.pagination != *pagination) {
        sync_timeline_chrome(w, pagination);
    }
    if last.is_none_or(|l| {
        !Arc::ptr_eq(&l.stickers.packs, &stickers.packs)
            || l.stickers.generation != stickers.generation
            || l.stickers.ready_images != stickers.ready_images
            || l.stickers.room_encrypted != stickers.room_encrypted
            || l.stickers.loading != stickers.loading
    }) {
        apply_stickers::<B>(w, last.map(|l| &l.stickers), stickers, ctx.media);
    }
    if last.is_none_or(|l| l.attachment != *attachment) {
        apply_attachment(w, attachment);
    }
    if last.is_none_or(|l| l.video != *video) {
        apply_video::<B>(w, video);
    }
    if last.is_none_or(|l| l.audio != *audio) {
        apply_audio::<B>(w, last.map(|l| &l.audio), audio, audio_start(video), ctx);
    }
    if last.is_none_or(|l| l.unsent != *unsent) {
        apply_unsent(w, unsent.as_ref());
    }
    if last.is_none_or(|l| l.toast != *toast) {
        apply_toast(w, toast);
    }
    with_session(|session| session.snapshot = Some(Arc::clone(view)));
}

fn apply_stickers<B: UiBackend>(
    w: &B::Window,
    last: Option<&StickerView>,
    stickers: &StickerView,
    media: &dyn MediaCache,
) {
    let catalog_changed = last.is_none_or(|l| {
        !Arc::ptr_eq(&l.packs, &stickers.packs) || l.generation != stickers.generation
    });
    if catalog_changed {
        rebuild_sticker_grid::<B>(stickers, media);
    } else if last.is_some_and(|l| l.ready_images != stickers.ready_images) {
        settle_sticker_downloads::<B>(media);
    }
    let for_this_room = stickers.generation == active_generation();
    w.set_int(IntProp::StickerColumns, GRID_COLUMNS);
    w.set_bool(BoolProp::StickerRoomEncrypted, stickers.room_encrypted);
    w.set_bool(BoolProp::StickerLoading, stickers.loading);
    w.set_bool(
        BoolProp::StickerHasPacks,
        for_this_room && !stickers.packs.is_empty(),
    );
}

fn rebuild_sticker_grid<B: UiBackend>(stickers: &StickerView, media: &dyn MediaCache) {
    let (active, needle) = with_session(|session| {
        (
            session.room.active_generation,
            session.sticker_needle.clone(),
        )
    });
    let grid = if stickers.generation == active {
        sticker_grid(stickers.packs.as_ref(), &needle, media)
    } else {
        sticker_grid(&[], &needle, media)
    };

    let replacements = index_sticker_grid(&grid);
    let missing_icons = packs_missing_icons(&grid.packs);
    B::with_stickers(|rows, packs| {
        replace_reshaped_rows::<B>(rows, grid.rows, &replacements);
        packs.set_vec(
            grid.packs
                .into_iter()
                .map(B::StickerPack::from)
                .collect::<Vec<_>>(),
        );
    });
    for icon_cell_key in &missing_icons {
        request_sticker(icon_cell_key);
    }
    settle_sticker_downloads::<B>(media);
}

fn replace_reshaped_rows<B: UiBackend>(
    rows: &SpliceModel<B::StickerRow>,
    grid_rows: Vec<StickerRowDto>,
    replacements: &RowReplacements,
) {
    let kept = rows.row_count();
    let total = grid_rows.len();
    let mut appended = Vec::new();
    for (row, entry) in grid_rows.into_iter().enumerate() {
        if row >= kept {
            appended.push(B::StickerRow::from(entry));
        } else if replacements.must_replace(row) {
            rows.set_row_data(row, B::StickerRow::from(entry));
        }
    }
    rows.truncate(total);
    rows.insert_rows(kept, appended);
}

fn settle_sticker_downloads<B: UiBackend>(media: &dyn MediaCache) {
    let mut settled = Vec::new();
    retain_awaited_downloads(|key, mxc| match sticker_art(key, mxc, media) {
        StickerArt::Downloading => true,
        StickerArt::Decoding => false,
        art => {
            settled.push((key.to_owned(), art));
            false
        }
    });
    for (key, art) in settled {
        match art {
            StickerArt::Ready(image) => apply_sticker_art::<B>(&key, Some(&image)),
            StickerArt::Failed => apply_sticker_art::<B>(&key, None),
            StickerArt::Decoding | StickerArt::Downloading => {}
        }
    }
}

fn packs_missing_icons(packs: &[StickerPackDto]) -> Vec<SharedString> {
    packs
        .iter()
        .filter(|pack| pack.icon.is_none() && !pack.icon_cell_key.is_empty())
        .map(|pack| pack.icon_cell_key.clone())
        .collect()
}

fn sync_timeline_chrome(w: &impl UiProps, pagination: &PaginationView) {
    let (backwards, forwards, badge) = if pagination.generation == active_generation() {
        (
            pagination.backwards_loading,
            pagination.forwards_loading,
            pagination.new_messages,
        )
    } else {
        (false, false, 0)
    };
    w.set_bool(BoolProp::BackwardsLoading, backwards);
    w.set_bool(BoolProp::ForwardsLoading, forwards);
    w.set_int(
        IntProp::NewMessagesCount,
        i32::try_from(badge).unwrap_or(i32::MAX),
    );
}

fn apply_lifecycle(w: &impl UiProps, last: Option<&LifecycleView>, next: &LifecycleView) {
    let LifecycleView {
        step,
        activity,
        messages,
        method,
        resolved_homeserver,
        user_id,
        avatar_path,
    } = next;

    if last.is_none_or(|l| l.step != *step) {
        w.set_login_phase(*step);
    }
    if last.is_none_or(|l| l.activity != *activity) {
        w.set_login_activity(*activity);
    }
    if last.is_none_or(|l| l.messages != *messages) {
        w.apply_login_messages(messages);
    }
    if last.is_none_or(|l| l.method != *method) {
        w.set_login_method_kind(*method);
    }
    if last.is_none_or(|l| l.resolved_homeserver != *resolved_homeserver) {
        w.set_string(
            StringProp::ResolvedHomeserver,
            SharedString::from(resolved_homeserver),
        );
    }
    if last.is_none_or(|l| l.user_id != *user_id) {
        w.set_string(StringProp::UserId, SharedString::from(user_id));
        w.set_string(
            StringProp::UserInitial,
            SharedString::from(user_initial(user_id)),
        );
    }
    if last.is_none_or(|l| l.avatar_path != *avatar_path) {
        let avatar = load_avatar_async(avatar_path.as_deref(), AvatarSlot::User);
        w.apply_user_avatar(avatar);
    }
}

fn apply_attachment(w: &impl UiProps, attachment: &AttachmentView) {
    let AttachmentView {
        pick,
        visible,
        filename,
        mimetype,
        size,
        width,
        height,
        kind,
        duration,
        preview_path,
        sending,
        error,
        error_detail,
    } = attachment;

    w.set_bool(BoolProp::AttachmentVisible, *visible);
    w.set_attachment_kind(*kind);
    w.set_bool(BoolProp::AttachmentSending, *sending);
    w.set_string(StringProp::AttachmentFilename, SharedString::from(filename));
    w.set_string(StringProp::AttachmentMimetype, SharedString::from(mimetype));
    w.set_string(
        StringProp::AttachmentExtension,
        SharedString::from(file_extension(filename).to_uppercase()),
    );
    w.set_string(
        StringProp::AttachmentSize,
        SharedString::from(if *visible {
            format_bytes(*size)
        } else {
            String::new()
        }),
    );
    w.set_string(
        StringProp::AttachmentDuration,
        SharedString::from(duration.map(duration_label).unwrap_or_default()),
    );
    w.set_int(IntProp::AttachmentWidth, (*width).cast_signed());
    w.set_int(IntProp::AttachmentHeight, (*height).cast_signed());
    w.set_attachment_error(*error);
    w.set_string(
        StringProp::AttachmentErrorDetail,
        SharedString::from(error_detail),
    );
    let preview = load_attachment_preview(*pick, preview_path.as_deref());
    w.apply_attachment_preview(preview);
}

fn apply_video<B: UiBackend>(w: &B::Window, view: &VideoView) {
    let VideoView {
        visible,
        loading,
        path,
        error,
    } = view;

    w.set_bool(BoolProp::VideoVisible, *visible);
    w.set_bool(BoolProp::VideoLoading, *loading);
    w.set_video_error(*error);

    match path.as_deref().filter(|_| *visible) {
        Some(path) => {
            audio::pause(w);
            video::open(w, &w.as_weak(), path);
        }
        None => video::close(w),
    }
}

fn audio_start(video: &VideoView) -> audio::Start {
    if video.visible {
        audio::Start::Paused
    } else {
        audio::Start::Playing
    }
}

fn apply_audio<B: UiBackend>(
    w: &B::Window,
    last: Option<&AudioView>,
    view: &AudioView,
    start: audio::Start,
    ctx: &UiEventContext<'_, B>,
) {
    if let Some(now) = &view.now_playing {
        show_now_playing(w, now, start);
    } else {
        w.set_bool(BoolProp::AudioVisible, false);
        w.set_bool(BoolProp::AudioLoading, false);
        w.set_string(StringProp::AudioEventId, SharedString::new());
        audio::close(w);
    }
    let previous = last.and_then(|l| l.now_playing.as_ref());
    for now in previous.into_iter().chain(view.now_playing.as_ref()) {
        refresh_audio_row::<B>(now, ctx);
    }
}

fn show_now_playing<W>(w: &W, now: &NowPlaying, start: audio::Start)
where
    W: ComponentHandle + UiProps + 'static,
{
    audio::prepare(w, now.request, now.meta.duration);
    w.set_string(StringProp::AudioEventId, SharedString::from(&now.event_id));
    w.set_string(
        StringProp::AudioRoomId,
        SharedString::from(now.room_id.as_ref()),
    );
    w.set_string(StringProp::AudioSender, SharedString::from(&now.sender));
    w.set_string(
        StringProp::AudioTitle,
        SharedString::from(&now.meta.filename),
    );
    w.set_audio_kind(now.meta.kind);
    w.set_bool(BoolProp::AudioLoading, now.file == TrackFile::Downloading);
    w.set_bool(BoolProp::AudioVisible, true);
    if let TrackFile::Ready(path) = &now.file {
        audio::open(w, &w.as_weak(), now.request, path, start);
    }
}

fn refresh_audio_row<B: UiBackend>(now: &NowPlaying, ctx: &UiEventContext<'_, B>) {
    let update = audio_row_update(&now.meta, ctx.media);
    let ids = HashSet::from([now.event_id.as_str()]);
    patch_rows_by_id(
        &*ctx.models.timeline,
        &ids,
        &B::Message::event_id,
        |entry| {
            entry.set_media_state(update.media_state);
            entry.set_media_failure(update.media_failure);
            entry.set_waveform(update.waveform.clone());
        },
    );
}

fn apply_unsent(w: &impl UiProps, unsent: Option<&UnsentMessage>) {
    let draft = unsent.map(|unsent| &unsent.draft);
    let reply = draft.and_then(|draft| draft.reply.as_ref());
    w.set_bool(BoolProp::UnsentVisible, unsent.is_some());
    w.set_int(
        IntProp::UnsentSubmission,
        unsent.map_or(0, |unsent| unsent.submission),
    );
    w.set_string(
        StringProp::UnsentRoomId,
        SharedString::from(unsent.map_or("", |unsent| unsent.room_id.as_ref())),
    );
    w.set_string(
        StringProp::UnsentBody,
        SharedString::from(draft.map_or("", |draft| draft.body.as_str())),
    );
    w.set_string(
        StringProp::UnsentReplyEventId,
        SharedString::from(reply.map_or("", |reply| reply.event_id.as_str())),
    );
    w.set_string(
        StringProp::UnsentReplySender,
        SharedString::from(reply.map_or("", |reply| reply.sender.as_str())),
    );
    w.set_string(
        StringProp::UnsentReplyPreview,
        SharedString::from(reply.map_or("", |reply| reply.preview.as_str())),
    );
}

fn apply_toast(w: &impl UiProps, toast: &Toast) {
    let (kind, detail) = match toast {
        Toast::None => (UserMessageKind::None, ""),
        Toast::Error(message) => (message.kind, message.detail.as_str()),
        Toast::FileSaved(path) => (UserMessageKind::FileSaved, path.as_str()),
    };
    w.set_toast_message(kind);
    w.set_string(StringProp::ToastDetail, SharedString::from(detail));
}

fn anchor_row<B: UiBackend>(model: &SpliceModel<B::Message>) -> i32 {
    let focus = with_session(|session| session.room.focus_event_id.clone());
    let is_anchor = |entry: &B::Message| match &focus {
        Some(event_id) => entry.event_id() == event_id,
        None => entry.first_unread(),
    };
    (0..model.row_count())
        .find(|row| model.row_data(*row).is_some_and(|entry| is_anchor(&entry)))
        .and_then(|row| i32::try_from(row).ok())
        .unwrap_or(NO_ANCHOR)
}

fn unread_anchor_row(patch: &TimelinePatch) -> i32 {
    patch
        .unread_anchor()
        .and_then(|anchor| i32::try_from(anchor.row).ok())
        .unwrap_or(NO_ANCHOR)
}

fn is_active(w: &impl UiProps, room_id: &RoomId, generation: i32) -> bool {
    w.get_string(StringProp::SelectedRoomId).as_str() == room_id.as_ref()
        && active_generation() == generation
}

fn apply_timeline_status(w: &impl UiProps, status: TimelineStatus) {
    w.set_bool(
        BoolProp::TimelineRetryable,
        matches!(status, TimelineStatus::Failed { retryable: true }),
    );
    w.set_timeline_state(status);
}

fn apply_verification(w: &impl UiProps, update: &VerificationUpdate) {
    match update {
        VerificationUpdate::Flow(event) => apply_verification_flow(w, event),
        VerificationUpdate::Busy(activity) => w.set_verification_activity(*activity),
        VerificationUpdate::Failed(message) => {
            w.set_verification_activity(VerificationActivity::None);
            set_verification_error(w, message);
        }
        VerificationUpdate::Dismissed => reset_verification(w),
    }
}

fn apply_verification_flow(w: &impl UiProps, event: &DomainVerificationEvent) {
    w.set_verification_activity(VerificationActivity::None);
    match event {
        DomainVerificationEvent::Requested { sender, is_self } => {
            w.set_bool(BoolProp::VerificationVisible, true);
            w.set_verification_phase(VerifyStep::Requested);
            w.set_string(
                StringProp::VerificationSender,
                SharedString::from(sender.as_str()),
            );
            w.set_bool(BoolProp::VerificationIsSelf, *is_self);
            set_verification_error(w, &UserMessage::default());
        }
        DomainVerificationEvent::Emojis(emojis) => {
            w.set_verification_phase(VerifyStep::Emojis);
            w.apply_emoji_model(emojis);
        }
        DomainVerificationEvent::Confirming => {
            w.set_verification_phase(VerifyStep::Confirming);
        }
        DomainVerificationEvent::Done => {
            w.set_verification_phase(VerifyStep::Done);
        }
        DomainVerificationEvent::Cancelled(reason) => {
            w.set_verification_phase(VerifyStep::Cancelled);
            set_verification_error(w, &verification_cancellation(reason));
        }
    }
}

fn reset_verification(w: &impl UiProps) {
    w.set_bool(BoolProp::VerificationVisible, false);
    w.set_verification_activity(VerificationActivity::None);
    w.set_verification_phase(VerifyStep::None);
    w.set_string(StringProp::VerificationSender, SharedString::default());
    w.set_bool(BoolProp::VerificationIsSelf, false);
    set_verification_error(w, &UserMessage::default());
    w.clear_emoji_model();
}

fn set_verification_error(w: &impl UiProps, message: &UserMessage) {
    w.set_verification_error(message.kind);
    w.set_string(
        StringProp::VerificationErrorDetail,
        SharedString::from(&message.detail),
    );
}

fn clear_selected_room(w: &impl UiProps) {
    w.set_string(StringProp::SelectedRoomId, SharedString::default());
    w.set_string(StringProp::SelectedRoomName, SharedString::default());
    w.set_int(IntProp::SelectedRoomMembers, 0);
    w.set_int(IntProp::AnchorIndex, NO_ANCHOR);
    publish_room_cursor(w);
    apply_timeline_status(w, TimelineStatus::None);
}
