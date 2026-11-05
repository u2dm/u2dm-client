use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use super::{
    NO_ANCHOR, is_new_generation, next_prepend_token, readopt_timeline,
    run_change_handlers_next_frame, set_focus,
};
use crate::adapters::ui::backend::{
    UiBackend, UiEventContext, enrich_message, keep_shown_enrichment,
};
use crate::adapters::ui::dto::{QuotedMessage, preview_line, quoted_message};
use crate::adapters::ui::fields::{MemberRowFields, MessageFields};
use crate::adapters::ui::props::{BoolProp, IntProp, StringProp, UiProps};
use crate::adapters::ui::reconcile::{apply_rows, apply_timeline_patch, rows_to_redo};
use crate::adapters::ui::rows::patch_rows_by_id;
use crate::adapters::ui::session::{active_generation, from_snapshot, with_session};
use crate::adapters::ui::splice_model::SpliceModel;
use crate::commands::view::{
    AppViewState, CopiedLink, PaginationView, PinnedView, PresenceView, ReadersView, SourceState,
    TypingView,
};
use crate::domain::message::{MessagePreviewKind, TimelineMessage};
use crate::domain::presence::PresenceLine;
use crate::domain::room::RoomId;
use crate::domain::timeline::{OlderHistory, SourceEncryption, TimelinePatch, TimelineStatus};
use crate::util::saturating_i32;

pub(super) fn apply_timeline<B: UiBackend>(
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
    let pinned_ids = from_snapshot(|view| Arc::clone(&view.pinned.pinned_ids)).unwrap_or_default();
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

fn is_pinned(pinned_ids: &BTreeSet<String>, message: &TimelineMessage) -> bool {
    message
        .event_id
        .as_ref()
        .is_some_and(|event_id| pinned_ids.contains(event_id))
}

pub(super) fn apply_pin_marks<B: UiBackend>(
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

pub(super) fn apply_source(w: &impl UiProps, state: &SourceState) {
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

pub(super) fn apply_readers<B: UiBackend>(
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
        w.set_int(IntProp::ReadersCount, saturating_i32(*total));
    }
    if last.is_none_or(|l| l.has_more != *has_more) {
        w.set_bool(BoolProp::ReadersHasMore, *has_more);
    }
    if last.is_none_or(|l| l.pages_landed != *pages_landed) {
        w.set_int(IntProp::ReadersPagesLanded, *pages_landed);
        run_change_handlers_next_frame(w);
    }
    let shown = last.map(|l| (&l.rows, l.avatars_ready));
    if let Some(previous) = rows_to_redo(shown, rows, *avatars_ready) {
        apply_rows(
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

pub(super) fn apply_copied_links(
    w: &(impl UiProps + ComponentHandle),
    last: Option<&AppViewState>,
    message_link: &CopiedLink,
    room_link: &CopiedLink,
) {
    if last.is_none_or(|l| l.message_link != *message_link) {
        apply_copied_link(
            w,
            message_link,
            StringProp::MessageLink,
            IntProp::MessageLinkSerial,
        );
    }
    if last.is_none_or(|l| l.room_link != *room_link) {
        apply_copied_link(w, room_link, StringProp::RoomLink, IntProp::RoomLinkSerial);
    }
}

fn apply_copied_link(
    w: &(impl UiProps + ComponentHandle),
    link: &CopiedLink,
    url: StringProp,
    serial: IntProp,
) {
    w.set_string(url, SharedString::from(link.url.as_str()));
    w.set_int(serial, link.serial);
    run_change_handlers_next_frame(w);
}

pub(super) fn sync_timeline_chrome(
    w: &(impl UiProps + ComponentHandle),
    pagination: &PaginationView,
) {
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
    w.set_int(IntProp::NewMessagesCount, saturating_i32(badge));
    run_change_handlers_next_frame(w);
}

pub(super) fn apply_pinned(w: &impl UiProps, pinned: &PinnedView) {
    let selected = w.get_string(StringProp::SelectedRoomId);
    let shown = pinned.shown_in(selected.as_str());
    let (count, index) = match shown {
        Some(_) => (pinned.messages.len(), pinned.shown),
        None => (0, 0),
    };
    w.set_int(IntProp::PinnedCount, saturating_i32(count));
    w.set_int(IntProp::PinnedIndex, saturating_i32(index));
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

pub(super) fn apply_room_people(
    w: &impl UiProps,
    last: Option<&AppViewState>,
    typing: &TypingView,
    presence: &PresenceView,
) {
    apply_typing(w, last.map(|l| &l.typing), typing);
    apply_presence(w, last.map(|l| &l.presence), presence);
}

fn apply_presence(w: &impl UiProps, last: Option<&PresenceView>, view: &PresenceView) {
    if last == Some(view) {
        return;
    }
    let PresenceView { partner, person } = view;
    let partner_line = partner
        .as_ref()
        .map_or(PresenceLine::Hidden, |partner| partner.line);
    w.set_string(
        StringProp::PresenceRoomId,
        partner.as_ref().map_or_else(SharedString::new, |partner| {
            SharedString::from(partner.room_id.as_ref())
        }),
    );
    w.set_presence_partner(partner_line);
    w.set_int(IntProp::PresencePartnerMinutes, minutes_seen(partner_line));
    let person_line = person
        .as_ref()
        .map_or(PresenceLine::Hidden, |person| person.line);
    w.set_string(
        StringProp::PresenceUserId,
        person.as_ref().map_or_else(SharedString::new, |person| {
            SharedString::from(person.user_id.as_ref())
        }),
    );
    w.set_presence_person(person_line);
    w.set_int(IntProp::PresencePersonMinutes, minutes_seen(person_line));
    w.set_string(
        StringProp::PresenceStatus,
        SharedString::from(
            person
                .as_ref()
                .and_then(|person| person.status.as_deref())
                .unwrap_or_default(),
        ),
    );
}

fn minutes_seen(line: PresenceLine) -> i32 {
    match line {
        PresenceLine::LastSeen { minutes } => saturating_i32(minutes),
        PresenceLine::Hidden | PresenceLine::Online | PresenceLine::Away => 0,
    }
}

fn apply_typing(w: &impl UiProps, last: Option<&TypingView>, view: &TypingView) {
    if last == Some(view) {
        return;
    }
    let typists = view.typists.users();
    let shown = |at: usize| {
        typists
            .get(at)
            .map_or_else(SharedString::new, |user| SharedString::from(user.shown()))
    };
    w.set_string(
        StringProp::TypingRoomId,
        SharedString::from(view.room_id.as_deref().unwrap_or_default()),
    );
    w.set_int(IntProp::TypingCount, saturating_i32(typists.len()));
    w.set_string(StringProp::TypingFirst, shown(0));
    w.set_string(StringProp::TypingSecond, shown(1));
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

pub(super) fn is_active(w: &impl UiProps, room_id: &RoomId, generation: i32) -> bool {
    w.get_string(StringProp::SelectedRoomId).as_str() == room_id.as_ref()
        && active_generation() == generation
}

pub(super) fn apply_timeline_status(w: &impl UiProps, status: TimelineStatus) {
    w.set_bool(
        BoolProp::TimelineRetryable,
        matches!(status, TimelineStatus::Failed { retryable: true }),
    );
    w.set_timeline_state(status);
}
