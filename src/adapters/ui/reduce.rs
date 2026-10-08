use std::collections::{BTreeSet, HashSet};
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString, StyledText};

use super::audio;
use super::backend::{
    UiBackend, UiEventContext, apply_sticker_art, enrich_message, keep_shown_enrichment,
};
use super::decode::{AvatarSlot, load_attachment_preview, load_avatar_async, request_sticker};
use super::dto::{
    GRID_COLUMNS, QuotedMessage, StickerArt, StickerPackDto, StickerRowDto, audio_row_update,
    load_room_info_avatar, load_user_info_avatar, preview_line, quoted_message, rich_body,
    sticker_art, sticker_grid, sticker_needle, user_info_pronouns,
};
use super::fields::{MemberRowFields, MessageFields, SpaceChildFields, SpaceFields};
use super::present::{
    VerifyStep, avatar_color_index, avatar_initials, duration_label, file_extension,
    message_sent_at_label, user_initial, verification_cancellation,
};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::{
    RoomListing, RowReplacements, apply_member_rows, apply_mention_rows, apply_reader_rows,
    apply_rooms, apply_space_children, apply_spaces, apply_timeline_patch, index_sticker_grid,
    retain_awaited_downloads,
};
use super::richtext::recognise_own_user;
use super::rows::patch_rows_by_id;
use super::session::{begin_session, with_session};
use super::splice_model::SpliceModel;
use super::video;
use crate::commands::effects::{Effect, VerificationActivity, VerificationUpdate};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::Draft;
use crate::commands::view::{
    AppViewState, AttachmentView, AudioView, CardStatus, CopiedLink, DirectoryView, LifecycleView,
    MentionsView, NowPlaying, PaginationView, PinnedView, ReadersView, RoomCard, RoomInfoView,
    RoomLogView, RoomMenuTarget, SourceState, SpaceIndexView, StickerView, Toast, TrackFile,
    UnsentMessage, UserCard, UserInfoView, VideoView,
};
use crate::domain::message::{
    EditKind, MessageEdit, MessagePermissions, MessagePreviewKind, RichText, TimelineMessage,
};
use crate::domain::poll::PollPermissions;
use crate::domain::room::RoomId;
use crate::domain::room_info::RoomAbout;
use crate::domain::room_log::RoomLog;
use crate::domain::timeline::{OlderHistory, SourceEncryption, TimelinePatch, TimelineStatus};
use crate::domain::user_info::{IdentityTrust, Moderation};
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
    avatar: SelectedRoomAvatar,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct SelectedRoomAvatar {
    room_id: String,
    mxc: Option<String>,
    path: Option<PathBuf>,
}

