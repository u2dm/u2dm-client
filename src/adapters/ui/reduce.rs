use std::cell::{Cell, RefCell};
use std::sync::Arc;

use slint::SharedString;

use super::backend::{UiBackend, UiEventContext};
use super::decode::{AvatarSlot, clear_session_media, load_avatar_async};
use super::present::{VerifyStep, toast_kind, user_initial};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::{apply_reconcile, apply_rooms, apply_timeline_patch};
use crate::commands::{
    AppViewState, Effect, LifecycleView, PaginationView, Toast, VerificationUpdate,
};
use crate::domain::models::{
    Room, RoomId, TimelinePatch, TimelineStatus, VerificationEvent as DomainVerificationEvent,
};

thread_local! {
    static PREPEND_TOKEN: Cell<i32> = const { Cell::new(0) };
    static ACTIVE_GENERATION: Cell<i32> = const { Cell::new(0) };
    static LATEST_SNAPSHOT: RefCell<Option<Arc<AppViewState>>> = const { RefCell::new(None) };
}

pub(super) fn latest_rooms() -> Option<Arc<[Room]>> {
    LATEST_SNAPSHOT.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|view| Arc::clone(&view.directory.rooms))
    })
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
            ACTIVE_GENERATION.with(|g| g.set(generation));
            w.set_int(IntProp::SelectedGeneration, generation);
            w.set_string(StringProp::SelectedRoomId, SharedString::from(id.as_ref()));
            w.set_string(StringProp::SelectedRoomName, SharedString::from(&name));
            w.set_int(
                IntProp::SelectedRoomMembers,
                i32::try_from(member_count).unwrap_or(i32::MAX),
            );
            let pagination = LATEST_SNAPSHOT
                .with(|cell| cell.borrow().as_ref().map(|view| view.pagination))
                .unwrap_or_default();
            sync_timeline_chrome(w, &pagination);
        }
        Effect::Timeline {
            room_id,
            generation,
            patch,
        } => {
            let selected = w.get_string(StringProp::SelectedRoomId);
            let matches = is_active(w, &room_id, generation);
            tracing::debug!(
                patch = patch.label(),
                %room_id,
                generation,
                %selected,
                matches,
                "dispatch_effect received Timeline event"
            );
            if matches {
                if matches!(patch.as_ref(), TimelinePatch::Reset(_)) {
                    apply_timeline_status(w, TimelineStatus::Ready);
                }
                if patch.is_prepend() {
                    let next = PREPEND_TOKEN.with(|t| {
                        let next = t.get().wrapping_add(1);
                        t.set(next);
                        next
                    });
                    w.set_int(IntProp::PrependToken, next);
                }
                apply_timeline_patch(
                    ctx.timeline,
                    *patch,
                    &|m| B::convert_message(m, ctx.media),
                    &|entry, delta| B::enrich_message(entry, delta, ctx.media),
                    &|entry| B::message_id(entry),
                );
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
        Effect::LoggedOut => {
            clear_session_media();
            ctx.timeline.set_vec(Vec::new());
            apply_snapshot::<B>(w, &Arc::new(AppViewState::logged_out()), ctx);
            clear_selected_room(w);
            reset_verification(w);
            w.clear_text_inputs();
        }
    }
}

fn apply_snapshot<B: UiBackend>(
    w: &B::Window,
    view: &Arc<AppViewState>,
    ctx: &UiEventContext<'_, B>,
) {
    let previous = LATEST_SNAPSHOT.with(|cell| cell.borrow().clone());
    let last = previous.as_deref();
    apply_lifecycle(w, last.map(|l| &l.lifecycle), &view.lifecycle);
    if last.is_none_or(|l| l.connection != view.connection) {
        w.set_connection_state(&view.connection);
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.directory.rooms, &view.directory.rooms)) {
        apply_rooms(
            ctx.rooms,
            view.directory.rooms.as_ref(),
            &|room| B::convert_room(room, ctx.media),
            &|entry| B::room_id(entry),
        );
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.directory.spaces, &view.directory.spaces)) {
        apply_reconcile(
            ctx.spaces,
            view.directory.spaces.as_ref(),
            &|s| s.id.as_str(),
            &|space| B::convert_space(space, ctx.media),
            &|entry| B::space_id(entry),
        );
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.directory.subspaces, &view.directory.subspaces)) {
        apply_reconcile(
            ctx.subspaces,
            view.directory.subspaces.as_ref(),
            &|s| s.id.as_str(),
            &|space| B::convert_space(space, ctx.media),
            &|entry| B::space_id(entry),
        );
    }
    if last.is_none_or(|l| l.directory.space_id != view.directory.space_id) {
        w.set_string(
            StringProp::SelectedSpaceId,
            SharedString::from(&view.directory.space_id),
        );
    }
    if last.is_none_or(|l| l.directory.subspace_id != view.directory.subspace_id) {
        w.set_string(
            StringProp::SelectedSubspaceId,
            SharedString::from(&view.directory.subspace_id),
        );
    }
    if last.is_none_or(|l| l.pagination != view.pagination) {
        sync_timeline_chrome(w, &view.pagination);
    }
    if last.is_none_or(|l| l.toast != view.toast) {
        apply_toast(w, &view.toast);
    }
    LATEST_SNAPSHOT.with(|cell| *cell.borrow_mut() = Some(Arc::clone(view)));
}

