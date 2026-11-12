use std::mem;
use std::sync::Arc;

use slint::{Model, SharedString, StyledText};

use super::{SheetAvatar, run_change_handlers_next_frame};
use crate::adapters::ui::backend::{UiBackend, UiEventContext};
use crate::adapters::ui::decode::{AvatarSlot, load_avatar_async};
use crate::adapters::ui::dto::{
    invite_avatar_path, invitee_to_dto, load_room_info_avatar, load_user_info_avatar,
    log_line_to_dto, member_row_to_dto, removal_avatar_path, removal_shape, rich_body,
    room_preview_avatar_path, user_device_to_dto, user_info_pronouns,
};
use crate::adapters::ui::fields::{InviteeFields, MemberRowFields};
use crate::adapters::ui::present::{
    avatar_color_index, avatar_initials, message_sent_at_label, name_initials,
};
use crate::adapters::ui::props::{BoolProp, ImageProp, IntProp, MessageProp, StringProp, UiProps};
use crate::adapters::ui::reconcile::{apply_rows, rows_to_redo};
use crate::adapters::ui::rows::row_index;
use crate::adapters::ui::session::{ActiveBackend, UiSession, with_session};
use crate::adapters::ui::splice_model::SpliceModel;
use crate::commands::view::{
    AppViewState, CardStatus, CreateRoomView, InvitePeopleView, InviteView, LeaveSpaceView,
    RemovalView, RoomCard, RoomInfoView, RoomLogView, RoomPreviewSheet, RoomPreviewView, RosterRow,
    ShownPreview, UserCard, UserInfoView,
};
use crate::domain::departure::{AdminStanding, Removal, RemovalList};
use crate::domain::invite::{Invite, InviteList};
use crate::domain::message::RichText;
use crate::domain::room_info::RoomAbout;
use crate::domain::room_log::RoomLog;
use crate::domain::room_preview::RoomPreview;
use crate::domain::user_info::Moderation;
use crate::ports::media::MediaCache;
use crate::util::saturating_i32;

pub(super) fn apply_room_log<B: UiBackend>(
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
        w.set_text(StringProp::RoomLogRoomId, next.room_id.as_ref());
        lines.set_vec(
            next.log
                .lines
                .iter()
                .map(|line| B::LogLine::from(log_line_to_dto(line)))
                .collect(),
        );
    }
    w.set_text(StringProp::RoomLogName, &next.name);
    w.set_int(IntProp::RoomLogDropped, saturating_i32(next.log.dropped));
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
        .map(|line| B::LogLine::from(log_line_to_dto(line)))
        .collect();
    lines.insert_rows(lines.row_count(), arrived);
}

pub(super) fn apply_invite(
    w: &impl UiProps,
    last_view: Option<&AppViewState>,
    view: &InviteView,
    listed: &InviteList,
    media: &dyn MediaCache,
) {
    let InviteView {
        invite,
        answer,
        error,
    } = view;
    let last = last_view.map(|l| &l.invite);

    if last.is_none_or(|l| l.answer != *answer) {
        w.set_invite_answer(*answer);
    }
    if last.is_none_or(|l| l.error != *error) {
        w.set_message(MessageProp::InviteError, error);
    }
    if last.is_none_or(|l| l.invite != *invite) {
        apply_invite_card(w, invite.as_deref());
    }
    if last_view.is_none_or(|l| l.invite != *view || !Arc::ptr_eq(&l.directory.invites, listed)) {
        show_invite_avatar(w, invite.as_deref(), media);
    }
}

