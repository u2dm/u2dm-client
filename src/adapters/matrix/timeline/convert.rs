use std::collections::{BTreeMap, HashSet};

use matrix_sdk::ruma::UInt;
use matrix_sdk::ruma::events::Mentions;
use matrix_sdk::ruma::events::poll::start::PollKind;
use matrix_sdk::ruma::events::room::ImageInfo;
use matrix_sdk::ruma::events::room::message::{
    AudioMessageEventContent, FileMessageEventContent, FormattedBody, ImageMessageEventContent,
    MessageFormat, MessageType, UnstableAmplitude, VideoInfo, VideoMessageEventContent,
};
use matrix_sdk::ruma::events::rtc::notification::CallIntent;
use matrix_sdk::ruma::events::sticker::StickerEventContent;
use matrix_sdk::ruma::html::{HtmlSanitizerMode, RemoveReplyFallback, sanitize_html};
use matrix_sdk_base::crypto::types::events::UtdCause;
use matrix_sdk_ui::timeline::{
    EncryptedMessage, EventSendState, EventTimelineItem, MemberProfileChange, MembershipChange,
    Message, PollState, Profile, ReactionInfo, RoomMembershipChange, Sticker, TimelineDetails,
    TimelineEventShieldState, TimelineEventShieldStateCode, TimelineItem, TimelineItemContent,
};
use serde::Deserialize;

use super::TimelineContext;
use super::members::{Arrived, MemberCache, Need};
use super::state_rows::{other_state_to_service, unparsed_state};
use crate::adapters::matrix::media::{EventMedia, file_content, thumbnail_content};
use crate::adapters::matrix::preview;
use crate::adapters::{body_emoji, markdown};
use crate::domain::media::{
    AudioKind, AudioMeta, FileMeta, ImageMeta, MediaKind, VideoMeta, Waveform,
};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::message::{
    CustomEmoji, DecryptionFailure, MessageBody, MessagePreviewKind, REACTOR_AVATAR_LIMIT,
    Reaction, ReactionSend, Reactor, ReadBy, ReplyInfo, RichText, SendState, ServiceEvent, Shield,
    ShieldCode, TimelineMessage,
};
use crate::domain::poll::{Poll, PollAnswer, PollChoice, PollDisclosure, PollStatus, Voter};

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
        UtdCause::UnsignedDevice => DecryptionFailure::UnsignedDevice,
        UtdCause::UnknownDevice => DecryptionFailure::UnknownDevice,
        UtdCause::VerificationViolation => DecryptionFailure::VerificationViolation,
        UtdCause::Unknown
        | UtdCause::SentBeforeWeJoined
        | UtdCause::HistoricalMessageAndBackupIsDisabled
        | UtdCause::HistoricalMessageAndDeviceIsUnverified
        | UtdCause::WithheldForUnverifiedOrInsecureDevice
        | UtdCause::WithheldBySender => DecryptionFailure::Unexplained,
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