fn sync_timeline_chrome(w: &impl UiProps, pagination: &PaginationView) {
    let active = ACTIVE_GENERATION.with(Cell::get);
    let (backwards, forwards, badge) = if pagination.generation == active {
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
    if last.is_none_or(|l| l.step != next.step) {
        w.set_login_phase(next.step);
    }
    if last.is_none_or(|l| l.activity != next.activity) {
        w.set_login_activity(next.activity);
    }
    if last.is_none_or(|l| l.error != next.error) {
        w.set_string(StringProp::LoginError, SharedString::from(&next.error));
    }
    if last.is_none_or(|l| l.method != next.method) {
        w.set_login_method_kind(next.method);
    }
    if last.is_none_or(|l| l.resolved_homeserver != next.resolved_homeserver) {
        w.set_string(
            StringProp::ResolvedHomeserver,
            SharedString::from(&next.resolved_homeserver),
        );
    }
    if last.is_none_or(|l| l.user_id != next.user_id) {
        w.set_string(StringProp::UserId, SharedString::from(&next.user_id));
        w.set_string(
            StringProp::UserInitial,
            SharedString::from(user_initial(&next.user_id)),
        );
    }
    if last.is_none_or(|l| l.avatar_path != next.avatar_path) {
        let avatar = next
            .avatar_path
            .as_deref()
            .and_then(|p| load_avatar_async(p, AvatarSlot::User));
        w.apply_user_avatar(avatar);
    }
}

fn apply_toast(w: &impl UiProps, toast: &Toast) {
    let (message, path) = match toast {
        Toast::None => ("", ""),
        Toast::Error(message) => (message.as_str(), ""),
        Toast::FileSaved(path) => ("", path.as_str()),
    };
    w.set_toast_kind(toast_kind(toast));
    w.set_string(StringProp::ToastMessage, SharedString::from(message));
    w.set_string(StringProp::SavedFilePath, SharedString::from(path));
}

fn is_active(w: &impl UiProps, room_id: &RoomId, generation: i32) -> bool {
    w.get_string(StringProp::SelectedRoomId).as_str() == room_id.as_ref()
        && ACTIVE_GENERATION.with(Cell::get) == generation
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
        VerificationUpdate::Rejecting => w.set_bool(BoolProp::VerificationBusy, true),
        VerificationUpdate::RejectFailed(reason) => {
            w.set_bool(BoolProp::VerificationBusy, false);
            w.set_string(
                StringProp::VerificationError,
                SharedString::from(reason.as_str()),
            );
        }
        VerificationUpdate::Dismissed => reset_verification(w),
    }
}

fn apply_verification_flow(w: &impl UiProps, event: &DomainVerificationEvent) {
    w.set_bool(BoolProp::VerificationBusy, false);
    match event {
        DomainVerificationEvent::Requested { sender, is_self } => {
            w.set_bool(BoolProp::VerificationVisible, true);
            w.set_verification_phase(VerifyStep::Requested);
            w.set_string(
                StringProp::VerificationSender,
                SharedString::from(sender.as_str()),
            );
            w.set_bool(BoolProp::VerificationIsSelf, *is_self);
            w.set_string(StringProp::VerificationError, SharedString::default());
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
            w.set_string(
                StringProp::VerificationError,
                SharedString::from(reason.as_str()),
            );
        }
    }
}

fn reset_verification(w: &impl UiProps) {
    w.set_bool(BoolProp::VerificationVisible, false);
    w.set_bool(BoolProp::VerificationBusy, false);
    w.set_verification_phase(VerifyStep::None);
    w.set_string(StringProp::VerificationSender, SharedString::default());
    w.set_bool(BoolProp::VerificationIsSelf, false);
    w.set_string(StringProp::VerificationError, SharedString::default());
    w.clear_emoji_model();
}

fn clear_selected_room(w: &impl UiProps) {
    w.set_string(StringProp::SelectedRoomId, SharedString::default());
    w.set_string(StringProp::SelectedRoomName, SharedString::default());
    w.set_int(IntProp::SelectedRoomMembers, 0);
    w.set_int(IntProp::SelectedGeneration, 0);
}
