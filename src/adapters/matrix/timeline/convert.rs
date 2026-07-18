use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use matrix_sdk::ruma::events::StateEventContentChange;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::{
    FileMessageEventContent, ImageMessageEventContent, MessageType,
};
use matrix_sdk::ruma::events::room::name::RoomNameEventContent;
use matrix_sdk_ui::timeline::{
    AnyOtherStateEventContentChange, EventTimelineItem, MemberProfileChange, MembershipChange,
    RoomMembershipChange, TimelineDetails, TimelineItem, TimelineItemContent,
};

use super::TimelineContext;
use crate::adapters::matrix::preview;
use crate::domain::models::{
    EventId, FileMeta, ImageMeta, MessageBody, MessagePreviewKind, ReplyInfo, ServiceEvent,
    TimelineMessage,
};

fn extract_sender_profile(event: &EventTimelineItem) -> (Option<String>, Option<String>) {
    match event.sender_profile() {
        TimelineDetails::Ready(profile) => (
            profile.display_name.clone(),
            profile.avatar_url.as_ref().map(ToString::to_string),
        ),
        _ => (None, None),
    }
}

fn event_id_from_str(event_id_str: String) -> Option<EventId> {
    (!event_id_str.is_empty()).then_some(EventId(event_id_str))
}

fn build_utd_message(
    unique_id: String,
    event: &EventTimelineItem,
    event_id_str: String,
    ctx: &TimelineContext<'_>,
    reply: Option<ReplyInfo>,
) -> TimelineMessage {
    let (sender_display_name, sender_avatar_url) = extract_sender_profile(event);
    let ts: u64 = event.timestamp().0.into();
    let sender_str = event.sender().to_string();
    let is_own = ctx.own_user_id.is_some_and(|uid| uid == sender_str);
    TimelineMessage {
        unique_id,
        event_id: event_id_from_str(event_id_str),
        sender_pronouns: ctx.pronouns.resolved(&sender_str),
        sender: sender_str,
        sender_display_name,
        sender_avatar_url,
        body: MessageBody::UnableToDecrypt,
        timestamp: ts,
        is_own,
        reply,
        edited: false,
    }
}

fn build_service_message(
    unique_id: String,
    event: &EventTimelineItem,
    event_id_str: String,
    ctx: &TimelineContext<'_>,
    service: ServiceEvent,
) -> TimelineMessage {
    let (sender_display_name, sender_avatar_url) = extract_sender_profile(event);
    let ts: u64 = event.timestamp().0.into();
    let sender_str = event.sender().to_string();
    let is_own = ctx.own_user_id.is_some_and(|uid| uid == sender_str);
    TimelineMessage {
        unique_id,
        event_id: event_id_from_str(event_id_str),
        sender_pronouns: Vec::new(),
        sender: sender_str,
        sender_display_name,
        sender_avatar_url,
        body: MessageBody::Service(service),
        timestamp: ts,
        is_own,
        reply: None,
        edited: false,
    }
}

fn membership_target(change: &RoomMembershipChange) -> Option<String> {
    change.display_name().or_else(|| {
        let user_id = change.user_id();
        let local = user_id.localpart();
        (!local.is_empty()).then(|| local.to_owned())
    })
}

fn membership_to_service(change: &RoomMembershipChange) -> Option<ServiceEvent> {
    let target = membership_target(change);
    Some(match change.change()? {
        MembershipChange::Joined => ServiceEvent::Joined,
        MembershipChange::Left => ServiceEvent::Left,
        MembershipChange::Invited => ServiceEvent::Invited { target },
        MembershipChange::InvitationAccepted => ServiceEvent::InvitationAccepted,
        MembershipChange::InvitationRejected => ServiceEvent::InvitationRejected,
        MembershipChange::InvitationRevoked => ServiceEvent::InvitationRevoked { target },
        MembershipChange::Kicked => ServiceEvent::Kicked { target },
        MembershipChange::Banned | MembershipChange::KickedAndBanned => {
            ServiceEvent::Banned { target }
        }
        MembershipChange::Unbanned => ServiceEvent::Unbanned { target },
        MembershipChange::Knocked => ServiceEvent::Knocked,
        MembershipChange::KnockAccepted => ServiceEvent::KnockAccepted { target },
        MembershipChange::None
        | MembershipChange::Error
        | MembershipChange::KnockRetracted
        | MembershipChange::KnockDenied
        | MembershipChange::NotImplemented => return None,
    })
}

