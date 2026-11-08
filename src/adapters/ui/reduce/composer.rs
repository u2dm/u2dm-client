use slint::SharedString;

use crate::adapters::ui::backend::{UiBackend, UiEventContext};
use crate::adapters::ui::decode::load_attachment_preview;
use crate::adapters::ui::dto::{mention_to_dto, preview_line};
use crate::adapters::ui::emote;
use crate::adapters::ui::fields::MemberRowFields;
use crate::adapters::ui::present::{duration_label, identity_banner};
use crate::adapters::ui::props::{BoolProp, ImageProp, IntProp, StringProp, UiProps};
use crate::adapters::ui::reconcile::{apply_rows, rows_to_redo};
use crate::commands::ui::Draft;
use crate::commands::view::{
    AppViewState, AttachmentView, IdentityAlertView, MentionsView, UnsentMessage, UpgradeView,
};
use crate::domain::media::file_extension;
use crate::domain::message::{EditKind, MessageEdit, RichText};
use crate::domain::room::RoomId;
use crate::util::format_bytes;

pub(super) fn apply_mentions<B: UiBackend>(
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
        w.set_text(
            StringProp::MentionsRoomId,
            room_id.as_ref().map_or("", |room| room.as_ref()),
        );
    }
    if last.is_none_or(|l| l.offers_room != *offers_room) {
        w.set_bool(BoolProp::MentionsOfferRoom, *offers_room);
    }
    let shown = last.map(|l| (&l.rows, l.avatars_ready));
    if let Some(previous) = rows_to_redo(shown, rows, *avatars_ready) {
        apply_rows(
            &ctx.models.mentions,
            rows.as_ref(),
            previous,
            ctx.media,
            &|member| mention_to_dto(member, ctx.media).into(),
            &|entry| entry.user_id(),
        );
    }
}

pub(super) fn apply_attachment(
    w: &impl UiProps,
    last: Option<&AttachmentView>,
    attachment: &AttachmentView,
) {
    if last == Some(attachment) {
        return;
    }
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
    w.set_text(StringProp::AttachmentFilename, filename);
    w.set_text(StringProp::AttachmentMimetype, mimetype);
    w.set_text(
        StringProp::AttachmentExtension,
        file_extension(filename).to_uppercase(),
    );
    w.set_text(
        StringProp::AttachmentSize,
        if *visible {
            format_bytes(*size)
        } else {
            String::new()
        },
    );
    w.set_text(
        StringProp::AttachmentDuration,
        duration.map(duration_label).unwrap_or_default(),
    );
    w.set_int(IntProp::AttachmentWidth, (*width).cast_signed());
    w.set_int(IntProp::AttachmentHeight, (*height).cast_signed());
    w.set_attachment_error(*error);
    w.set_text(StringProp::AttachmentErrorDetail, error_detail);
    let preview = load_attachment_preview(*pick, preview_path.as_deref());
    w.set_image(ImageProp::AttachmentPreview, preview);
}

pub(super) fn apply_unsent(w: &impl UiProps, unsent: Option<&UnsentMessage>) {
    if let Some(unsent) = unsent {
        emote::rebind(unsent.room_id.as_ref(), unsent.draft.emotes());
    }
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
    w.set_int(
        IntProp::UnsentShortenBy,
        unsent
            .and_then(|unsent| unsent.oversize)
            .map_or(0, |oversize| i32::from(oversize.shorten_by())),
    );
    w.set_text(
        StringProp::UnsentRoomId,
        unsent.map_or("", |unsent| unsent.room_id.as_ref()),
    );
    w.set_text(StringProp::UnsentBody, draft.map_or("", Draft::body));
    w.set_text(
        StringProp::UnsentReplyEventId,
        reply.map_or("", |reply| reply.event_id.as_str()),
    );
    w.set_text(
        StringProp::UnsentReplySender,
        reply.map_or("", |reply| reply.sender.as_str()),
    );
    w.set_text(
        StringProp::UnsentReplyPreview,
        reply.map_or("", |reply| reply.preview.as_str()),
    );
    apply_unsent_edit(w, edit);
}

pub(super) fn apply_composer_banners(
    w: &impl UiProps,
    last: Option<&AppViewState>,
    identity_alert: &IdentityAlertView,
    upgrade: &UpgradeView,
) {
    if last.is_none_or(|l| l.identity_alert != *identity_alert) {
        apply_identity_alert(w, identity_alert);
    }
    if last.is_none_or(|l| l.upgrade != *upgrade) {
        apply_upgrade(w, upgrade);
    }
}

fn apply_identity_alert(w: &impl UiProps, view: &IdentityAlertView) {
    let banner = identity_banner(view.alert.as_ref());
    w.set_identity_alert_kind(banner.kind);
    w.set_text(StringProp::IdentityAlertUserId, banner.user_id);
    w.set_text(StringProp::IdentityAlertName, banner.name);
    w.set_bool(BoolProp::IdentityAlertBusy, view.busy);
}

fn apply_upgrade(w: &impl UiProps, view: &UpgradeView) {
    let id = |room_id: Option<&RoomId>| SharedString::from(room_id.map_or("", |id| id.as_ref()));
    w.set_string(StringProp::UpgradeRoomId, id(view.room_id.as_ref()));
    w.set_upgrade_successor(&view.successor);
    w.set_string(StringProp::UpgradeSuccessorId, id(view.successor.room_id()));
    w.set_upgrade_predecessor(&view.predecessor);
    w.set_string(
        StringProp::UpgradePredecessorId,
        id(view.predecessor.room_id()),
    );
}

fn apply_unsent_edit(w: &impl UiProps, edit: Option<&MessageEdit>) {
    w.set_bool(BoolProp::UnsentIsEdit, edit.is_some());
    w.set_bool(
        BoolProp::UnsentEditCaption,
        edit.is_some_and(|edit| edit.kind == EditKind::Caption),
    );
    w.set_text(
        StringProp::UnsentEditEventId,
        edit.and_then(|edit| edit.target.event_id())
            .unwrap_or_default(),
    );
    w.set_text(
        StringProp::UnsentEditLocalId,
        edit.and_then(|edit| edit.target.local_id())
            .unwrap_or_default(),
    );
    w.set_string(
        StringProp::UnsentEditPreview,
        edit.and_then(|edit| edit.original.clone())
            .map(|original| preview_line(&RichText::plain(original)))
            .unwrap_or_default(),
    );
}
