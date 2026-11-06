mod bodies;

use std::collections::{BTreeMap, HashSet};

use matrix_sdk::ruma::events::StateEventContentChange;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk::ruma::events::rtc::notification::CallIntent;
use matrix_sdk_base::crypto::types::events::UtdCause;
use matrix_sdk_ui::timeline::{
    EncryptedMessage, EventSendState, EventTimelineItem, MemberProfileChange, MembershipChange,
    Message, PollState, Profile, ReactionInfo, RoomMembershipChange, Sticker, TimelineDetails,
    TimelineEventShieldState, TimelineEventShieldStateCode, TimelineItem, TimelineItemContent,
};

use self::bodies::{
    authored, emoji_states, extract_poll_body, extract_sticker_body, is_own, message_type_to_body,
    name_target, names_room, with_emoticons_marked,
};
use super::TimelineContext;
use super::members::{Arrived, MemberCache, Need};
use super::state_rows::{other_state_to_service, unparsed_state};
use crate::adapters::matrix::media::EventMedia;
use crate::adapters::matrix::preview;
use crate::domain::departure::membership_reason;
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::message::{
    DecryptionFailure, MessageBody, MessagePreviewKind, REACTOR_AVATAR_LIMIT, Reaction,
    ReactionSend, Reactor, ReadBy, ReplyInfo, RichText, SendState, ServiceEvent, Shield,
    ShieldCode, TimelineMessage,
};

fn reported_name(profile: &TimelineDetails<Profile>) -> MemberName {
    match profile {
        TimelineDetails::Ready(profile) => MemberName::new(
            profile.display_name.as_deref(),
            profile.display_name_ambiguous,
        ),
        _ => MemberName::Unset,
    }
}

fn sender_avatar_url(event: &EventTimelineItem) -> Option<String> {
    match event.sender_profile() {
        TimelineDetails::Ready(profile) => profile.avatar_url.as_ref().map(ToString::to_string),
        _ => None,
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

const STRICT_SHIELDS: bool = false;

fn shield(event: &EventTimelineItem) -> Shield {
    match event.get_shield(STRICT_SHIELDS) {
        TimelineEventShieldState::None => Shield::None,
        TimelineEventShieldState::Red { code } => Shield::Warning(shield_code(code)),
        TimelineEventShieldState::Grey { code } => Shield::Notice(shield_code(code)),
    }
}

fn shield_code(code: TimelineEventShieldStateCode) -> ShieldCode {
    match code {
        TimelineEventShieldStateCode::AuthenticityNotGuaranteed => {
            ShieldCode::AuthenticityNotGuaranteed
        }
        TimelineEventShieldStateCode::UnknownDevice => ShieldCode::UnknownDevice,
        TimelineEventShieldStateCode::UnsignedDevice => ShieldCode::UnsignedDevice,
        TimelineEventShieldStateCode::UnverifiedIdentity => ShieldCode::UnverifiedIdentity,
        TimelineEventShieldStateCode::VerificationViolation => ShieldCode::VerificationViolation,
        TimelineEventShieldStateCode::MismatchedSender => ShieldCode::MismatchedSender,
        TimelineEventShieldStateCode::SentInClear => ShieldCode::SentInClear,
    }
}

fn decryption_failure(encrypted: &EncryptedMessage) -> DecryptionFailure {
    let EncryptedMessage::MegolmV1AesSha2 { cause, .. } = encrypted else {
        return DecryptionFailure::Unexplained;
    };
    match cause {
        UtdCause::Unknown => DecryptionFailure::Unexplained,
        UtdCause::SentBeforeWeJoined => DecryptionFailure::SentBeforeJoining,
        UtdCause::WithheldForUnverifiedOrInsecureDevice => {
            DecryptionFailure::WithheldFromUnverified
        }
        UtdCause::WithheldBySender => DecryptionFailure::WithheldBySender,
        UtdCause::HistoricalMessageAndBackupIsDisabled => DecryptionFailure::HistoryWithoutBackup,
        UtdCause::HistoricalMessageAndDeviceIsUnverified => {
            DecryptionFailure::HistoryNeedsVerification
        }
        UtdCause::UnsignedDevice => DecryptionFailure::UnsignedDevice,
        UtdCause::UnknownDevice => DecryptionFailure::UnknownDevice,
        UtdCause::VerificationViolation => DecryptionFailure::VerificationViolation,
    }
}

fn local_id(event: &EventTimelineItem) -> Option<String> {
    let txn = event.transaction_id()?;
    Some(format!("{LOCAL_ID_PREFIX}{txn}"))
}

pub(super) fn reaction_send(info: Option<&ReactionInfo>) -> ReactionSend {
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
                image: ctx.emoji_files.state(key, ctx.media),
                senders: by_sender
                    .keys()
                    .map(|user_id| Reactor::new(user_id.to_string()))
                    .collect(),
                mine: own.is_some(),
                send: reaction_send(own),
            };
            for reactor in reaction.named_mut() {
                reactor.name = ctx.members.names.name_of(&reactor.user_id);
            }
            if reaction.shows_reactors() {
                attach_reactor_avatars(&mut reaction, ctx);
            }
            reaction
        })
        .collect()
}