fn apply_invite_card(w: &impl UiProps, invite: Option<&Invite>) {
    let Some(invite) = invite else {
        w.set_bool(BoolProp::InviteVisible, false);
        return;
    };
    w.set_text(StringProp::InviteRoomId, invite.id.as_ref());
    w.set_text(StringProp::InviteName, &invite.name);
    w.set_text(StringProp::InviteInitial, avatar_initials(&invite.name));
    w.set_int(
        IntProp::InviteColorIndex,
        avatar_color_index(invite.id.as_ref()),
    );
    w.set_invite_kind(invite.kind);
    w.set_text(
        StringProp::InviteTopic,
        invite.topic.as_deref().unwrap_or_default(),
    );
    w.set_text(
        StringProp::InviteInviter,
        invite
            .inviter
            .as_ref()
            .map(|inviter| inviter.name.identified(&inviter.user_id))
            .unwrap_or_default(),
    );
    w.set_bool(BoolProp::InviteVisible, true);
}

fn show_invite_avatar(w: &impl UiProps, invite: Option<&Invite>, media: &dyn MediaCache) {
    let next = SheetAvatar {
        room_id: invite
            .map(|invite| invite.id.to_string())
            .unwrap_or_default(),
        path: invite.and_then(|invite| invite_avatar_path(invite, media)),
    };
    show_sheet_avatar(
        w,
        &next,
        |session| &mut session.invite_avatar,
        AvatarSlot::Invite,
        ImageProp::InviteAvatar,
    );
}

pub(super) fn apply_removal(
    w: &impl UiProps,
    last_view: Option<&AppViewState>,
    view: &RemovalView,
    listed: &RemovalList,
    media: &dyn MediaCache,
) {
    let RemovalView {
        removal,
        forgetting,
        error,
    } = view;
    let last = last_view.map(|l| &l.removal);

    if last.is_none_or(|l| l.forgetting != *forgetting) {
        w.set_bool(BoolProp::RemovalForgetting, *forgetting);
    }
    if last.is_none_or(|l| l.error != *error) {
        w.set_message(MessageProp::RemovalError, error);
    }
    if last.is_none_or(|l| l.removal != *removal) {
        apply_removal_card(w, removal.as_deref());
    }
    if last_view.is_none_or(|l| l.removal != *view || !Arc::ptr_eq(&l.directory.removals, listed)) {
        show_removal_avatar(w, removal.as_deref(), media);
    }
}

fn apply_removal_card(w: &impl UiProps, removal: Option<&Removal>) {
    let Some(removal) = removal else {
        w.set_bool(BoolProp::RemovalVisible, false);
        return;
    };
    w.set_text(StringProp::RemovalRoomId, removal.id.as_ref());
    w.set_text(StringProp::RemovalName, &removal.name);
    w.set_text(StringProp::RemovalInitial, avatar_initials(&removal.name));
    w.set_int(
        IntProp::RemovalColorIndex,
        avatar_color_index(removal.id.as_ref()),
    );
    w.set_removal_kind(removal.kind);
    w.set_removal_shape(removal_shape(removal));
    w.set_text(StringProp::RemovalBy, removal.by.identified());
    w.set_text(
        StringProp::RemovalReason,
        removal.reason.as_deref().unwrap_or_default(),
    );
    w.set_bool(BoolProp::RemovalVisible, true);
}

fn show_removal_avatar(w: &impl UiProps, removal: Option<&Removal>, media: &dyn MediaCache) {
    let next = SheetAvatar {
        room_id: removal
            .map(|removal| removal.id.to_string())
            .unwrap_or_default(),
        path: removal.and_then(|removal| removal_avatar_path(removal, media)),
    };
    show_sheet_avatar(
        w,
        &next,
        |session| &mut session.removal_avatar,
        AvatarSlot::Removal,
        ImageProp::RemovalAvatar,
    );
}