fn build_utd_message(
    unique_id: String,
    event: &EventTimelineItem,
    event_id_str: String,
    ctx: &TimelineContext<'_>,
    encrypted: &EncryptedMessage,
    reply: Option<ReplyInfo>,
) -> TimelineMessage {
    TimelineMessage {
        body: MessageBody::UnableToDecrypt(decryption_failure(encrypted)),
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

fn membership_to_service(change: &RoomMembershipChange) -> Option<ServiceEvent> {
    let target = NamedUser::unnamed(change.user_id().as_str());
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
        MembershipChange::KnockRetracted => ServiceEvent::KnockRetracted,
        MembershipChange::KnockDenied => ServiceEvent::KnockDenied { target },
        MembershipChange::None | MembershipChange::Error | MembershipChange::NotImplemented => {
            return None;
        }
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
        thumbnail: None,
    }
}

fn extract_image_body(image: &ImageMessageEventContent) -> MessageBody {
    let mut meta = ImageMeta {
        filename: Some(image.filename().to_owned()),
        ..image.info.as_deref().map(image_meta).unwrap_or_default()
    };
    meta.thumbnail = thumbnail_content(MediaKind::Photo, &meta, EventMedia::of_image(image));
    MessageBody::Image {
        caption: image
            .caption()
            .map(|caption| rich_body(caption, image.formatted_caption())),
        meta,
    }
}

fn video_meta(info: &VideoInfo) -> VideoMeta {
    VideoMeta {
        image: ImageMeta {
            width: info.width.map(pixels),
            height: info.height.map(pixels),
            mimetype: info.mimetype.clone(),
            filename: None,
            thumbnail: None,
        },
        duration: info.duration,
        size: info.size.map(Into::into),
    }
}

fn extract_video_body(video: &VideoMessageEventContent) -> MessageBody {
    let mut meta = video.info.as_deref().map(video_meta).unwrap_or_default();
    meta.image.filename = Some(video.filename().to_owned());
    meta.image.thumbnail =
        thumbnail_content(MediaKind::Video, &meta.image, EventMedia::of_video(video));
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
        file: file_content(&audio.source),
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
    let mut meta = image_meta(&sticker.info);
    meta.thumbnail = thumbnail_content(MediaKind::Sticker, &meta, EventMedia::of_sticker(sticker));
    MessageBody::Sticker {
        alt: sticker.body.clone(),
        meta,
    }
}

fn extract_poll_body(
    event: &EventTimelineItem,
    poll: &PollState,
    ctx: &TimelineContext<'_>,
) -> MessageBody {
    let results = poll.results();
    let ending = event
        .event_id()
        .is_some_and(|event_id| ctx.ending.contains(event_id.as_str()));
    let status = if results.end_time.is_some() || ending {
        PollStatus::Ended
    } else {
        PollStatus::Open
    };
    let answered_undecrypted = event
        .event_id()
        .is_some_and(|event_id| ctx.undecrypted.respond_to(event_id));
    let answers = results
        .answers
        .iter()
        .map(|answer| PollAnswer {
            id: answer.id.clone(),
            text: answer.text.clone(),
            voters: results
                .votes
                .get(&answer.id)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|user_id| Voter::new(user_id.clone(), ctx.own_user_id))
                .collect(),
        })
        .collect();
    let mut poll = Poll {
        disclosure: if results.kind == PollKind::Disclosed {
            PollDisclosure::Disclosed
        } else {
            PollDisclosure::Undisclosed
        },
        choice: PollChoice::up_to(usize::try_from(results.max_selections).unwrap_or(usize::MAX)),
        answers,
        editable: status == PollStatus::Open
            && event.is_editable()
            && !answered_undecrypted
            && !ctx.focused,
        status,
        question: results.question,
    };
    name_voters(&mut poll, ctx);
    MessageBody::Poll(poll)
}

fn name_voters(poll: &mut Poll, ctx: &TimelineContext<'_>) {
    for voter in poll.named_voters_mut() {
        voter.name = ctx.members.names.name_of(&voter.user_id);
    }
}