fn attach_reactor_avatars(reaction: &mut Reaction, ctx: &TimelineContext<'_>) {
    for reactor in &mut reaction.senders {
        reactor.avatar_url = ctx.members.avatars.get(&reactor.user_id);
        if reactor.avatar_url.is_none() {
            ctx.members.avatars.want(&reactor.user_id);
        }
    }
}

pub(super) fn involves(
    item: &TimelineItem,
    message: Option<&TimelineMessage>,
    arrived: &Arrived,
    names: &MemberCache<MemberName>,
) -> bool {
    match arrived.need {
        Need::Avatar => reacted_by(item, &arrived.users),
        Need::Name => message.is_some_and(|message| renamed(message, &arrived.users, names)),
    }
}

fn renamed(
    message: &TimelineMessage,
    users: &HashSet<String>,
    names: &MemberCache<MemberName>,
) -> bool {
    let differs = |user_id: &str, shown: &MemberName| {
        users.contains(user_id) && names.confirm(user_id, shown.clone()) != *shown
    };
    differs(&message.sender, &message.sender_name)
        || message
            .reply
            .as_ref()
            .is_some_and(|reply| differs(&reply.sender.user_id, &reply.sender.name))
        || message
            .reactions
            .iter()
            .flat_map(Reaction::named)
            .any(|reactor| users.contains(&reactor.user_id))
        || voted_by(message, users)
        || message
            .body
            .service()
            .and_then(ServiceEvent::target)
            .is_some_and(|target| users.contains(&target.user_id))
}

fn voted_by(message: &TimelineMessage, users: &HashSet<String>) -> bool {
    message.body.poll().is_some_and(|poll| {
        poll.named_voters()
            .any(|voter| users.contains(&voter.user_id))
    })
}

pub(super) fn reacted_with(item: &TimelineItem, key: &str) -> bool {
    item.as_event()
        .and_then(|event| event.content().reactions())
        .is_some_and(|by_key| by_key.contains_key(key))
}

fn reacted_by(item: &TimelineItem, users: &HashSet<String>) -> bool {
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
    let ts: u64 = event.timestamp().0.into();
    let sender_str = event.sender().to_string();
    let sender_name = ctx
        .members
        .names
        .confirm(&sender_str, reported_name(event.sender_profile()));
    let is_own = ctx.own_user_id.is_some_and(|uid| uid == sender_str);
    let is_first_unread = ctx.first_unread.is_some_and(|id| id == event_id_str);
    TimelineMessage {
        unique_id,
        event_id: event_id_from_str(event_id_str),
        local_id: local_id(event),
        sender_pronouns: ctx.pronouns.resolved(&sender_str),
        sender: sender_str,
        sender_name,
        sender_avatar_url: sender_avatar_url(event),
        body: MessageBody::UnableToDecrypt(DecryptionFailure::Unexplained),
        body_emoji: BTreeMap::new(),
        mentions_room: false,
        timestamp: ts,
        is_own,
        reply: None,
        edited: false,
        editable: event.is_editable(),
        is_first_unread,
        send_state: send_state(event),
        shield: Shield::None,
        reactions: extract_reactions(event.content(), ctx),
        read_by: ReadBy::default(),
    }
}