impl SelectedRoomAvatar {
    fn same_source(&self, other: &Self) -> bool {
        self.room_id == other.room_id && self.mxc == other.mxc
    }
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

pub(super) fn is_latest_adoption(generation: i32, seen_timeline_token: i32) -> bool {
    with_session(|session| {
        let room = &session.room;
        room.adopted_generation == generation && room.timeline_token == seen_timeline_token
    })
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

pub(super) fn latest_directory() -> Option<DirectoryView> {
    with_session(|session| session.snapshot.as_ref().map(|view| view.directory.clone()))
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
            avatar_mxc,
            member_count,
            encrypted,
            polls,
            messages,
            generation,
            live,
        } => {
            if adopt_selected_generation(generation) {
                set_focus(w, None);
            }
            w.set_bool(BoolProp::TimelineDetached, !live);
            w.set_int(IntProp::SelectedGeneration, generation);
            w.set_string(StringProp::SelectedRoomId, SharedString::from(id.as_ref()));
            w.set_string(StringProp::SelectedRoomName, SharedString::from(&name));
            w.set_string(
                StringProp::SelectedRoomInitial,
                SharedString::from(avatar_initials(&name)),
            );
            w.set_int(
                IntProp::SelectedRoomColorIndex,
                avatar_color_index(id.as_ref()),
            );
            show_selected_room_avatar(w, id.as_ref(), avatar_mxc.as_deref(), ctx.media);
            w.set_int(
                IntProp::SelectedRoomMembers,
                i32::try_from(member_count).unwrap_or(i32::MAX),
            );
            w.set_bool(BoolProp::SelectedRoomEncrypted, encrypted);
            apply_poll_permissions(w, polls);
            apply_message_permissions(w, messages);
            let (pagination, pinned) = with_session(|session| {
                session
                    .snapshot
                    .as_ref()
                    .map(|view| (view.pagination, view.pinned.clone()))
            })
            .unwrap_or_default();
            sync_timeline_chrome(w, &pagination);
            apply_pinned(w, &pinned);
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
    let pinned_ids = with_session(|session| {
        session
            .snapshot
            .as_ref()
            .map(|view| Arc::clone(&view.pinned.pinned_ids))
    })
    .unwrap_or_default();
    apply_timeline_patch(
        &ctx.models.timeline,
        *patch,
        &|m| B::convert_message(m, is_pinned(&pinned_ids, m), ctx.media),
        &keep_shown_enrichment::<B>,
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
        space_index,
        room_info,
        room_menu,
        user_info,
        pagination,
        pinned,
        stickers,
        attachment,
        video,
        audio,
        unsent,
        toast,
        message_link,
        room_link,
        source,
        readers,
        mentions,
        room_log,
    } = view.as_ref();

    apply_lifecycle(w, last.map(|l| &l.lifecycle), lifecycle);
    if last.is_none_or(|l| l.connection != *connection) {
        w.set_connection_state(connection);
    }
    apply_directory::<B>(w, last.map(|l| &l.directory), directory, ctx);
    apply_space_index::<B>(w, last.map(|l| &l.space_index), space_index, ctx);
    apply_room_info::<B>(w, last.map(|l| &l.room_info), room_info, ctx);
    if last.is_none_or(|l| l.room_menu != *room_menu) {
        apply_room_menu(w, room_menu.as_ref());
    }
    apply_user_info(w, last.map(|l| &l.user_info), user_info, ctx.media);
    if last.is_none_or(|l| l.pagination != *pagination) {
        sync_timeline_chrome(w, pagination);
    }
    if last.is_none_or(|l| l.pinned != *pinned) {
        apply_pinned(w, pinned);
    }
    let last_pinned_ids = last.map(|l| Arc::clone(&l.pinned.pinned_ids));
    if last_pinned_ids.as_ref() != Some(&pinned.pinned_ids) {
        apply_pin_marks::<B>(last_pinned_ids.as_deref(), &pinned.pinned_ids, ctx);
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
    if last.is_none_or(|l| l.message_link != *message_link) {
        apply_message_link(w, message_link);
    }
    if last.is_none_or(|l| l.room_link != *room_link) {
        apply_room_link(w, room_link);
    }
    if last.is_none_or(|l| l.source != *source) {
        apply_source(w, source);
    }
    apply_readers::<B>(w, last.map(|l| &l.readers), readers, ctx);
    apply_mentions::<B>(w, last.map(|l| &l.mentions), mentions, ctx);
    if last.is_none_or(|l| l.room_log != *room_log) {
        apply_room_log::<B>(
            w,
            last.and_then(|l| l.room_log.as_ref()),
            room_log.as_ref(),
            ctx,
        );
    }
    with_session(|session| session.snapshot = Some(Arc::clone(view)));
}

fn apply_room_log<B: UiBackend>(
    w: &B::Window,
    last: Option<&RoomLogView>,
    next: Option<&RoomLogView>,
    ctx: &UiEventContext<'_, B>,
) {
    let lines = &ctx.models.room_log;
    let Some(next) = next else {
        w.set_bool(BoolProp::RoomLogVisible, false);
        lines.set_vec(Vec::new());
        return;
    };
    if let Some(last) = last.filter(|last| last.room_id == next.room_id) {
        follow_room_log::<B>(lines, &last.log, &next.log);
    } else {
        w.set_string(
            StringProp::RoomLogRoomId,
            SharedString::from(next.room_id.as_ref()),
        );
        lines.set_vec(
            next.log
                .lines
                .iter()
                .map(|line| B::convert_log_line(line))
                .collect(),
        );
    }
    w.set_string(StringProp::RoomLogName, SharedString::from(&next.name));
    w.set_int(
        IntProp::RoomLogDropped,
        i32::try_from(next.log.dropped).unwrap_or(i32::MAX),
    );
    w.set_int(IntProp::RoomLogLinesLanded, next.lines_landed);
    w.set_bool(BoolProp::RoomLogVisible, true);
    run_change_handlers_next_frame(w);
}

fn follow_room_log<B: UiBackend>(lines: &SpliceModel<B::LogLine>, shown: &RoomLog, next: &RoomLog) {
    let oldest_kept = next.oldest().unwrap_or(u64::MAX);
    lines.remove_front(shown.lines.partition_point(|line| line.seq < oldest_kept));
    let newest_shown = shown.newest();
    let arrived_from = next
        .lines
        .partition_point(|line| newest_shown.is_some_and(|newest| line.seq <= newest));
    let arrived: Vec<B::LogLine> = next
        .lines
        .iter()
        .skip(arrived_from)
        .map(|line| B::convert_log_line(line))
        .collect();
    lines.insert_rows(lines.row_count(), arrived);
}

fn is_pinned(pinned_ids: &BTreeSet<String>, message: &TimelineMessage) -> bool {
    message
        .event_id
        .as_ref()
        .is_some_and(|event_id| pinned_ids.contains(event_id))
}

fn apply_pin_marks<B: UiBackend>(
    last: Option<&BTreeSet<String>>,
    next: &BTreeSet<String>,
    ctx: &UiEventContext<'_, B>,
) {
    let changed: HashSet<&str> = match last {
        Some(last) => last
            .symmetric_difference(next)
            .map(String::as_str)
            .collect(),
        None => next.iter().map(String::as_str).collect(),
    };
    if changed.is_empty() {
        return;
    }
    patch_rows_by_id(
        &*ctx.models.timeline,
        &changed,
        &B::Message::event_id,
        |entry| {
            let pinned = next.contains(entry.event_id());
            entry.set_pinned(pinned);
        },
    );
}

fn apply_source(w: &impl UiProps, state: &SourceState) {
    let (event_id, source) = match state {
        SourceState::Closed => ("", None),
        SourceState::Locating { event_id } | SourceState::Unavailable { event_id } => {
            (event_id.as_str(), None)
        }
        SourceState::Ready(source) => (source.event_id.as_str(), Some(source.as_ref())),
    };
    let encryption = source.map_or(&SourceEncryption::Plain, |source| &source.encryption);
    let encryption_json = match encryption {
        SourceEncryption::Decrypted { details } => details.as_str(),
        SourceEncryption::Plain | SourceEncryption::Undecryptable => "",
    };
    w.set_string(StringProp::SourceEventId, SharedString::from(event_id));
    w.set_string(
        StringProp::SourceJson,
        SharedString::from(source.map_or("", |source| source.json.as_str())),
    );
    w.set_string(
        StringProp::SourceEditJson,
        SharedString::from(
            source
                .and_then(|source| source.edit_json.as_deref())
                .unwrap_or(""),
        ),
    );
    w.set_string(
        StringProp::SourceEncryptionJson,
        SharedString::from(encryption_json),
    );
    w.set_source_encryption(encryption);
    w.set_source_status(state);
}

fn apply_readers<B: UiBackend>(
    w: &B::Window,
    last: Option<&ReadersView>,
    readers: &ReadersView,
    ctx: &UiEventContext<'_, B>,
) {
    let ReadersView {
        status,
        message,
        total,
        rows,
        has_more,
        pages_landed,
        avatars_ready,
    } = readers;

    if last.is_none_or(|l| l.message != *message) {
        apply_quoted_message(w, quoted_message(message.as_deref()));
    }
    if last.is_none_or(|l| l.total != *total) {
        w.set_int(
            IntProp::ReadersCount,
            i32::try_from(*total).unwrap_or(i32::MAX),
        );
    }
    if last.is_none_or(|l| l.has_more != *has_more) {
        w.set_bool(BoolProp::ReadersHasMore, *has_more);
    }
    if last.is_none_or(|l| l.pages_landed != *pages_landed) {
        w.set_int(IntProp::ReadersPagesLanded, *pages_landed);
        run_change_handlers_next_frame(w);
    }
    let rows_changed = last.is_none_or(|l| !Arc::ptr_eq(&l.rows, rows));
    let avatars_landed = last.is_some_and(|l| l.avatars_ready != *avatars_ready);
    if rows_changed || avatars_landed {
        let previous = last
            .filter(|_| !avatars_landed)
            .map_or(&[] as &[_], |l| l.rows.as_ref());
        apply_reader_rows(
            &ctx.models.readers,
            rows.as_ref(),
            previous,
            ctx.media,
            &|reader| B::convert_reader(reader, ctx.media),
            &|entry| entry.user_id(),
        );
    }
    if last.is_none_or(|l| l.status != *status) {
        w.set_readers_status(*status);
    }
}

fn apply_mentions<B: UiBackend>(
    w: &B::Window,
    last: Option<&MentionsView>,
    mentions: &MentionsView,
    ctx: &UiEventContext<'_, B>,
) {
    let MentionsView {
        room_id,
        offers_room,
        rows,
        avatars_ready,
    } = mentions;

    if last.is_none_or(|l| l.room_id != *room_id) {
        w.set_string(
            StringProp::MentionsRoomId,
            room_id
                .as_ref()
                .map(|room| SharedString::from(room.as_ref()))
                .unwrap_or_default(),
        );
    }
    if last.is_none_or(|l| l.offers_room != *offers_room) {
        w.set_bool(BoolProp::MentionsOfferRoom, *offers_room);
    }
    let rows_changed = last.is_none_or(|l| !Arc::ptr_eq(&l.rows, rows));
    let avatars_landed = last.is_some_and(|l| l.avatars_ready != *avatars_ready);
    if rows_changed || avatars_landed {
        let previous = last
            .filter(|_| !avatars_landed)
            .map_or(&[] as &[_], |l| l.rows.as_ref());
        apply_mention_rows(
            &ctx.models.mentions,
            rows.as_ref(),
            previous,
            ctx.media,
            &|member| B::convert_mention(member, ctx.media),
            &|entry| entry.user_id(),
        );
    }
}

fn apply_quoted_message(w: &impl UiProps, quoted: QuotedMessage) {
    let QuotedMessage {
        sender,
        sent_at,
        kind,
        body,
        service_kind,
        service_target,
    } = quoted;
    w.set_string(StringProp::ReadersSender, sender);
    w.set_string(StringProp::ReadersSentAt, sent_at);
    w.set_readers_kind(kind);
    w.set_string(StringProp::ReadersBody, body);
    w.set_readers_service_kind(service_kind);
    w.set_string(StringProp::ReadersServiceTarget, service_target);
}

fn apply_message_link(w: &(impl UiProps + ComponentHandle), link: &CopiedLink) {
    w.set_string(
        StringProp::MessageLink,
        SharedString::from(link.url.as_str()),
    );
    w.set_int(IntProp::MessageLinkSerial, link.serial);
    run_change_handlers_next_frame(w);
}

fn apply_room_link(w: &(impl UiProps + ComponentHandle), link: &CopiedLink) {
    w.set_string(StringProp::RoomLink, SharedString::from(link.url.as_str()));
    w.set_int(IntProp::RoomLinkSerial, link.serial);
    run_change_handlers_next_frame(w);
}

fn apply_room_menu(w: &(impl UiProps + ComponentHandle), menu: Option<&RoomMenuTarget>) {
    let Some(menu) = menu else {
        w.set_string(StringProp::RoomMenuRoomId, SharedString::default());
        run_change_handlers_next_frame(w);
        return;
    };
    w.set_string(StringProp::RoomMenuName, SharedString::from(&menu.name));
    w.set_bool(BoolProp::RoomMenuUnread, menu.unread);
    w.set_room_menu_notify(menu.notify);
    w.set_bool(BoolProp::RoomMenuNotifyBusy, menu.notify_busy);
    w.set_bool(BoolProp::RoomMenuLeaving, menu.leaving);
    w.set_string(
        StringProp::RoomMenuRoomId,
        SharedString::from(menu.room_id.as_ref()),
    );
    run_change_handlers_next_frame(w);
}

fn apply_directory<B: UiBackend>(
    w: &B::Window,
    last: Option<&DirectoryView>,
    directory: &DirectoryView,
    ctx: &UiEventContext<'_, B>,
) {
    let DirectoryView {
        rooms,
        space_matches,
        spaces,
        subspaces,
        scope,
        space_id,
        subspace_id,
        listed_space,
        direct_flags,
    } = directory;

    if last.is_none_or(|l| !Arc::ptr_eq(&l.rooms, rooms) || l.space_matches != *space_matches) {
        apply_rooms::<B>(
            &ctx.models.rooms,
            RoomListing::of(directory),
            last.map_or_else(RoomListing::default, RoomListing::of),
            ctx.media,
        );
        refresh_selected_room_avatar(w, ctx.media);
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.spaces, spaces)) {
        apply_spaces(
            &ctx.models.spaces,
            spaces.as_ref(),
            ctx.media,
            &|space| B::convert_space(space, ctx.media),
            &|entry| entry.id(),
        );
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.subspaces, subspaces)) {
        apply_spaces(
            &ctx.models.subspaces,
            subspaces.as_ref(),
            ctx.media,
            &|space| B::convert_space(space, ctx.media),
            &|entry| entry.id(),
        );
    }
    if last.is_none_or(|l| l.scope != *scope) {
        w.set_room_scope(*scope);
    }
    if last.is_none_or(|l| l.space_id != *space_id) {
        w.set_string(StringProp::SelectedSpaceId, SharedString::from(space_id));
    }
    if last.is_none_or(|l| l.subspace_id != *subspace_id) {
        w.set_string(
            StringProp::SelectedSubspaceId,
            SharedString::from(subspace_id),
        );
    }
    if last.is_none_or(|l| l.listed_space != *listed_space) {
        w.set_string(
            StringProp::ListedSpaceName,
            SharedString::from(&listed_space.name),
        );
        w.set_int(
            IntProp::ListedSpaceMembers,
            i32::try_from(listed_space.member_count).unwrap_or(i32::MAX),
        );
    }
    if last.is_none_or(|l| l.direct_flags != *direct_flags) {
        w.set_bool(BoolProp::DirectAlert, direct_flags.alert);
        w.set_bool(BoolProp::DirectMention, direct_flags.mention);
        w.set_bool(BoolProp::DirectHint, direct_flags.hint);
    }
}

fn apply_space_index<B: UiBackend>(
    w: &B::Window,
    last: Option<&SpaceIndexView>,
    space_index: &SpaceIndexView,
    ctx: &UiEventContext<'_, B>,
) {
    let SpaceIndexView {
        status,
        space_name,
        rows,
        avatars_ready,
        pages_landed,
    } = space_index;

    if last.is_none_or(|l| l.status != *status) {
        w.set_space_index_status(*status);
    }
    if last.is_none_or(|l| l.space_name != *space_name) {
        w.set_string(StringProp::SpaceIndexName, SharedString::from(space_name));
    }
    if last.is_none_or(|l| l.pages_landed != *pages_landed) {
        w.set_int(IntProp::SpaceIndexPagesLanded, *pages_landed);
        run_change_handlers_next_frame(w);
    }
    let rows_changed = last.is_none_or(|l| !Arc::ptr_eq(&l.rows, rows));
    let avatars_landed = last.is_some_and(|l| l.avatars_ready != *avatars_ready);
    if !rows_changed && !avatars_landed {
        return;
    }
    let previous = last
        .filter(|_| !avatars_landed)
        .map_or(&[] as &[_], |l| l.rows.as_ref());
    apply_space_children(
        &ctx.models.space_children,
        rows.as_ref(),
        previous,
        ctx.media,
        &|row| B::convert_space_child(row, ctx.media),
        &|entry| entry.id(),
    );
}

fn apply_room_info<B: UiBackend>(
    w: &B::Window,
    last: Option<&RoomInfoView>,
    room_info: &RoomInfoView,
    ctx: &UiEventContext<'_, B>,
) {
    let RoomInfoView {
        placement,
        card,
        about,
        roster,
        rows,
        has_more,
        pages_landed,
        avatars_ready,
        notify,
        notify_busy,
        leaving,
        error,
    } = room_info;

    if last.is_none_or(|l| l.card != *card) {
        apply_room_card(w, card.as_ref(), ctx.media);
    }
    if last.is_none_or(|l| l.placement != *placement) {
        w.set_room_info_placement(*placement);
    }
    if last.is_none_or(|l| l.card != *card || l.about != *about) {
        apply_room_topic(w, card.as_ref(), about.as_ref());
    }
    if last.is_none_or(|l| l.about != *about) {
        let joined_on = about
            .as_ref()
            .and_then(|about| about.joined_at)
            .map(message_sent_at_label)
            .unwrap_or_default();
        let link = about.as_ref().map_or("", |about| about.link.as_str());
        w.set_string(StringProp::RoomInfoJoinedOn, SharedString::from(joined_on));
        w.set_string(StringProp::RoomInfoLink, SharedString::from(link));
    }
    if last.is_none_or(|l| l.roster != *roster) {
        w.set_room_info_roster(*roster);
    }
    if last.is_none_or(|l| l.has_more != *has_more) {
        w.set_bool(BoolProp::RoomInfoHasMore, *has_more);
    }
    if last.is_none_or(|l| l.notify != *notify) {
        w.set_room_info_notify(*notify);
    }
    if last.is_none_or(|l| l.notify_busy != *notify_busy) {
        w.set_bool(BoolProp::RoomInfoNotifyBusy, *notify_busy);
    }
    if last.is_none_or(|l| l.leaving != *leaving) {
        w.set_bool(BoolProp::RoomInfoLeaving, *leaving);
    }
    if last.is_none_or(|l| l.error != *error) {
        w.set_room_info_error(error.kind);
        w.set_string(
            StringProp::RoomInfoErrorDetail,
            SharedString::from(&error.detail),
        );
    }
    if last.is_none_or(|l| l.pages_landed != *pages_landed) {
        w.set_int(IntProp::RoomInfoPagesLanded, *pages_landed);
        run_change_handlers_next_frame(w);
    }
    let rows_changed = last.is_none_or(|l| !Arc::ptr_eq(&l.rows, rows));
    let avatars_landed = last.is_some_and(|l| l.avatars_ready != *avatars_ready);
    if !rows_changed && !avatars_landed {
        return;
    }
    let previous = last
        .filter(|_| !avatars_landed)
        .map_or(&[] as &[_], |l| l.rows.as_ref());
    apply_member_rows(
        &ctx.models.room_members,
        rows.as_ref(),
        previous,
        ctx.media,
        &|row| B::convert_member_row(row, ctx.media),
        &|entry| entry.user_id(),
    );
}

fn apply_room_card(w: &impl UiProps, card: Option<&RoomCard>, media: &dyn MediaCache) {
    let Some(card) = card else {
        w.apply_room_info_avatar(None);
        return;
    };
    w.set_string(
        StringProp::RoomInfoRoomId,
        SharedString::from(card.id.as_ref()),
    );
    w.set_string(StringProp::RoomInfoName, SharedString::from(&card.name));
    w.set_string(
        StringProp::RoomInfoInitial,
        SharedString::from(avatar_initials(&card.name)),
    );
    w.set_int(IntProp::RoomInfoColorIndex, avatar_color_index(&card.id));
    w.set_int(
        IntProp::RoomInfoMembers,
        i32::try_from(card.member_count).unwrap_or(i32::MAX),
    );
    w.set_bool(BoolProp::RoomInfoIsDirect, card.is_direct);
    w.set_string(
        StringProp::RoomInfoAlias,
        SharedString::from(card.alias.as_deref().unwrap_or_default()),
    );
    w.apply_room_info_avatar(load_room_info_avatar(card, media));
}

fn apply_room_topic(w: &impl UiProps, card: Option<&RoomCard>, about: Option<&RoomAbout>) {
    let Some(topic) = shown_topic(card, about) else {
        w.set_string(StringProp::RoomInfoTopic, SharedString::default());
        w.set_bool(BoolProp::RoomInfoTopicHasLinks, false);
        w.apply_room_info_topic(StyledText::default());
        return;
    };
    let body = rich_body(&topic);
    w.set_string(StringProp::RoomInfoTopic, body.plain);
    w.set_bool(BoolProp::RoomInfoTopicHasLinks, body.has_links);
    w.apply_room_info_topic(body.styled);
}

fn shown_topic(card: Option<&RoomCard>, about: Option<&RoomAbout>) -> Option<RichText> {
    let plain = card?.topic.as_ref()?;
    match about.and_then(|about| about.topic.as_ref()) {
        Some(rich) if rich.plain == *plain => Some(rich.clone()),
        _ => Some(RichText::plain(plain.clone())),
    }
}

fn apply_user_info(
    w: &impl UiProps,
    last: Option<&UserInfoView>,
    user_info: &UserInfoView,
    media: &dyn MediaCache,
) {
    let UserInfoView {
        card,
        direct,
        ignore_busy,
        moderating,
        avatars_ready,
        error,
    } = user_info;

    if last.is_none_or(|l| l.direct != *direct) {
        w.set_user_info_direct(*direct);
    }
    if last.is_none_or(|l| l.ignore_busy != *ignore_busy) {
        w.set_bool(BoolProp::UserInfoIgnoreBusy, *ignore_busy);
    }
    if last.is_none_or(|l| l.moderating != *moderating) {
        w.set_user_info_moderating(*moderating);
    }
    if last.is_none_or(|l| l.card != *card) {
        apply_user_card(w, card.as_ref(), media);
    } else if last.is_some_and(|l| l.avatars_ready != *avatars_ready) {
        let avatar = card
            .as_ref()
            .and_then(|card| load_user_info_avatar(&card.profile, media));
        w.apply_user_info_avatar(avatar);
    }
    if last.is_none_or(|l| l.error != *error) {
        w.set_user_info_error(error.kind);
        w.set_string(
            StringProp::UserInfoErrorDetail,
            SharedString::from(&error.detail),
        );
    }
}

fn apply_user_card(w: &impl UiProps, card: Option<&UserCard>, media: &dyn MediaCache) {
    let Some(card) = card else {
        w.set_bool(BoolProp::UserInfoVisible, false);
        w.apply_user_info_avatar(None);
        return;
    };
    let profile = &card.profile;
    let label = profile.label();
    w.set_string(
        StringProp::UserInfoUserId,
        SharedString::from(profile.user_id.as_ref()),
    );
    w.set_string(StringProp::UserInfoName, SharedString::from(label));
    w.set_string(
        StringProp::UserInfoInitial,
        SharedString::from(avatar_initials(label)),
    );
    w.set_int(
        IntProp::UserInfoColorIndex,
        avatar_color_index(&profile.user_id),
    );
    w.set_string(StringProp::UserInfoPronouns, user_info_pronouns(profile));
    w.set_string(StringProp::UserInfoLink, SharedString::from(&profile.link));
    w.set_user_info_role(profile.role);
    w.set_user_info_membership(profile.membership);
    w.set_bool(
        BoolProp::UserInfoVerified,
        profile.trust == IdentityTrust::Verified,
    );
    w.set_bool(BoolProp::UserInfoIsSelf, profile.is_self);
    w.set_bool(BoolProp::UserInfoIgnored, profile.ignored);
    w.set_bool(BoolProp::UserInfoMayKick, profile.offers(Moderation::Kick));
    w.set_bool(BoolProp::UserInfoMayBan, profile.offers(Moderation::Ban));
    w.set_bool(
        BoolProp::UserInfoMayUnban,
        profile.offers(Moderation::Unban),
    );
    w.set_bool(
        BoolProp::UserInfoReadFailed,
        card.status == CardStatus::ReadFailed,
    );
    w.set_bool(
        BoolProp::UserInfoRetrying,
        card.status == CardStatus::Retrying,
    );
    w.apply_user_info_avatar(load_user_info_avatar(profile, media));
    w.set_bool(BoolProp::UserInfoVisible, true);
}

fn run_change_handlers_next_frame(w: &impl ComponentHandle) {
    w.window().request_redraw();
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

fn sync_timeline_chrome(w: &(impl UiProps + ComponentHandle), pagination: &PaginationView) {
    let (older_history, forwards, badge) = if pagination.generation == active_generation() {
        (
            pagination.older_history,
            pagination.forwards_loading,
            pagination.new_messages,
        )
    } else {
        (OlderHistory::Unknown, false, 0)
    };
    w.set_bool(
        BoolProp::BackwardsLoading,
        older_history == OlderHistory::Loading,
    );
    w.set_bool(
        BoolProp::OlderHistoryAvailable,
        older_history == OlderHistory::Available,
    );
    w.set_bool(BoolProp::ForwardsLoading, forwards);
    w.set_int(
        IntProp::NewMessagesCount,
        i32::try_from(badge).unwrap_or(i32::MAX),
    );
    run_change_handlers_next_frame(w);
}

fn apply_pinned(w: &impl UiProps, pinned: &PinnedView) {
    let selected = w.get_string(StringProp::SelectedRoomId);
    let shown = pinned.shown_in(selected.as_str());
    let (count, index) = match shown {
        Some(_) => (pinned.messages.len(), pinned.shown),
        None => (0, 0),
    };
    w.set_int(
        IntProp::PinnedCount,
        i32::try_from(count).unwrap_or(i32::MAX),
    );
    w.set_int(
        IntProp::PinnedIndex,
        i32::try_from(index).unwrap_or(i32::MAX),
    );
    w.set_string(
        StringProp::PinnedEventId,
        SharedString::from(shown.map_or("", |message| message.event_id.as_str())),
    );
    w.set_pinned_kind(shown.map_or(MessagePreviewKind::None, |message| message.kind));
    w.set_string(
        StringProp::PinnedBody,
        shown
            .map(|message| preview_line(&message.body))
            .unwrap_or_default(),
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
        recognise_own_user(user_id);
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
    let reply = match draft {
        Some(Draft::Message(message)) => message.reply.as_ref(),
        Some(Draft::Edit(_)) | None => None,
    };
    let edit = match draft {
        Some(Draft::Edit(edit)) => Some(edit),
        Some(Draft::Message(_)) | None => None,
    };
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
        SharedString::from(draft.map_or("", Draft::body)),
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
    apply_unsent_edit(w, edit);
}

fn apply_unsent_edit(w: &impl UiProps, edit: Option<&MessageEdit>) {
    w.set_bool(BoolProp::UnsentIsEdit, edit.is_some());
    w.set_bool(
        BoolProp::UnsentEditCaption,
        edit.is_some_and(|edit| edit.kind == EditKind::Caption),
    );
    w.set_string(
        StringProp::UnsentEditEventId,
        SharedString::from(
            edit.and_then(|edit| edit.target.event_id())
                .unwrap_or_default(),
        ),
    );
    w.set_string(
        StringProp::UnsentEditLocalId,
        SharedString::from(
            edit.and_then(|edit| edit.target.local_id())
                .unwrap_or_default(),
        ),
    );
    w.set_string(
        StringProp::UnsentEditPreview,
        edit.and_then(|edit| edit.original.clone())
            .map(|original| preview_line(&RichText::plain(original)))
            .unwrap_or_default(),
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

fn show_selected_room_avatar(
    w: &impl UiProps,
    room_id: &str,
    mxc: Option<&str>,
    media: &dyn MediaCache,
) {
    let next = SelectedRoomAvatar {
        room_id: room_id.to_owned(),
        mxc: mxc.map(str::to_owned),
        path: mxc.and_then(|mxc| media.room_avatar_path(mxc)),
    };
    let previous = with_session(|session| mem::replace(&mut session.room.avatar, next.clone()));
    if previous == next {
        return;
    }
    let image = load_avatar_async(next.path.as_deref(), AvatarSlot::SelectedRoom);
    if image.is_some() || !previous.same_source(&next) {
        w.apply_selected_room_avatar(image);
    }
}

fn refresh_selected_room_avatar(w: &impl UiProps, media: &dyn MediaCache) {
    let shown = with_session(|session| session.room.avatar.clone());
    show_selected_room_avatar(w, &shown.room_id, shown.mxc.as_deref(), media);
}

fn clear_selected_room(w: &impl UiProps) {
    w.set_string(StringProp::SelectedRoomId, SharedString::default());
    w.set_string(StringProp::SelectedRoomName, SharedString::default());
    w.set_string(StringProp::SelectedRoomInitial, SharedString::default());
    w.set_int(IntProp::SelectedRoomColorIndex, 0);
    w.apply_selected_room_avatar(None);
    w.set_int(IntProp::SelectedRoomMembers, 0);
    w.set_bool(BoolProp::SelectedRoomEncrypted, false);
    apply_poll_permissions(w, PollPermissions::UNRESTRICTED);
    apply_message_permissions(w, MessagePermissions::UNRESTRICTED);
    w.set_int(IntProp::AnchorIndex, NO_ANCHOR);
    w.set_bool(BoolProp::TimelineDetached, false);
    publish_room_cursor(w);
    apply_timeline_status(w, TimelineStatus::None);
}

fn apply_poll_permissions(w: &impl UiProps, polls: PollPermissions) {
    w.set_bool(BoolProp::MayVote, polls.vote);
    w.set_bool(BoolProp::MayEndPolls, polls.end);
    w.set_bool(BoolProp::MayStartPolls, polls.start);
}

fn apply_message_permissions(w: &impl UiProps, messages: MessagePermissions) {
    w.set_bool(BoolProp::MayDeleteOwn, messages.delete_own);
    w.set_bool(BoolProp::MayDeleteOthers, messages.delete_others);
    w.set_bool(BoolProp::MayPin, messages.pin);
}