pub(super) fn apply_leave_space(
    w: &impl UiProps,
    last: Option<&LeaveSpaceView>,
    view: &LeaveSpaceView,
) {
    if last == Some(view) {
        return;
    }
    let LeaveSpaceView {
        space,
        standing,
        leaving,
        error,
    } = view;
    w.set_bool(BoolProp::LeaveSpaceLeaving, *leaving);
    w.set_bool(
        BoolProp::LeaveSpaceOnlyAdmin,
        *standing == Some(AdminStanding::OnlyAdmin),
    );
    w.set_message(MessageProp::LeaveSpaceError, error);
    let Some(space) = space else {
        w.set_bool(BoolProp::LeaveSpaceVisible, false);
        return;
    };
    w.set_text(StringProp::LeaveSpaceId, space.id.as_ref());
    w.set_text(StringProp::LeaveSpaceName, &space.name);
    w.set_int(IntProp::LeaveSpaceRooms, saturating_i32(space.rooms));
    w.set_int(IntProp::LeaveSpaceSpaces, saturating_i32(space.spaces));
    w.set_bool(BoolProp::LeaveSpaceVisible, true);
}

pub(super) fn apply_create_room(
    w: &impl UiProps,
    last: Option<&CreateRoomView>,
    view: &CreateRoomView,
) {
    if last == Some(view) {
        return;
    }
    w.set_create_room_kind(view.kind);
    w.set_text(StringProp::CreateRoomParentName, &view.parent_name);
    w.set_text(StringProp::CreateRoomServerName, &view.server_name);
    w.set_message(MessageProp::CreateRoomError, &view.error);
    w.set_create_room_status(view.status);
}

pub(super) fn apply_invite_people<B: UiBackend>(
    w: &B::Window,
    last: Option<&InvitePeopleView>,
    view: &InvitePeopleView,
    ctx: &UiEventContext<'_, B>,
) {
    let InvitePeopleView {
        target,
        search,
        results,
        picked,
        limited,
        sending,
        error,
        avatars_ready,
    } = view;
    if last.is_none_or(|l| l.target != *target) {
        let target = target.as_ref();
        w.set_text(
            StringProp::InvitePeopleRoomId,
            target.map_or("", |target| target.room_id.as_ref()),
        );
        w.set_text(
            StringProp::InvitePeopleRoomName,
            target.map_or("", |target| target.name.as_str()),
        );
        w.set_bool(
            BoolProp::InvitePeopleIsSpace,
            target.is_some_and(|target| target.space),
        );
    }
    if last.is_none_or(|l| l.search != *search) {
        w.set_people_search_status(*search);
    }
    if last.is_none_or(|l| l.limited != *limited) {
        w.set_bool(BoolProp::InvitePeopleLimited, *limited);
    }
    if last.is_none_or(|l| l.sending != *sending) {
        w.set_bool(BoolProp::InvitePeopleSending, *sending);
    }
    if last.is_none_or(|l| l.error != *error) {
        w.set_message(MessageProp::InvitePeopleError, error);
    }
    let avatars_landed = last.is_some_and(|l| l.avatars_ready != *avatars_ready);
    for (model, rows, previous) in [
        (
            &ctx.models.invitee_results,
            results,
            last.map(|l| &l.results),
        ),
        (&ctx.models.invitee_picks, picked, last.map(|l| &l.picked)),
    ] {
        if avatars_landed || previous != Some(rows) {
            apply_rows(
                model,
                rows,
                previous
                    .filter(|_| !avatars_landed)
                    .map_or(&[] as &[_], |rows| rows.as_ref()),
                ctx.media,
                &|row| invitee_to_dto(row, ctx.media).into(),
                &|entry| entry.user_id(),
            );
        }
    }
    if last.is_none_or(|l| l.target.is_some() != target.is_some()) {
        w.set_bool(BoolProp::InvitePeopleVisible, target.is_some());
    }
}