fn membership_to_service(change: &RoomMembershipChange) -> Option<ServiceEvent> {
    let target = NamedUser::unnamed(change.user_id().as_str());
    Some(match change.change()? {
        MembershipChange::Joined => ServiceEvent::Joined,
        MembershipChange::Left => ServiceEvent::Left,
        MembershipChange::Invited => ServiceEvent::Invited { target },
        MembershipChange::InvitationAccepted => ServiceEvent::InvitationAccepted,
        MembershipChange::InvitationRejected => ServiceEvent::InvitationRejected,
        MembershipChange::InvitationRevoked => ServiceEvent::InvitationRevoked { target },
        MembershipChange::Kicked => ServiceEvent::Kicked {
            target,
            reason: reason_given(change),
        },
        MembershipChange::Banned | MembershipChange::KickedAndBanned => ServiceEvent::Banned {
            target,
            reason: reason_given(change),
        },
        MembershipChange::Unbanned => ServiceEvent::Unbanned { target },
        MembershipChange::Knocked => ServiceEvent::Knocked,
        MembershipChange::KnockAccepted => ServiceEvent::KnockAccepted { target },
        MembershipChange::KnockRetracted => ServiceEvent::KnockRetracted,
        MembershipChange::KnockDenied => ServiceEvent::KnockDenied { target },
        MembershipChange::None | MembershipChange::Error | MembershipChange::NotImplemented => {
            return None;
        }
    })
}