fn profile_to_service(change: &MemberProfileChange) -> Option<ServiceEvent> {
    if let Some(name_change) = change.displayname_change() {
        return Some(match (&name_change.old, &name_change.new) {
            (_, Some(new)) if name_change.old.is_some() => {
                ServiceEvent::DisplayNameChanged { name: new.clone() }
            }
            (_, Some(new)) => ServiceEvent::DisplayNameSet { name: new.clone() },
            (Some(_), None) => ServiceEvent::DisplayNameRemoved,
            (None, None) => return None,
        });
    }
    if change.avatar_url_change().is_some() {
        return Some(ServiceEvent::AvatarChanged);
    }
    None
}

fn room_name_from_change(change: &StateEventContentChange<RoomNameEventContent>) -> String {
    match change {
        StateEventContentChange::Original { content, .. } => content.name.clone(),
        StateEventContentChange::Redacted(_) => String::new(),
    }
}

fn other_state_to_service(state: &AnyOtherStateEventContentChange) -> Option<ServiceEvent> {
    Some(match state {
        AnyOtherStateEventContentChange::RoomName(change) => ServiceEvent::RoomNameChanged {
            name: room_name_from_change(change),
        },
        AnyOtherStateEventContentChange::RoomTopic(_) => ServiceEvent::RoomTopicChanged,
        AnyOtherStateEventContentChange::RoomAvatar(_) => ServiceEvent::RoomAvatarChanged,
        AnyOtherStateEventContentChange::RoomCreate(_) => ServiceEvent::RoomCreated,
        AnyOtherStateEventContentChange::RoomEncryption(_) => ServiceEvent::EncryptionEnabled,
        _ => return None,
    })
}

pub(super) fn service_event_from_content(content: &TimelineItemContent) -> Option<ServiceEvent> {
    match content {
        TimelineItemContent::MembershipChange(change) => membership_to_service(change),
        TimelineItemContent::ProfileChange(change) => profile_to_service(change),
        TimelineItemContent::OtherState(state) => other_state_to_service(state.content()),
        TimelineItemContent::CallInvite => Some(ServiceEvent::CallStarted),
        TimelineItemContent::RtcNotification { .. } => Some(ServiceEvent::CallNotification),
        _ => None,
    }
}

fn reply_preview_from_content(content: &TimelineItemContent) -> (MessagePreviewKind, String) {
    if let Some(message) = content.as_message() {
        let preview = preview::from_msgtype(message.msgtype());
        return (preview.kind, preview.body);
    }
    if content.as_unable_to_decrypt().is_some() {
        return (MessagePreviewKind::Encrypted, String::new());
    }
    (MessagePreviewKind::None, String::new())
}

fn extract_reply(content: &TimelineItemContent) -> Option<ReplyInfo> {
    let details = content.in_reply_to()?;
    let TimelineDetails::Ready(embedded) = details.event else {
        return None;
    };
    let sender = match &embedded.sender_profile {
        TimelineDetails::Ready(profile) => profile
            .display_name
            .clone()
            .unwrap_or_else(|| embedded.sender.to_string()),
        _ => embedded.sender.to_string(),
    };
    let (kind, body) = reply_preview_from_content(&embedded.content);
    Some(ReplyInfo { sender, kind, body })
}