pub(super) fn apply_room_preview(
    w: &impl UiProps,
    last: Option<&RoomPreviewView>,
    view: &RoomPreviewView,
    media: &dyn MediaCache,
) {
    if last == Some(view) {
        return;
    }
    let RoomPreviewView {
        sheet,
        address,
        entry,
        access,
        error,
        avatars_ready: _,
    } = view;
    let shown = match sheet {
        RoomPreviewSheet::Loaded(shown) => Some(shown),
        RoomPreviewSheet::Closed
        | RoomPreviewSheet::Idle
        | RoomPreviewSheet::Loading
        | RoomPreviewSheet::Missing
        | RoomPreviewSheet::Unavailable
        | RoomPreviewSheet::Failed => None,
    };
    if let Some(shown) = shown {
        apply_room_preview_card(w, shown);
    }
    show_room_preview_avatar(w, shown.map(|shown| &*shown.room), media);
    w.set_text(StringProp::RoomPreviewAddress, address);
    w.set_bool(BoolProp::RoomPreviewEntry, *entry);
    w.set_room_preview_access(*access);
    w.set_message(MessageProp::RoomPreviewError, error);
    w.set_room_preview_status(sheet);
}

fn apply_room_preview_card(w: &impl UiProps, shown: &ShownPreview) {
    let room = &shown.room;
    w.set_text(StringProp::RoomPreviewRoomId, room.id.as_ref());
    w.set_text(StringProp::RoomPreviewName, &room.name);
    w.set_text(StringProp::RoomPreviewInitial, avatar_initials(&room.name));
    w.set_int(
        IntProp::RoomPreviewColorIndex,
        avatar_color_index(room.id.as_ref()),
    );
    w.set_text(
        StringProp::RoomPreviewAlias,
        room.alias.as_deref().unwrap_or_default(),
    );
    w.set_text(
        StringProp::RoomPreviewTopic,
        room.topic.as_deref().unwrap_or_default(),
    );
    w.set_int(
        IntProp::RoomPreviewMembers,
        saturating_i32(room.member_count),
    );
    w.set_bool(BoolProp::RoomPreviewIsSpace, room.is_space);
    w.set_bool(BoolProp::RoomPreviewOpensEvent, shown.opens_event);
}

fn show_room_preview_avatar(w: &impl UiProps, room: Option<&RoomPreview>, media: &dyn MediaCache) {
    let next = SheetAvatar {
        room_id: room.map(|room| room.id.to_string()).unwrap_or_default(),
        path: room.and_then(|room| room_preview_avatar_path(room, media)),
    };
    show_sheet_avatar(
        w,
        &next,
        |session| &mut session.room_preview_avatar,
        AvatarSlot::RoomPreview,
        ImageProp::RoomPreviewAvatar,
    );
}

fn show_sheet_avatar(
    w: &impl UiProps,
    next: &SheetAvatar,
    shown: impl FnOnce(&mut UiSession<ActiveBackend>) -> &mut SheetAvatar,
    slot: AvatarSlot,
    prop: ImageProp,
) {
    let previous = with_session(|session| mem::replace(shown(session), next.clone()));
    if previous == *next {
        return;
    }
    let image = load_avatar_async(next.path.as_deref(), slot);
    if image.is_some() || previous.room_id != next.room_id {
        w.set_image(prop, image);
    }
}

pub(super) fn apply_joined_on(w: &impl UiProps, about: Option<&RoomAbout>) {
    let joined_on = about
        .and_then(|about| about.joined_at)
        .map(message_sent_at_label)
        .unwrap_or_default();
    w.set_text(StringProp::RoomInfoJoinedOn, joined_on);
}

pub(super) fn apply_room_info<B: UiBackend>(
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
        apply_joined_on(w, about.as_ref());
        let link = about.as_ref().map_or("", |about| about.link.as_str());
        w.set_text(StringProp::RoomInfoLink, link);
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
        w.set_message(MessageProp::RoomInfoError, error);
    }
    if last.is_none_or(|l| l.pages_landed != *pages_landed) {
        w.set_int(IntProp::RoomInfoPagesLanded, *pages_landed);
        run_change_handlers_next_frame(w);
    }
    let shown = last.map(|l| (&l.rows, l.avatars_ready));
    if let Some(previous) = rows_to_redo(shown, rows, *avatars_ready) {
        apply_rows(
            &ctx.models.room_members,
            rows.as_ref(),
            previous,
            ctx.media,
            &|row| member_row_to_dto(row, ctx.media).into(),
            &|entry| entry.user_id(),
        );
        let invited = rows
            .iter()
            .position(|row| matches!(row, RosterRow::InvitedHeading));
        w.set_int(IntProp::RoomInfoInvitedRow, row_index(invited));
    }
}

