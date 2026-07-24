use std::cell::{Cell, RefCell};
use std::sync::Arc;

use slint::SharedString;

use super::backend::{UiBackend, UiEventContext};
use super::decode::{AvatarSlot, clear_session_media, load_avatar_async};
use super::present::{LoginMethodKind, Status, VerifyStep, login_method_kind, user_initial};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::{apply_reconcile, apply_rooms, apply_timeline_patch};
use crate::commands::{AppViewState, Effect, LifecycleView, LoginStep, PaginationView};
use crate::domain::models::{
    ConnectionStatus, RoomId, TimelinePatch, TimelineStatus,
    VerificationEvent as DomainVerificationEvent,
};

thread_local! {
    static PREPEND_TOKEN: Cell<i32> = const { Cell::new(0) };
    static ACTIVE_GENERATION: Cell<i32> = const { Cell::new(0) };
    static LATEST_SNAPSHOT: RefCell<Arc<AppViewState>> =
        RefCell::new(Arc::new(AppViewState::default()));
}

#[allow(clippy::too_many_lines)]
pub fn dispatch_effect<B: UiBackend>(w: &B::Window, event: Effect, ctx: &UiEventContext<'_, B>) {
    match event {
        Effect::LoginError(message) => apply_login_error(w, &message),
        Effect::Toast(message) => apply_toast_error(w, &message),
        Effect::Status(msg) => apply_status(w, &msg),
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
            LATEST_SNAPSHOT.with(|cell| sync_timeline_chrome(w, &cell.borrow().pagination));
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
        Effect::Verification(event) => apply_verification(w, &event),
        Effect::FileSaved { path } => {
            w.set_string(StringProp::SavedFilePath, SharedString::from(&path));
            w.set_string(
                StringProp::ToastMessage,
                SharedString::from(Status::FileSaved.as_str()),
            );
        }
        Effect::LoggedOut => {
            clear_session_media();
            ctx.timeline.set_vec(Vec::new());
            ctx.rooms.set_vec(Vec::new());
            ctx.spaces.set_vec(Vec::new());
            ctx.subspaces.set_vec(Vec::new());
            LATEST_SNAPSHOT.with(|cell| *cell.borrow_mut() = Arc::new(AppViewState::default()));
            apply_logged_out(w);
        }
    }
}

fn apply_snapshot<B: UiBackend>(
    w: &B::Window,
    view: &Arc<AppViewState>,
    ctx: &UiEventContext<'_, B>,
) {
    let last = LATEST_SNAPSHOT.with(|cell| Arc::clone(&cell.borrow()));
    apply_lifecycle(w, &last.lifecycle, &view.lifecycle);
    if last.connection != view.connection {
        w.set_connection_state(&view.connection);
    }
    if !Arc::ptr_eq(&last.directory.rooms, &view.directory.rooms) {
        apply_rooms(
            ctx.rooms,
            view.directory.rooms.as_ref(),
            &|room| B::convert_room(room, ctx.media),
            &|entry| B::room_id(entry),
        );
    }
    if !Arc::ptr_eq(&last.directory.spaces, &view.directory.spaces) {
        apply_reconcile(
            ctx.spaces,
            view.directory.spaces.as_ref(),
            &|s| s.id.as_str(),
            &|space| B::convert_space(space, ctx.media),
            &|entry| B::space_id(entry),
        );
    }
    if !Arc::ptr_eq(&last.directory.subspaces, &view.directory.subspaces) {
        apply_reconcile(
            ctx.subspaces,
            view.directory.subspaces.as_ref(),
            &|s| s.id.as_str(),
            &|space| B::convert_space(space, ctx.media),
            &|entry| B::space_id(entry),
        );
    }
    if last.directory.space_id != view.directory.space_id {
        w.set_string(
            StringProp::SelectedSpaceId,
            SharedString::from(&view.directory.space_id),
        );
    }
    if last.directory.subspace_id != view.directory.subspace_id {
        w.set_string(
            StringProp::SelectedSubspaceId,
            SharedString::from(&view.directory.subspace_id),
        );
    }
    if last.pagination != view.pagination {
        sync_timeline_chrome(w, &view.pagination);
    }
    LATEST_SNAPSHOT.with(|cell| *cell.borrow_mut() = Arc::clone(view));
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

fn apply_lifecycle(w: &impl UiProps, last: &LifecycleView, next: &LifecycleView) {
    if last.step != next.step {
        w.set_login_phase(next.step);
    }
    if last.method != next.method {
        w.set_login_method_kind(login_method_kind(next.method));
    }
    if last.resolved_homeserver != next.resolved_homeserver {
        w.set_string(
            StringProp::ResolvedHomeserver,
            SharedString::from(&next.resolved_homeserver),
        );
    }
    if last.user_id != next.user_id {
        w.set_string(StringProp::UserId, SharedString::from(&next.user_id));
        w.set_string(
            StringProp::UserInitial,
            SharedString::from(user_initial(&next.user_id)),
        );
    }
    if last.avatar_path != next.avatar_path {
        let avatar = next
            .avatar_path
            .as_deref()
            .and_then(|p| load_avatar_async(p, AvatarSlot::User));
        w.apply_user_avatar(avatar);
    }
}

fn apply_login_error(w: &impl UiProps, msg: &str) {
    w.set_string(StringProp::LoginError, SharedString::from(msg));
    w.set_string(StringProp::LoginStatus, SharedString::default());
}

fn apply_toast_error(w: &impl UiProps, msg: &str) {
    w.set_string(StringProp::ToastMessage, SharedString::from(msg));
}

fn apply_status(w: &impl UiProps, msg: &str) {
    w.set_string(StringProp::LoginStatus, SharedString::from(msg));
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

fn apply_verification(w: &impl UiProps, event: &DomainVerificationEvent) {
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

fn apply_logged_out(w: &impl UiProps) {
    w.set_login_phase(LoginStep::Homeserver);
    w.set_string(StringProp::UserId, SharedString::default());
    w.set_string(StringProp::UserInitial, SharedString::default());
    w.set_string(StringProp::LoginStatus, SharedString::default());
    w.set_string(StringProp::LoginError, SharedString::default());
    w.set_login_method_kind(LoginMethodKind::None);
    w.set_string(StringProp::ResolvedHomeserver, SharedString::default());
    w.set_string(StringProp::SelectedRoomName, SharedString::default());
    w.set_string(StringProp::SelectedRoomId, SharedString::default());
    w.set_string(StringProp::SelectedSpaceId, SharedString::default());
    w.set_string(StringProp::SelectedSubspaceId, SharedString::default());
    w.clear_text_inputs();
    w.set_connection_state(&ConnectionStatus::Disconnected);
    w.set_bool(BoolProp::VerificationVisible, false);
    w.set_verification_phase(VerifyStep::None);
    w.set_string(StringProp::VerificationSender, SharedString::default());
    w.set_bool(BoolProp::VerificationIsSelf, false);
    w.set_string(StringProp::VerificationError, SharedString::default());
    w.set_string(StringProp::ToastMessage, SharedString::default());
    w.set_string(StringProp::SavedFilePath, SharedString::default());
    w.set_bool(BoolProp::BackwardsLoading, false);
    w.set_bool(BoolProp::ForwardsLoading, false);
    w.set_int(IntProp::NewMessagesCount, 0);
    w.set_int(IntProp::SelectedRoomMembers, 0);
    w.set_int(IntProp::SelectedGeneration, 0);
    w.apply_user_avatar(None);
    w.clear_emoji_model();
}
