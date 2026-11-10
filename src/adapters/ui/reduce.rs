mod composer;
mod directory;
mod media;
mod security;
mod settings;
mod sheets;
mod timeline;

use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use self::composer::{apply_attachment, apply_composer_banners, apply_mentions, apply_unsent};
use self::directory::{apply_directory, apply_room_menu, apply_space_index};
use self::media::{apply_audio, apply_packs, apply_video, audio_start, rebuild_pack_grid};
use self::security::{
    apply_account, apply_encryption, apply_lifecycle, apply_logout, apply_verification,
    reset_verification,
};
pub(super) use self::settings::seed_settings;
use self::settings::{apply_notification_rules, apply_settings};
use self::sheets::{
    apply_create_room, apply_invite, apply_invite_people, apply_leave_space, apply_removal,
    apply_room_info, apply_room_log, apply_room_preview, apply_user_info,
};
use self::timeline::{
    apply_copied_links, apply_pin_marks, apply_pinned, apply_readers, apply_room_people,
    apply_source, apply_timeline, apply_timeline_status, is_active, sync_timeline_chrome,
};
use super::backend::{UiBackend, UiEventContext};
use super::call::apply_call;
use super::decode::{AvatarSlot, load_avatar_async};
use super::dto::PackBrowse;
use super::notices::apply_notifications;
use super::present::{avatar_color_index, avatar_initials};
use super::props::{BoolProp, ImageProp, IntProp, StringProp, UiProps};
use super::reconcile::RoomListing;
use super::rows::{NO_ROW, row_index};
use super::session::{begin_session, from_snapshot, with_session};
use super::{call_window, emote, sending};
use crate::commands::effects::Effect;
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{AppViewState, CallView, DirectoryView, NotificationsView, Toast};
use crate::domain::message::MessagePermissions;
use crate::domain::poll::PollPermissions;
use crate::domain::timeline::TimelineStatus;
use crate::ports::media::MediaCache;
use crate::util::saturating_i32;

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

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SheetAvatar {
    room_id: String,
    path: Option<PathBuf>,
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
    w.set_text(StringProp::FocusEventId, event_id.unwrap_or_default());
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
    w.set_text(StringProp::FocusEventId, focus.unwrap_or_default());
}

pub(super) fn latest_directory() -> Option<DirectoryView> {
    from_snapshot(|view| view.directory.clone())
}

pub(super) fn set_pack_browse<B: UiBackend>(
    w: &B::Window,
    browse: PackBrowse,
    media: &dyn MediaCache,
) {
    let packs = with_session(|session| {
        if session.pack_browse.as_ref() == Some(&browse) {
            return None;
        }
        session.pack_browse = Some(browse);
        session
            .snapshot
            .as_ref()
            .map(|view| view.image_packs.clone())
    });
    if let Some(packs) = packs {
        rebuild_pack_grid::<B>(w, &packs, media);
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
            event_ids,
            readable_by_anyone,
            polls,
            messages,
            generation,
            live,
        } => {
            emote::follow_room(id.as_ref());
            sending::follow_room(&id, encrypted, event_ids, messages);
            if adopt_selected_generation(generation) {
                set_focus(w, None);
            }
            w.set_bool(BoolProp::TimelineDetached, !live);
            w.set_int(IntProp::SelectedGeneration, generation);
            w.set_text(StringProp::SelectedRoomId, id.as_ref());
            let listed = latest_directory().unwrap_or_default();
            publish_selected_room_row(w, RoomListing::of(&listed));
            w.set_text(StringProp::SelectedRoomName, &name);
            w.set_text(StringProp::SelectedRoomInitial, avatar_initials(&name));
            w.set_int(
                IntProp::SelectedRoomColorIndex,
                avatar_color_index(id.as_ref()),
            );
            show_selected_room_avatar(w, id.as_ref(), avatar_mxc.as_deref(), ctx.media);
            w.set_int(IntProp::SelectedRoomMembers, saturating_i32(member_count));
            w.set_bool(BoolProp::SelectedRoomEncrypted, encrypted);
            w.set_bool(BoolProp::SelectedRoomReadableByAnyone, readable_by_anyone);
            apply_poll_permissions(w, polls);
            apply_message_permissions(w, messages);
            let (pagination, pinned) =
                from_snapshot(|view| (view.pagination, view.pinned.clone())).unwrap_or_default();
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
                let anchor = i32::try_from(row).unwrap_or(NO_ROW);
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
            ctx.notifier.withdraw_all();
            call_window::close::<B>(w);
            let models = begin_session::<B>(w);
            let fresh = UiEventContext::<B> {
                models: &models,
                media: ctx.media,
                notifier: ctx.notifier,
            };
            apply_snapshot::<B>(w, &Arc::new(*view), &fresh);
            clear_selected_room(w);
            reset_verification(w);
            w.clear_text_inputs();
        }
    }
}