fn name_target(mut service: ServiceEvent, ctx: &TimelineContext<'_>) -> ServiceEvent {
    if let Some(target) = service.target_mut() {
        target.name = ctx.members.names.name_of(&target.user_id);
    }
    service
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

const REPLY_FALLBACKS: [RemoveReplyFallback; 2] =
    [RemoveReplyFallback::Yes, RemoveReplyFallback::No];

#[derive(Deserialize)]
struct EventJson<C> {
    content: C,
}

#[derive(Deserialize)]
struct FormattedJson {
    #[serde(default)]
    formatted_body: Option<String>,
}

#[derive(Deserialize)]
struct EditJson {
    #[serde(rename = "m.new_content")]
    new_content: FormattedJson,
}

fn emoji_states(body: &MessageBody, ctx: &TimelineContext<'_>) -> BTreeMap<String, CustomEmoji> {
    let Some(html) = body.html() else {
        return BTreeMap::new();
    };
    body_emoji::emoticon_sources(html)
        .into_iter()
        .filter_map(|mxc| {
            let state = ctx.emoji_files.state(&mxc, ctx.media)?;
            Some((mxc, state))
        })
        .collect()
}

fn with_emoticons_marked(
    body: MessageBody,
    event: &EventTimelineItem,
    ctx: &TimelineContext<'_>,
) -> MessageBody {
    match body {
        MessageBody::Text(text) => MessageBody::Text(emoticons_marked(text, event, ctx)),
        MessageBody::Notice(text) => MessageBody::Notice(emoticons_marked(text, event, ctx)),
        MessageBody::Emote(text) => MessageBody::Emote(emoticons_marked(text, event, ctx)),
        other => other,
    }
}

fn emoticons_marked(
    text: RichText,
    event: &EventTimelineItem,
    ctx: &TimelineContext<'_>,
) -> RichText {
    let Some(shown) = text.html() else {
        return text;
    };
    if body_emoji::image_sources(shown).is_empty() {
        return text;
    }
    let html = html_as_sent(shown, event)
        .or_else(|| is_own(event, ctx).then(|| body_emoji::marked_as_emoticons(shown)))
        .unwrap_or_else(|| shown.to_owned());
    RichText::formatted(text.plain, html)
}

fn html_as_sent(shown: &str, event: &EventTimelineItem) -> Option<String> {
    sent_versions(event).into_iter().find_map(|sent| {
        REPLY_FALLBACKS.into_iter().find_map(|fallback| {
            (sanitize_html(&sent, HtmlSanitizerMode::Compat, fallback) == shown)
                .then(|| markdown::sanitized_with(&sent, fallback))
        })
    })
}

fn sent_versions(event: &EventTimelineItem) -> Vec<String> {
    let edit = event
        .latest_edit_json()
        .and_then(|edit| edit.deserialize_as_unchecked::<EventJson<EditJson>>().ok())
        .and_then(|edit| edit.content.new_content.formatted_body);
    let original = event
        .original_json()
        .and_then(|original| {
            original
                .deserialize_as_unchecked::<EventJson<FormattedJson>>()
                .ok()
        })
        .and_then(|original| original.content.formatted_body);
    edit.into_iter().chain(original).collect()
}

fn names_room(mentions: Option<&Mentions>) -> bool {
    mentions.is_some_and(|mentions| mentions.room)
}

fn is_own(event: &EventTimelineItem, ctx: &TimelineContext<'_>) -> bool {
    ctx.own_user_id
        .is_some_and(|own| own == event.sender().as_str())
}

fn authored(body: MessageBody) -> MessageBody {
    match body {
        MessageBody::Text(text) => MessageBody::Text(markdown::own_text(&text.plain, text.html())),
        MessageBody::Image { caption, meta } => MessageBody::Image {
            caption: caption.map(markdown::own_caption),
            meta,
        },
        MessageBody::Video { caption, meta } => MessageBody::Video {
            caption: caption.map(markdown::own_caption),
            meta,
        },
        MessageBody::Audio { caption, meta } => MessageBody::Audio {
            caption: caption.map(markdown::own_caption),
            meta,
        },
        other => other,
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
    let reply = extract_reply(content, ctx);

    match classify_event(event) {
        Some(Renderable::Message(message)) => {
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
            Some(TimelineMessage {
                body_emoji: emoji_states(&body, ctx),
                body,
                mentions_room,
                reply,
                edited,
                shield: shield(event),
                ..base_message(unique_id, event, event_id_str, ctx)
            })
        }
        Some(Renderable::Sticker(sticker)) => {
            let body = extract_sticker_body(sticker.content());
            Some(TimelineMessage {
                body,
                reply,
                shield: shield(event),
                ..base_message(unique_id, event, event_id_str, ctx)
            })
        }
        Some(Renderable::Poll(poll)) => Some(TimelineMessage {
            body: extract_poll_body(event, poll, ctx),
            reply,
            edited: poll.is_edit(),
            shield: shield(event),
            ..base_message(unique_id, event, event_id_str, ctx)
        }),
        Some(Renderable::Utd(encrypted)) => Some(build_utd_message(
            unique_id,
            event,
            event_id_str,
            ctx,
            encrypted,
            reply,
        )),
        Some(Renderable::Service(service)) => Some(build_service_message(
            unique_id,
            event,
            event_id_str,
            ctx,
            name_target(service, ctx),
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