fn extract_image_body(
    image: &ImageMessageEventContent,
    event_id_str: &str,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
) -> MessageBody {
    if let Ok(mut sources) = media_sources.lock() {
        sources.insert(event_id_str.to_owned(), image.source.clone());
        if let Some(info) = &image.info
            && let Some(ref thumb_source) = info.thumbnail_source
        {
            sources.insert(format!("{event_id_str}:thumb"), thumb_source.clone());
        }
    }
    #[allow(clippy::cast_possible_truncation)]
    let (width, height, mimetype) = image.info.as_ref().map_or((None, None, None), |info| {
        let w = info.width.map(|v| {
            let n: u64 = v.into();
            n as u32
        });
        let h = info.height.map(|v| {
            let n: u64 = v.into();
            n as u32
        });
        (w, h, info.mimetype.clone())
    });
    MessageBody::Image {
        caption: image.caption().map(String::from),
        meta: ImageMeta {
            width,
            height,
            mimetype,
        },
    }
}

fn extract_file_body(
    file: &FileMessageEventContent,
    event_id_str: &str,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
) -> MessageBody {
    if let Ok(mut sources) = media_sources.lock() {
        sources.insert(event_id_str.to_owned(), file.source.clone());
    }
    let (mimetype, size) = file.info.as_ref().map_or((None, None), |info| {
        (info.mimetype.clone(), info.size.map(Into::into))
    });
    MessageBody::File {
        meta: FileMeta {
            filename: file.filename.clone().unwrap_or_else(|| file.body.clone()),
            mimetype,
            size,
        },
    }
}

fn message_type_to_body(
    msgtype: &MessageType,
    event_id_str: &str,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
) -> MessageBody {
    match msgtype {
        MessageType::Text(t) => MessageBody::Text(t.body.clone()),
        MessageType::Notice(n) => MessageBody::Notice(n.body.clone()),
        MessageType::Emote(e) => MessageBody::Emote(e.body.clone()),
        MessageType::Image(i) => extract_image_body(i, event_id_str, media_sources),
        MessageType::File(f) => extract_file_body(f, event_id_str, media_sources),
        other => MessageBody::Unsupported {
            kind: other.msgtype().to_string(),
            fallback: other.body().to_string(),
        },
    }
}

pub(super) fn convert_timeline_item(
    item: &TimelineItem,
    ctx: &TimelineContext<'_>,
) -> Option<TimelineMessage> {
    let event = item.as_event()?;
    let unique_id = item.unique_id().0.clone();
    convert_event_item_with_uid(unique_id, event, ctx)
}

pub(super) fn convert_event_item_with_uid(
    unique_id: String,
    event: &EventTimelineItem,
    ctx: &TimelineContext<'_>,
) -> Option<TimelineMessage> {
    let event_id_str = event
        .event_id()
        .map(ToString::to_string)
        .unwrap_or_default();

    let content = event.content();
    let reply = extract_reply(content);

    let Some(message) = content.as_message() else {
        if content.as_unable_to_decrypt().is_some() {
            return Some(build_utd_message(
                unique_id,
                event,
                event_id_str,
                ctx,
                reply,
            ));
        }
        if let Some(service) = service_event_from_content(content) {
            return Some(build_service_message(
                unique_id,
                event,
                event_id_str,
                ctx,
                service,
            ));
        }
        tracing::debug!(
            event_id = event_id_str,
            sender = %event.sender(),
            "skipping non-message event"
        );
        return None;
    };

    let body = message_type_to_body(message.msgtype(), &event_id_str, ctx.media_sources);
    let (sender_display_name, sender_avatar_url) = extract_sender_profile(event);
    let ts: u64 = event.timestamp().0.into();
    let sender_str = event.sender().to_string();
    let is_own = ctx.own_user_id.is_some_and(|uid| uid == sender_str);

    Some(TimelineMessage {
        unique_id,
        event_id: event_id_from_str(event_id_str),
        sender_pronouns: ctx.pronouns.resolved(&sender_str),
        sender: sender_str,
        sender_display_name,
        sender_avatar_url,
        body,
        timestamp: ts,
        is_own,
        reply,
        edited: message.is_edited(),
    })
}