fn reason_given(change: &RoomMembershipChange) -> Option<String> {
    let StateEventContentChange::Original { content, .. } = change.content() else {
        return None;
    };
    content.reason.clone().and_then(membership_reason)
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

fn service_event_from_content(content: &TimelineItemContent) -> Option<ServiceEvent> {
    match content {
        TimelineItemContent::MembershipChange(change) => membership_to_service(change),
        TimelineItemContent::ProfileChange(change) => profile_to_service(change),
        TimelineItemContent::OtherState(state) => other_state_to_service(state.content()),
        TimelineItemContent::CallInvite => Some(ServiceEvent::CallStarted),
        TimelineItemContent::RtcNotification {
            call_intent,
            declined_by,
            ..
        } => Some(ServiceEvent::CallNotification {
            video: matches!(call_intent, Some(CallIntent::Video)),
            declined_by: declined_by
                .first()
                .map(|user_id| NamedUser::unnamed(user_id.as_str())),
        }),
        _ => None,
    }
}

pub(super) enum Renderable<'a> {
    Message(&'a Message),
    Sticker(&'a Sticker),
    Poll(&'a PollState),
    Utd(&'a EncryptedMessage),
    Service(ServiceEvent),
}

pub(super) fn classify(content: &TimelineItemContent) -> Option<Renderable<'_>> {
    if let Some(message) = content.as_message() {
        if let MessageType::VerificationRequest(request) = message.msgtype() {
            return Some(Renderable::Service(ServiceEvent::VerificationRequested {
                target: NamedUser::unnamed(request.to.as_str()),
            }));
        }
        return Some(Renderable::Message(message));
    }
    if let Some(sticker) = content.as_sticker() {
        return Some(Renderable::Sticker(sticker));
    }
    if let Some(poll) = content.as_poll() {
        return Some(Renderable::Poll(poll));
    }
    if let Some(encrypted) = content.as_unable_to_decrypt() {
        return Some(Renderable::Utd(encrypted));
    }
    service_event_from_content(content).map(Renderable::Service)
}

fn classify_event(event: &EventTimelineItem) -> Option<Renderable<'_>> {
    classify(event.content()).or_else(|| unparsed_state(event).map(Renderable::Service))
}

pub(super) fn renders(item: &TimelineItem) -> bool {
    item.as_event()
        .is_some_and(|event| classify_event(event).is_some())
}

pub(super) fn event_media(item: &TimelineItem) -> Option<EventMedia> {
    match classify(item.as_event()?.content())? {
        Renderable::Message(message) => EventMedia::of_message(message.msgtype()),
        Renderable::Sticker(sticker) => Some(EventMedia::of_sticker(sticker.content())),
        Renderable::Poll(_) | Renderable::Utd(_) | Renderable::Service(_) => None,
    }
}

pub(super) fn content_preview(content: &TimelineItemContent) -> (MessagePreviewKind, RichText) {
    if content.is_redacted() {
        return (MessagePreviewKind::Deleted, RichText::default());
    }
    match classify(content) {
        Some(Renderable::Message(message)) => {
            let preview = preview::from_msgtype(message.msgtype());
            (preview.kind, preview.body)
        }
        Some(Renderable::Sticker(_)) => (MessagePreviewKind::Sticker, RichText::default()),
        Some(Renderable::Poll(poll)) => (
            MessagePreviewKind::Poll,
            RichText::plain(poll.results().question),
        ),
        Some(Renderable::Utd(_)) => (MessagePreviewKind::Encrypted, RichText::default()),
        Some(Renderable::Service(_)) | None => (MessagePreviewKind::None, RichText::default()),
    }
}

fn extract_reply(content: &TimelineItemContent, ctx: &TimelineContext<'_>) -> Option<ReplyInfo> {
    let details = content.in_reply_to()?;
    let event_id = details.event_id.to_string();
    let TimelineDetails::Ready(embedded) = details.event else {
        return None;
    };
    let sender_id = embedded.sender.as_str();
    let sender = NamedUser::new(
        sender_id,
        ctx.members
            .names
            .confirm(sender_id, reported_name(&embedded.sender_profile)),
    );
    let (kind, body) = content_preview(&embedded.content);
    Some(ReplyInfo {
        event_id,
        sender,
        kind,
        body,
    })
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

    let reply = extract_reply(event.content(), ctx);
    let Some(renderable) = classify_event(event) else {
        tracing::debug!(
            event_id = event_id_str,
            sender = %event.sender(),
            "skipping non-message event"
        );
        return None;
    };

    Some(match renderable {
        Renderable::Message(message) => {
            let ShownMessage {
                body,
                mentions_room,
                edited,
            } = shown_message(event, message, ctx);
            TimelineMessage {
                body_emoji: emoji_states(&body, ctx),
                body,
                mentions_room,
                reply,
                edited,
                shield: shield(event),
                ..base_message(unique_id, event, event_id_str, ctx)
            }
        }
        Renderable::Sticker(sticker) => TimelineMessage {
            body: extract_sticker_body(sticker.content()),
            reply,
            shield: shield(event),
            ..base_message(unique_id, event, event_id_str, ctx)
        },
        Renderable::Poll(poll) => TimelineMessage {
            body: extract_poll_body(event, poll, ctx),
            reply,
            edited: poll.is_edit(),
            shield: shield(event),
            ..base_message(unique_id, event, event_id_str, ctx)
        },
        Renderable::Utd(encrypted) => TimelineMessage {
            body: MessageBody::UnableToDecrypt(decryption_failure(encrypted)),
            reply,
            ..base_message(unique_id, event, event_id_str, ctx)
        },
        Renderable::Service(service) => TimelineMessage {
            sender_pronouns: Vec::new(),
            body: MessageBody::Service(name_target(service, ctx)),
            ..base_message(unique_id, event, event_id_str, ctx)
        },
    })
}

struct ShownMessage {
    body: MessageBody,
    mentions_room: bool,
    edited: bool,
}

fn shown_message(
    event: &EventTimelineItem,
    message: &Message,
    ctx: &TimelineContext<'_>,
) -> ShownMessage {
    let (body, mentions_room, edited) = match ctx.discarded.original_content(event) {
        Some(original) => (
            message_type_to_body(&original.msgtype),
            names_room(original.mentions.as_ref()),
            false,
        ),
        None => (
            message_type_to_body(message.msgtype()),
            names_room(message.mentions()),
            message.is_edited(),
        ),
    };
    let body = with_emoticons_marked(body, event, ctx);
    let body = if is_own(event, ctx) {
        authored(body)
    } else {
        body
    };
    ShownMessage {
        body,
        mentions_room,
        edited,
    }
}
