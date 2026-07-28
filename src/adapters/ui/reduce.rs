use std::cell::{Cell, RefCell};
use std::sync::Arc;

use slint::SharedString;

use super::backend::{UiBackend, UiEventContext};
use super::decode::{AvatarSlot, clear_session_media, load_avatar_async};
use super::present::{VerifyStep, user_initial, verification_cancellation};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::{apply_rooms, apply_spaces, apply_timeline_patch};
use crate::commands::{
    AppViewState, DirectoryView, Effect, LifecycleView, PaginationView, Toast, UserMessage,
    UserMessageKind, VerificationActivity, VerificationUpdate,
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
    let AppViewState {
        lifecycle,
        connection,
        directory,
        pagination,
        toast,
    } = view.as_ref();
    let DirectoryView {
        rooms,
        spaces,
        subspaces,
        space_id,
        subspace_id,
    } = directory;

    apply_lifecycle(w, last.map(|l| &l.lifecycle), lifecycle);
    if last.is_none_or(|l| l.connection != *connection) {
        w.set_connection_state(connection);
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.directory.rooms, rooms)) {
        apply_rooms(
            ctx.rooms,
            rooms.as_ref(),
            last.map_or(&[], |l| l.directory.rooms.as_ref()),
            ctx.media,
            &|room| B::convert_room(room, ctx.media),
            &|entry| B::room_id(entry),
        );
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.directory.spaces, spaces)) {
        apply_spaces(
            ctx.spaces,
            spaces.as_ref(),
            ctx.media,
            &|space| B::convert_space(space, ctx.media),
            &|entry| B::space_id(entry),
        );
    }
    if last.is_none_or(|l| !Arc::ptr_eq(&l.directory.subspaces, subspaces)) {
        apply_spaces(
            ctx.subspaces,
            subspaces.as_ref(),
            ctx.media,
            &|space| B::convert_space(space, ctx.media),
            &|entry| B::space_id(entry),
        );
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
    if last.is_none_or(|l| l.pagination != *pagination) {
        sync_timeline_chrome(w, pagination);
    }
    if last.is_none_or(|l| l.toast != *toast) {
        apply_toast(w, toast);
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
        let avatar = avatar_path
            .as_deref()
            .and_then(|p| load_avatar_async(p, AvatarSlot::User));
        w.apply_user_avatar(avatar);
    }
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
    w.set_int(IntProp::SelectedGeneration, 0);
}
