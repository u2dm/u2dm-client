use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use super::{
    publish_selected_room_row, refresh_selected_room_avatar, run_change_handlers_next_frame,
};
use crate::adapters::ui::backend::{UiBackend, UiEventContext};
use crate::adapters::ui::dto::{space_child_to_dto, space_to_dto};
use crate::adapters::ui::fields::{SpaceChildFields, SpaceFields};
use crate::adapters::ui::props::{BoolProp, IntProp, StringProp, UiProps};
use crate::adapters::ui::reconcile::{RoomListing, apply_rooms, apply_rows, rows_to_redo};
use crate::adapters::ui::rows::row_index;
use crate::commands::view::{DirectoryView, RoomMenuTarget, SpaceHeading, SpaceIndexView};
use crate::util::saturating_i32;

pub(super) fn apply_room_menu(w: &(impl UiProps + ComponentHandle), menu: Option<&RoomMenuTarget>) {
    let Some(menu) = menu else {
        w.set_string(StringProp::RoomMenuRoomId, SharedString::default());
        run_change_handlers_next_frame(w);
        return;
    };
    w.set_text(StringProp::RoomMenuName, &menu.name);
    w.set_bool(BoolProp::RoomMenuUnread, menu.unread);
    w.set_room_menu_notify(menu.notify);
    w.set_bool(BoolProp::RoomMenuNotifyBusy, menu.notify_busy);
    w.set_bool(BoolProp::RoomMenuLeaving, menu.leaving);
    w.set_bool(BoolProp::RoomMenuMayInvite, menu.may_invite);
    w.set_bool(BoolProp::RoomMenuIsSpace, menu.is_space);
    w.set_bool(BoolProp::RoomMenuMayAddChildren, menu.may_add_children);
    w.set_bool(BoolProp::RoomMenuHasCall, menu.has_call);
    w.set_text(StringProp::RoomMenuRoomId, menu.room_id.as_ref());
    run_change_handlers_next_frame(w);
}

fn publish_heading_rows(w: &impl UiProps, listing: RoomListing<'_>) {
    let headings = listing.heading_rows();
    w.set_int(IntProp::RoomsHeadingRow, row_index(headings.rooms));
    w.set_int(IntProp::SpacesHeadingRow, row_index(headings.spaces));
}

pub(super) fn apply_directory<B: UiBackend>(
    w: &B::Window,
    last: Option<&DirectoryView>,
    directory: &DirectoryView,
    ctx: &UiEventContext<'_, B>,
) {
    let DirectoryView {
        rooms,
        invites,
        invites_section,
        removals,
        space_matches,
        spaces,
        subspaces,
        scope,
        space_id,
        space_name,
        subspace_id,
        listed_space,
        direct_flags,
        invites_pending,
        attention,
    } = directory;

    if last.is_none_or(|l| {
        !Arc::ptr_eq(&l.rooms, rooms)
            || !Arc::ptr_eq(&l.invites, invites)
            || l.invites_section != *invites_section
            || !Arc::ptr_eq(&l.removals, removals)
            || l.space_matches != *space_matches
    }) {
        apply_rooms::<B>(
            &ctx.models.rooms,
            RoomListing::of(directory),
            last.map_or_else(RoomListing::default, RoomListing::of),
            ctx.media,
        );
        refresh_selected_room_avatar(w, ctx.media);
        publish_selected_room_row(w, RoomListing::of(directory));
        publish_heading_rows(w, RoomListing::of(directory));
    }
    for (model, listed, shown) in [
        (&ctx.models.spaces, spaces, last.map(|l| &l.spaces)),
        (&ctx.models.subspaces, subspaces, last.map(|l| &l.subspaces)),
    ] {
        if shown.is_none_or(|shown| !Arc::ptr_eq(shown, listed)) {
            apply_rows(
                model,
                listed.as_ref(),
                &[],
                ctx.media,
                &|space| space_to_dto(space, ctx.media).into(),
                &|entry| entry.id(),
            );
        }
    }
    if last.is_none_or(|l| l.scope != *scope) {
        w.set_room_scope(*scope);
    }
    if last.is_none_or(|l| l.space_id != *space_id) {
        w.set_text(StringProp::SelectedSpaceId, space_id);
    }
    if last.is_none_or(|l| l.space_name != *space_name) {
        w.set_text(StringProp::SelectedSpaceName, space_name);
    }
    if last.is_none_or(|l| l.subspace_id != *subspace_id) {
        w.set_text(StringProp::SelectedSubspaceId, subspace_id);
    }
    if last.is_none_or(|l| l.listed_space != *listed_space) {
        apply_listed_space(w, listed_space);
    }
    if last.is_none_or(|l| l.direct_flags != *direct_flags) {
        w.set_bool(BoolProp::DirectAlert, direct_flags.alert);
        w.set_bool(BoolProp::DirectMention, direct_flags.mention);
        w.set_bool(BoolProp::DirectHint, direct_flags.hint);
    }
    if last.is_none_or(|l| l.invites_pending != *invites_pending) {
        w.set_bool(BoolProp::InvitesPending, *invites_pending);
    }
    if last.is_none_or(|l| l.attention != *attention) {
        w.set_int(IntProp::Attention, saturating_i32(*attention));
    }
}

fn apply_listed_space(w: &impl UiProps, listed_space: &SpaceHeading) {
    w.set_text(StringProp::ListedSpaceName, &listed_space.name);
    w.set_int(
        IntProp::ListedSpaceMembers,
        saturating_i32(listed_space.member_count),
    );
    w.set_text(StringProp::ListedSpaceId, &listed_space.id);
    w.set_bool(BoolProp::ListedSpaceMayInvite, listed_space.may_invite);
    w.set_bool(
        BoolProp::ListedSpaceMayAddChildren,
        listed_space.may_add_children,
    );
}

pub(super) fn apply_space_index<B: UiBackend>(
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
        w.set_text(StringProp::SpaceIndexName, space_name);
    }
    if last.is_none_or(|l| l.pages_landed != *pages_landed) {
        w.set_int(IntProp::SpaceIndexPagesLanded, *pages_landed);
        run_change_handlers_next_frame(w);
    }
    let shown = last.map(|l| (&l.rows, l.avatars_ready));
    if let Some(previous) = rows_to_redo(shown, rows, *avatars_ready) {
        apply_rows(
            &ctx.models.space_children,
            rows.as_ref(),
            previous,
            ctx.media,
            &|row| space_child_to_dto(row, ctx.media).into(),
            &|entry| entry.id(),
        );
    }
}