fn apply_room_card(w: &impl UiProps, card: Option<&RoomCard>, media: &dyn MediaCache) {
    let Some(card) = card else {
        w.set_image(ImageProp::RoomInfoAvatar, None);
        return;
    };
    w.set_text(StringProp::RoomInfoRoomId, card.id.as_ref());
    w.set_text(StringProp::RoomInfoName, &card.name);
    w.set_text(StringProp::RoomInfoInitial, avatar_initials(&card.name));
    w.set_int(IntProp::RoomInfoColorIndex, avatar_color_index(&card.id));
    w.set_int(IntProp::RoomInfoMembers, saturating_i32(card.member_count));
    w.set_bool(BoolProp::RoomInfoIsDirect, card.is_direct);
    w.set_bool(BoolProp::RoomInfoMayInvite, card.may_invite);
    w.set_bool(BoolProp::RoomInfoReadableByAnyone, card.readable_by_anyone);
    w.set_text(
        StringProp::RoomInfoAlias,
        card.alias.as_deref().unwrap_or_default(),
    );
    w.set_image(
        ImageProp::RoomInfoAvatar,
        load_room_info_avatar(card, media),
    );
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

pub(super) fn apply_user_info<B: UiBackend>(
    w: &B::Window,
    last: Option<&UserInfoView>,
    user_info: &UserInfoView,
    ctx: &UiEventContext<'_, B>,
) {
    let media = ctx.media;
    let UserInfoView {
        card,
        direct,
        verify,
        sessions,
        ignore_busy,
        moderating,
        avatars_ready,
        error,
    } = user_info;

    if last.is_none_or(|l| l.direct != *direct) {
        w.set_user_info_direct(*direct);
    }
    if last.is_none_or(|l| l.verify != *verify) {
        w.set_user_info_verify(*verify);
    }
    if last.is_none_or(|l| l.sessions != *sessions) {
        w.set_bool(BoolProp::UserInfoSessionsKnown, sessions.is_some());
        let rows = sessions
            .iter()
            .flat_map(|devices| devices.iter())
            .map(user_device_to_dto)
            .map(B::UserDevice::from)
            .collect::<Vec<_>>();
        ctx.models.user_devices.set_vec(rows);
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
        w.set_image(ImageProp::UserInfoAvatar, avatar);
    }
    if last.is_none_or(|l| l.error != *error) {
        w.set_message(MessageProp::UserInfoError, error);
    }
}

fn apply_user_card(w: &impl UiProps, card: Option<&UserCard>, media: &dyn MediaCache) {
    let Some(card) = card else {
        w.set_bool(BoolProp::UserInfoVisible, false);
        w.set_image(ImageProp::UserInfoAvatar, None);
        return;
    };
    let profile = &card.profile;
    w.set_text(StringProp::UserInfoUserId, profile.user_id.as_ref());
    w.set_text(StringProp::UserInfoName, profile.shown());
    w.set_text(
        StringProp::UserInfoInitial,
        name_initials(&profile.name, &profile.user_id),
    );
    w.set_int(
        IntProp::UserInfoColorIndex,
        avatar_color_index(&profile.user_id),
    );
    w.set_string(StringProp::UserInfoPronouns, user_info_pronouns(profile));
    w.set_text(StringProp::UserInfoLink, &profile.link);
    w.set_user_info_role(profile.role);
    w.set_user_info_membership(profile.membership);
    w.set_user_info_identity(profile.trust);
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
    w.set_image(
        ImageProp::UserInfoAvatar,
        load_user_info_avatar(profile, media),
    );
    w.set_bool(BoolProp::UserInfoVisible, true);
}