#[allow(clippy::too_many_lines)]
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
        invite,
        removal,
        leave_space,
        room_preview,
        create_room,
        invite_people,
        room_info,
        room_menu,
        user_info,
        pagination,
        pinned,
        image_packs: stickers,
        attachment,
        video,
        audio,
        unsent,
        identity_alert,
        upgrade,
        typing,
        presence,
        call,
        notifications,
        encryption,
        account,
        settings,
        logout,
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
    apply_invite(w, last, invite, &directory.invites, ctx.media);
    apply_removal(w, last, removal, &directory.removals, ctx.media);
    apply_leave_space(w, last.map(|l| &l.leave_space), leave_space);
    apply_room_preview(w, last.map(|l| &l.room_preview), room_preview, ctx.media);
    apply_create_room(w, last.map(|l| &l.create_room), create_room);
    apply_invite_people::<B>(w, last.map(|l| &l.invite_people), invite_people, ctx);
    apply_room_info::<B>(w, last.map(|l| &l.room_info), room_info, ctx);
    if last.is_none_or(|l| l.room_menu != *room_menu) {
        apply_room_menu(w, room_menu.as_ref());
    }
    apply_user_info::<B>(w, last.map(|l| &l.user_info), user_info, ctx);
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
        !Arc::ptr_eq(&l.image_packs.packs, &stickers.packs)
            || l.image_packs.generation != stickers.generation
            || l.image_packs.ready_images != stickers.ready_images
            || l.image_packs.room_encrypted != stickers.room_encrypted
            || l.image_packs.loading != stickers.loading
    }) {
        apply_packs::<B>(w, last.map(|l| &l.image_packs), stickers, ctx.media);
    }
    apply_attachment(w, last.map(|l| &l.attachment), attachment);
    if last.is_none_or(|l| l.video != *video) {
        apply_video::<B>(w, video);
    }
    if last.is_none_or(|l| l.audio != *audio) {
        apply_audio::<B>(w, last.map(|l| &l.audio), audio, audio_start(video), ctx);
    }
    if last.is_none_or(|l| l.unsent != *unsent) {
        apply_unsent(w, unsent.as_ref());
    }
    apply_composer_banners(w, last, identity_alert, upgrade);
    apply_room_people(w, last, typing, presence);
    apply_notices::<B>(w, last, notifications, call, ctx);
    apply_encryption(w, last.map(|l| &l.encryption), encryption);
    apply_account::<B>(w, last.map(|l| &l.account), account, ctx);
    apply_settings(w, last.map(|l| &l.settings), settings);
    apply_notification_rules::<B>(w, last.map(|l| &l.settings.rules), &settings.rules, ctx);
    apply_logout(w, last.map(|l| &l.logout), logout);
    apply_toast(w, last.map(|l| &l.toast), toast);
    apply_copied_links(w, last, message_link, room_link);
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

fn apply_notices<B: UiBackend>(
    w: &B::Window,
    last: Option<&AppViewState>,
    notifications: &NotificationsView,
    call: &CallView,
    ctx: &UiEventContext<'_, B>,
) {
    apply_notifications::<B>(w, last, notifications, ctx.notifier);
    apply_call::<B>(w, last, call, ctx);
}

fn run_change_handlers_next_frame(w: &impl ComponentHandle) {
    w.window().request_redraw();
}

fn apply_toast(w: &impl UiProps, last: Option<&Toast>, toast: &Toast) {
    if last == Some(toast) {
        return;
    }
    let (kind, detail) = match toast {
        Toast::None => (UserMessageKind::None, ""),
        Toast::Error(message) => (message.kind, message.detail.as_str()),
        Toast::FileSaved(path) => (UserMessageKind::FileSaved, path.as_str()),
    };
    w.set_toast_message(kind);
    w.set_text(StringProp::ToastDetail, detail);
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
        w.set_image(ImageProp::SelectedRoomAvatar, image);
    }
}

fn publish_selected_room_row(w: &impl UiProps, listing: RoomListing<'_>) {
    let selected = w.get_string(StringProp::SelectedRoomId);
    let place = listing.place_of(selected.as_str());
    let row = row_index(place.as_ref().map(|place| place.row));
    let headings = place
        .and_then(|place| i32::try_from(place.headings_above).ok())
        .unwrap_or_default();
    w.set_int(IntProp::SelectedRoomHeadings, headings);
    w.set_int(IntProp::SelectedRoomRow, row);
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
    w.set_image(ImageProp::SelectedRoomAvatar, None);
    w.set_int(IntProp::SelectedRoomRow, NO_ROW);
    w.set_int(IntProp::SelectedRoomHeadings, 0);
    w.set_int(IntProp::SelectedRoomMembers, 0);
    w.set_bool(BoolProp::SelectedRoomEncrypted, false);
    w.set_bool(BoolProp::SelectedRoomReadableByAnyone, false);
    apply_poll_permissions(w, PollPermissions::UNRESTRICTED);
    apply_message_permissions(w, MessagePermissions::UNRESTRICTED);
    w.set_int(IntProp::AnchorIndex, NO_ROW);
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
    w.set_bool(BoolProp::MaySendMessages, messages.send_messages);
    w.set_bool(BoolProp::MayReact, messages.react);
    w.set_bool(BoolProp::MaySendStickers, messages.send_stickers);
    w.set_bool(BoolProp::MayDeleteOwn, messages.delete_own);
    w.set_bool(BoolProp::MayDeleteOthers, messages.delete_others);
    w.set_bool(BoolProp::MayPin, messages.pin);
}
