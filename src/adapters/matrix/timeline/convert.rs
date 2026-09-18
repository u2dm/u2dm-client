use std::collections::HashSet;

use matrix_sdk::ruma::UInt;
use matrix_sdk::ruma::events::StateEventContentChange;
use matrix_sdk::ruma::events::room::ImageInfo;
use matrix_sdk::ruma::events::room::message::{
    AudioMessageEventContent, FileMessageEventContent, FormattedBody, ImageMessageEventContent,
    MessageFormat, MessageType, UnstableAmplitude, VideoInfo, VideoMessageEventContent,
};
use matrix_sdk::ruma::events::room::name::RoomNameEventContent;
use matrix_sdk::ruma::events::sticker::StickerEventContent;
use matrix_sdk_ui::timeline::{
    AnyOtherStateEventContentChange, EventSendState, EventTimelineItem, MemberProfileChange,
    MembershipChange, Message, ReactionInfo, RoomMembershipChange, Sticker, TimelineDetails,
    TimelineItem, TimelineItemContent,
};

use super::TimelineContext;
use crate::adapters::matrix::media::EventMedia;
use crate::adapters::matrix::preview;
use crate::domain::media::{AudioKind, AudioMeta, FileMeta, ImageMeta, VideoMeta, Waveform};
use crate::domain::message::{
    MessageBody, MessagePreviewKind, REACTOR_AVATAR_LIMIT, Reaction, ReactionSend, Reactor,
    ReplyInfo, RichText, SendState, ServiceEvent, TimelineMessage,
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

fn event_id_from_str(event_id_str: String) -> Option<String> {
    (!event_id_str.is_empty()).then_some(event_id_str)
}

pub(super) const LOCAL_ID_PREFIX: &str = "local:";

fn send_state(event: &EventTimelineItem) -> SendState {
    match event.send_state() {
        None | Some(EventSendState::Sent { .. }) => SendState::Sent,
        Some(EventSendState::SendingFailed { .. }) => SendState::Failed,
        Some(EventSendState::NotSentYet { progress: None }) => SendState::Sending,
        Some(EventSendState::NotSentYet {
            progress: Some(upload),
        }) => SendState::Uploading {
            sent: upload.progress.current as u64,
            total: upload.progress.total as u64,
        },
    }
}

fn local_id(event: &EventTimelineItem) -> Option<String> {
    let txn = event.transaction_id()?;
    Some(format!("{LOCAL_ID_PREFIX}{txn}"))
}

fn reaction_send(info: Option<&ReactionInfo>) -> ReactionSend {
    match info.and_then(|info| info.send_state.as_ref()) {
        None | Some(EventSendState::Sent { .. }) => ReactionSend::Sent,
        Some(EventSendState::SendingFailed { .. }) => ReactionSend::Failed,
        Some(EventSendState::NotSentYet { .. }) => ReactionSend::Sending,
    }
}

fn extract_reactions(content: &TimelineItemContent, ctx: &TimelineContext<'_>) -> Vec<Reaction> {
    let Some(by_key) = content.reactions() else {
        return Vec::new();
    };
    by_key
        .iter()
        .map(|(key, by_sender)| {
            let own = ctx.own_user_id.and_then(|own| {
                by_sender
                    .iter()
                    .find(|(user_id, _)| user_id.as_str() == own)
                    .map(|(_, info)| info)
            });
            let mut reaction = Reaction {
                key: key.clone(),
                senders: by_sender
                    .keys()
                    .map(|user_id| Reactor::new(user_id.to_string()))
                    .collect(),
                mine: own.is_some(),
                send: reaction_send(own),
            };
            if reaction.shows_reactors() {
                attach_reactor_avatars(&mut reaction, ctx);
            }
            reaction
        })
        .collect()
}

fn attach_reactor_avatars(reaction: &mut Reaction, ctx: &TimelineContext<'_>) {
    for reactor in &mut reaction.senders {
        reactor.avatar_url = ctx.reactor_avatars.avatar(&reactor.user_id);
        if reactor.avatar_url.is_none() {
            ctx.reactor_avatars.want(&reactor.user_id);
        }
    }
}

pub(super) fn reacted_by(item: &TimelineItem, users: &HashSet<String>) -> bool {
    let Some(by_key) = item
        .as_event()
        .and_then(|event| event.content().reactions())
    else {
        return false;
    };
    by_key.iter().any(|(_, by_sender)| {
        by_sender.len() < REACTOR_AVATAR_LIMIT
            && by_sender
                .keys()
                .any(|user_id| users.contains(user_id.as_str()))
    })
}

fn base_message(
    unique_id: String,
    event: &EventTimelineItem,
    event_id_str: String,
    ctx: &TimelineContext<'_>,
) -> TimelineMessage {
    let (sender_display_name, sender_avatar_url) = extract_sender_profile(event);
    let ts: u64 = event.timestamp().0.into();
    let sender_str = event.sender().to_string();
    let is_own = ctx.own_user_id.is_some_and(|uid| uid == sender_str);
    let is_first_unread = ctx.first_unread.is_some_and(|id| id == event_id_str);
    TimelineMessage {
        unique_id,
        event_id: event_id_from_str(event_id_str),
        local_id: local_id(event),
        sender_pronouns: ctx.pronouns.resolved(&sender_str),
        sender: sender_str,
        sender_display_name,
        sender_avatar_url,
        body: MessageBody::UnableToDecrypt,
        timestamp: ts,
        is_own,
        reply: None,
        edited: false,
        is_first_unread,
        send_state: send_state(event),
        reactions: extract_reactions(event.content(), ctx),
    }
}

fn build_utd_message(
    unique_id: String,
    event: &EventTimelineItem,
    event_id_str: String,
    ctx: &TimelineContext<'_>,
    reply: Option<ReplyInfo>,
) -> TimelineMessage {
    TimelineMessage {
        reply,
        ..base_message(unique_id, event, event_id_str, ctx)
    }
}

fn build_service_message(
    unique_id: String,
    event: &EventTimelineItem,
    event_id_str: String,
    ctx: &TimelineContext<'_>,
    service: ServiceEvent,
) -> TimelineMessage {
    TimelineMessage {
        sender_pronouns: Vec::new(),
        body: MessageBody::Service(service),
        ..base_message(unique_id, event, event_id_str, ctx)
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

fn service_event_from_content(content: &TimelineItemContent) -> Option<ServiceEvent> {
    match content {
        TimelineItemContent::MembershipChange(change) => membership_to_service(change),
        TimelineItemContent::ProfileChange(change) => profile_to_service(change),
        TimelineItemContent::OtherState(state) => other_state_to_service(state.content()),
        TimelineItemContent::CallInvite => Some(ServiceEvent::CallStarted),
        TimelineItemContent::RtcNotification { .. } => Some(ServiceEvent::CallNotification),
        _ => None,
    }
}

pub(super) enum Renderable<'a> {
    Message(&'a Message),
    Sticker(&'a Sticker),
    Utd,
    Service(ServiceEvent),
}

pub(super) fn classify(content: &TimelineItemContent) -> Option<Renderable<'_>> {
    if let Some(message) = content.as_message() {
        return Some(Renderable::Message(message));
    }
    if let Some(sticker) = content.as_sticker() {
        return Some(Renderable::Sticker(sticker));
    }
    if content.as_unable_to_decrypt().is_some() {
        return Some(Renderable::Utd);
    }
    service_event_from_content(content).map(Renderable::Service)
}

pub(super) fn renders(item: &TimelineItem) -> bool {
    item.as_event()
        .is_some_and(|event| classify(event.content()).is_some())
}

pub(super) fn event_media(item: &TimelineItem) -> Option<EventMedia> {
    match classify(item.as_event()?.content())? {
        Renderable::Message(message) => EventMedia::of_message(message.msgtype()),
        Renderable::Sticker(sticker) => Some(EventMedia::of_sticker(sticker.content())),
        Renderable::Utd | Renderable::Service(_) => None,
    }
}

fn reply_preview_from_content(content: &TimelineItemContent) -> (MessagePreviewKind, String) {
    match classify(content) {
        Some(Renderable::Message(message)) => {
            let preview = preview::from_msgtype(message.msgtype());
            (preview.kind, preview.body)
        }
        Some(Renderable::Sticker(_)) => (MessagePreviewKind::Sticker, String::new()),
        Some(Renderable::Utd) => (MessagePreviewKind::Encrypted, String::new()),
        Some(Renderable::Service(_)) | None => (MessagePreviewKind::None, String::new()),
    }
}

fn extract_reply(content: &TimelineItemContent) -> Option<ReplyInfo> {
    let details = content.in_reply_to()?;
    let event_id = details.event_id.to_string();
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
    Some(ReplyInfo {
        event_id,
        sender,
        kind,
        body,
    })
}

#[allow(clippy::cast_possible_truncation)]
fn pixels(value: UInt) -> u32 {
    let value: u64 = value.into();
    value as u32
}

fn image_meta(info: &ImageInfo) -> ImageMeta {
    ImageMeta {
        width: info.width.map(pixels),
        height: info.height.map(pixels),
        mimetype: info.mimetype.clone(),
        filename: None,
    }
}

fn extract_image_body(image: &ImageMessageEventContent) -> MessageBody {
    MessageBody::Image {
        caption: image
            .caption()
            .map(|caption| rich_body(caption, image.formatted_caption())),
        meta: ImageMeta {
            filename: Some(image.filename().to_owned()),
            ..image.info.as_deref().map(image_meta).unwrap_or_default()
        },
    }
}

fn video_meta(info: &VideoInfo) -> VideoMeta {
    VideoMeta {
        image: ImageMeta {
            width: info.width.map(pixels),
            height: info.height.map(pixels),
            mimetype: info.mimetype.clone(),
            filename: None,
        },
        duration: info.duration,
        size: info.size.map(Into::into),
    }
}

fn extract_video_body(video: &VideoMessageEventContent) -> MessageBody {
    let mut meta = video.info.as_deref().map(video_meta).unwrap_or_default();
    meta.image.filename = Some(video.filename().to_owned());
    MessageBody::Video {
        caption: video
            .caption()
            .map(|caption| rich_body(caption, video.formatted_caption())),
        meta,
    }
}

fn amplitude(value: UnstableAmplitude) -> u16 {
    u16::try_from(u64::from(value.get())).unwrap_or(u16::MAX)
}

fn audio_meta(audio: &AudioMessageEventContent) -> AudioMeta {
    let info = audio.info.as_deref();
    let details = audio.audio.as_ref();
    AudioMeta {
        kind: if audio.voice.is_some() {
            AudioKind::Voice
        } else {
            AudioKind::Track
        },
        filename: audio.filename().to_owned(),
        mimetype: info.and_then(|info| info.mimetype.clone()),
        duration: info
            .and_then(|info| info.duration)
            .or_else(|| details.map(|details| details.duration)),
        size: info.and_then(|info| info.size).map(Into::into),
        waveform: details.and_then(|details| {
            Waveform::from_amplitudes(details.waveform.iter().copied().map(amplitude))
        }),
    }
}

fn extract_audio_body(audio: &AudioMessageEventContent) -> MessageBody {
    MessageBody::Audio {
        caption: audio
            .caption()
            .map(|caption| rich_body(caption, audio.formatted_caption())),
        meta: audio_meta(audio),
    }
}

fn extract_sticker_body(sticker: &StickerEventContent) -> MessageBody {
    MessageBody::Sticker {
        alt: sticker.body.clone(),
        meta: image_meta(&sticker.info),
    }
}

fn extract_file_body(file: &FileMessageEventContent) -> MessageBody {
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

fn rich_body(plain: &str, formatted: Option<&FormattedBody>) -> RichText {
    match formatted {
        Some(formatted) if formatted.format == MessageFormat::Html => {
            RichText::formatted(plain.to_owned(), formatted.body.clone())
        }
        _ => RichText::plain(plain.to_owned()),
    }
}

fn message_type_to_body(msgtype: &MessageType) -> MessageBody {
    match msgtype {
        MessageType::Text(t) => MessageBody::Text(rich_body(&t.body, t.formatted.as_ref())),
        MessageType::Notice(n) => MessageBody::Notice(rich_body(&n.body, n.formatted.as_ref())),
        MessageType::Emote(e) => MessageBody::Emote(rich_body(&e.body, e.formatted.as_ref())),
        MessageType::Image(i) => extract_image_body(i),
        MessageType::Video(v) => extract_video_body(v),
        MessageType::Audio(a) => extract_audio_body(a),
        MessageType::File(f) => extract_file_body(f),
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

    match classify(content) {
        Some(Renderable::Message(message)) => {
            let body = message_type_to_body(message.msgtype());
            Some(TimelineMessage {
                body,
                reply,
                edited: message.is_edited(),
                ..base_message(unique_id, event, event_id_str, ctx)
            })
        }
        Some(Renderable::Sticker(sticker)) => {
            let body = extract_sticker_body(sticker.content());
            Some(TimelineMessage {
                body,
                reply,
                ..base_message(unique_id, event, event_id_str, ctx)
            })
        }
        Some(Renderable::Utd) => Some(build_utd_message(
            unique_id,
            event,
            event_id_str,
            ctx,
            reply,
        )),
        Some(Renderable::Service(service)) => Some(build_service_message(
            unique_id,
            event,
            event_id_str,
            ctx,
            service,
        )),
        None => {
            tracing::debug!(
                event_id = event_id_str,
                sender = %event.sender(),
                "skipping non-message event"
            );
            None
        }
    }
}
