use std::path::PathBuf;

use slint::{Image, SharedString, StyledText};

use super::body::{BodyLineDto, message_body, preview_line, row_body_and_preview};
use super::reactions::{ReactionDto, reaction_dtos};
use crate::adapters::ui::decode::{
    AvatarSlot, DecodeFailure, Decoded, MediaSlot, TimelineItemKey, is_animatable,
    load_avatar_async, load_thumbnail, peek_avatar, peek_thumbnail, record_media_need,
};
use crate::adapters::ui::present::{
    Delivery, MessageKind, PollPhase, ServiceKind, ShieldReason, avatar_color_index,
    decryption_failure, delivery, duration_label, message_kind, message_sender_label,
    message_sent_at_label, message_timestamp_label, name_initials, poll_phase, preview_subject,
    pronoun_labels, reader_labels, service_kind, service_target, shield_reason, unsupported_kind,
    voice_bars, voter_labels,
};
use crate::adapters::ui::richtext::{Cover, Spoilers};
use crate::adapters::ui::schema::{
    define_ui_enum, define_ui_names, emoji_arts, media_failures, media_states, spoiler_states,
};
use crate::domain::media::{
    AudioKind, AudioMeta, ContentKey, FileMeta, MediaFailure, ThumbnailOutcome, file_extension,
};
use crate::domain::member_name::isolated;
use crate::domain::message::{
    DecryptionFailure, MessageBody, MessagePreviewKind, SendState, ServiceEvent, Shield,
    TimelineMessage,
};
use crate::domain::poll::{Poll, PollAnswer};
use crate::domain::timeline::EnrichmentDelta;
use crate::ports::media::MediaCache;
use crate::util::{format_bytes, saturating_i32};

media_states!(define_ui_enum MediaState;);

media_states!(define_ui_names MediaState;);

media_failures!(define_ui_enum MediaFailureKind;);

emoji_arts!(define_ui_enum EmojiArt;);

spoiler_states!(define_ui_enum SpoilerState;);

pub const REVEALABLE: Spoilers = Spoilers::Covered(Cover::Link);

fn failure_kind(failure: MediaFailure) -> MediaFailureKind {
    match failure {
        MediaFailure::NoSource => MediaFailureKind::NoSource,
        MediaFailure::Download => MediaFailureKind::Download,
        MediaFailure::TooLarge => MediaFailureKind::TooLarge,
        MediaFailure::Storage => MediaFailureKind::Storage,
        MediaFailure::Unreadable => MediaFailureKind::Unreadable,
    }
}

pub(in crate::adapters::ui) fn decode_failure_kind(failure: DecodeFailure) -> MediaFailureKind {
    match failure {
        DecodeFailure::UnsupportedFormat => MediaFailureKind::UnsupportedFormat,
        DecodeFailure::OverBudget => MediaFailureKind::TooLargeToDisplay,
        DecodeFailure::Damaged => MediaFailureKind::Damaged,
        DecodeFailure::Unreadable => MediaFailureKind::Unreadable,
    }
}

#[derive(Clone)]
pub struct PollAnswerDto {
    pub id: SharedString,
    pub label: SharedString,
    pub count: i32,
    pub share: f32,
    pub mine: bool,
    pub leading: bool,
    pub votable: bool,
    pub voters: SharedString,
    pub hidden_voters: i32,
}

#[allow(clippy::struct_excessive_bools)]
pub struct MessageDto {
    pub unique_id: SharedString,
    pub local_id: SharedString,
    pub sender: SharedString,
    pub sender_id: SharedString,
    pub pronouns: Vec<SharedString>,
    pub body: SharedString,
    pub styled: StyledText,
    pub has_links: bool,
    pub mentions_you: bool,
    pub body_lines: Vec<BodyLineDto>,
    pub emote_only: bool,
    pub spoilers: SpoilerState,
    pub timestamp: SharedString,
    pub sent_at: SharedString,
    pub message_type: MessageKind,
    pub preview_kind: MessagePreviewKind,
    pub preview_body: SharedString,
    pub unsupported_kind: SharedString,
    pub event_id: SharedString,
    pub sender_initial: SharedString,
    pub color_index: i32,
    pub is_own: bool,
    pub edited: bool,
    pub text_editable: bool,
    pub edit_text: SharedString,
    pub pinned: bool,
    pub first_unread: bool,
    pub counts_as_unread: bool,
    pub send_state: SendState,
    pub send_progress: f32,
    pub delivery: Delivery,
    pub shield: Shield,
    pub shield_reason: ShieldReason,
    pub decryption_failure: DecryptionFailure,
    pub readers: SharedString,
    pub hidden_readers: i32,
    pub reader_count: i32,
    pub has_reply: bool,
    pub reply_event_id: SharedString,
    pub reply_sender: SharedString,
    pub reply_kind: MessagePreviewKind,
    pub reply_body: SharedString,
    pub service_kind: ServiceKind,
    pub service_target: SharedString,
    pub service_reason: SharedString,
    pub image_width: i32,
    pub image_height: i32,
    pub duration: SharedString,
    pub filename: SharedString,
    pub size: SharedString,
    pub map_link: SharedString,
    pub audio_kind: AudioKind,
    pub waveform: Vec<f32>,
    pub mimetype: SharedString,
    pub image_extension: SharedString,
    pub thumbnail: Option<Image>,
    pub media_state: MediaState,
    pub media_failure: MediaFailureKind,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
    pub needs_media: bool,
    pub reactions: Vec<ReactionDto>,
    pub all_reactions: Vec<ReactionDto>,
    pub poll_phase: PollPhase,
    pub poll_editable: bool,
    pub poll_choices: i32,
    pub poll_voters: i32,
    pub poll_answers: Vec<PollAnswerDto>,
}

pub enum ThumbUpdate {
    Unchanged,
    Failed(MediaFailureKind),
    Ready(Image),
}

pub struct EnrichUpdate {
    pub thumbnail: ThumbUpdate,
    pub avatar: Option<Image>,
    pub pronouns: Option<Vec<SharedString>>,
}

pub struct AudioRowUpdate {
    pub media_state: MediaState,
    pub media_failure: MediaFailureKind,
    pub waveform: Vec<f32>,
}

fn audio_media_state(file: &ContentKey, media: &dyn MediaCache) -> (MediaState, MediaFailureKind) {
    if media.audio_path(file).is_some() {
        return (MediaState::Ready, MediaFailureKind::None);
    }
    media
        .audio_failure(file)
        .map_or((MediaState::Idle, MediaFailureKind::None), |reason| {
            (MediaState::Failed, failure_kind(reason))
        })
}

pub fn audio_row_update(meta: &AudioMeta, media: &dyn MediaCache) -> AudioRowUpdate {
    let (media_state, media_failure) = audio_media_state(&meta.file, media);
    AudioRowUpdate {
        media_state,
        media_failure,
        waveform: voice_bars(meta, media.audio_waveform(&meta.file).as_ref()),
    }
}

fn size_label(size: Option<u64>) -> SharedString {
    size.map(|size| SharedString::from(&format_bytes(size)))
        .unwrap_or_default()
}

fn apply_file(dto: &mut MessageDto, meta: &FileMeta) {
    dto.filename = SharedString::from(&meta.filename);
    dto.mimetype = SharedString::from(meta.mimetype.as_deref().unwrap_or_default());
    dto.size = size_label(meta.size);
}

fn apply_audio(dto: &mut MessageDto, meta: &AudioMeta, media: &dyn MediaCache) {
    dto.audio_kind = meta.kind;
    dto.filename = SharedString::from(&meta.filename);
    dto.mimetype = SharedString::from(meta.mimetype.as_deref().unwrap_or_default());
    dto.size = size_label(meta.size);
    if let Some(duration) = meta.duration {
        dto.duration = SharedString::from(&duration_label(duration));
    }
    AudioRowUpdate {
        media_state: dto.media_state,
        media_failure: dto.media_failure,
        waveform: dto.waveform,
    } = audio_row_update(meta, media);
}

fn poll_answer_dto(
    poll: &Poll,
    answer: &PollAnswer,
    turnout: usize,
    votable: bool,
) -> PollAnswerDto {
    let (voters, hidden_voters) = voter_labels(answer);
    let sealed = PollAnswerDto {
        id: SharedString::from(&answer.id),
        label: SharedString::from(&answer.text),
        count: 0,
        share: 0.0,
        mine: answer.mine(),
        leading: false,
        votable: votable && poll.can_toggle(answer),
        voters: SharedString::new(),
        hidden_voters: 0,
    };
    if !poll.reveals_results() {
        return sealed;
    }
    PollAnswerDto {
        count: saturating_i32(answer.votes()),
        share: answer.share(turnout),
        leading: !poll.is_open() && poll.is_leading(answer),
        voters: SharedString::from(voters),
        hidden_voters: saturating_i32(hidden_voters),
        ..sealed
    }
}

fn apply_poll(dto: &mut MessageDto, poll: &Poll, message: &TimelineMessage) {
    let sent = message.event_id.is_some();
    let turnout = poll.voter_count();
    dto.poll_phase = poll_phase(poll);
    dto.poll_editable = sent && message.is_own && poll.editable;
    dto.poll_choices = saturating_i32(poll.choice.max().min(poll.answers.len()));
    dto.poll_voters = if poll.reveals_results() {
        saturating_i32(turnout)
    } else {
        0
    };
    dto.poll_answers = poll
        .answers
        .iter()
        .map(|answer| poll_answer_dto(poll, answer, turnout, sent))
        .collect();
}

fn apply_media(
    dto: &mut MessageDto,
    m: &TimelineMessage,
    item: &TimelineItemKey,
    media: &dyn MediaCache,
) -> Option<PathBuf> {
    apply_kind_details(dto, m, media);
    let (_, meta) = m.body.media()?;
    dto.image_width = meta.width.unwrap_or(0).cast_signed();
    dto.image_height = meta.height.unwrap_or(0).cast_signed();
    dto.mimetype = SharedString::from(meta.mimetype.as_deref().unwrap_or_default());
    dto.filename = SharedString::from(meta.filename.as_deref().unwrap_or_default());
    dto.image_extension = SharedString::from(
        meta.filename
            .as_deref()
            .map(file_extension)
            .unwrap_or_default()
            .to_uppercase(),
    );
    apply_thumbnail(dto, meta.thumbnail.as_ref(), item, m.is_own, media)
}

fn apply_kind_details(dto: &mut MessageDto, m: &TimelineMessage, media: &dyn MediaCache) {
    if let MessageBody::Video { meta, .. } = &m.body
        && let Some(duration) = meta.duration
    {
        dto.duration = SharedString::from(&duration_label(duration));
    }
    if let Some(meta) = m.body.audio() {
        apply_audio(dto, meta, media);
    }
    if let MessageBody::File { meta, .. } = &m.body {
        apply_file(dto, meta);
    }
    if let MessageBody::Location(location) = &m.body {
        dto.map_link = location
            .map_link()
            .map(SharedString::from)
            .unwrap_or_default();
    }
    if let Some(poll) = m.body.poll() {
        apply_poll(dto, poll, m);
    }
}

fn apply_thumbnail(
    dto: &mut MessageDto,
    content: Option<&ContentKey>,
    item: &TimelineItemKey,
    is_own: bool,
    media: &dyn MediaCache,
) -> Option<PathBuf> {
    let Some(content) = content else {
        dto.media_state = MediaState::Failed;
        dto.media_failure = MediaFailureKind::NoSource;
        return None;
    };
    if !is_own && media.withholds(content) {
        dto.media_state = MediaState::NotLoaded;
        return None;
    }
    let Some(path) = media.thumbnail_path(content) else {
        if let Some(reason) = media.thumbnail_failure(content) {
            dto.media_state = MediaState::Failed;
            dto.media_failure = failure_kind(reason);
        }
        return None;
    };
    match peek_thumbnail(&path, &MediaSlot::Thumbnail(item.clone())) {
        Decoded::Ready(img) => {
            dto.thumbnail = Some(img);
            dto.media_state = MediaState::Ready;
        }
        Decoded::Failed(failure) => {
            dto.media_state = MediaState::Failed;
            dto.media_failure = decode_failure_kind(failure);
        }
        Decoded::Pending => {}
    }
    Some(path)
}

pub struct QuotedMessage {
    pub sender: SharedString,
    pub sent_at: SharedString,
    pub kind: MessagePreviewKind,
    pub body: SharedString,
    pub service_kind: ServiceKind,
    pub service_target: SharedString,
}

pub fn quoted_message(message: Option<&TimelineMessage>) -> QuotedMessage {
    let Some(m) = message else {
        return QuotedMessage {
            sender: SharedString::new(),
            sent_at: SharedString::new(),
            kind: MessagePreviewKind::None,
            body: SharedString::new(),
            service_kind: ServiceKind::None,
            service_target: SharedString::new(),
        };
    };
    let service = m.body.service();
    QuotedMessage {
        sender: SharedString::from(message_sender_label(m)),
        sent_at: SharedString::from(message_sent_at_label(m.timestamp)),
        kind: m.body.preview_kind(),
        body: preview_subject(&m.body).map_or_else(|| message_body(m).plain, SharedString::from),
        service_kind: service.map_or(ServiceKind::None, service_kind),
        service_target: SharedString::from(service.map(service_target).unwrap_or_default()),
    }
}

pub fn message_to_dto(m: &TimelineMessage, pinned: bool, media: &dyn MediaCache) -> MessageDto {
    let item = TimelineItemKey::current(&m.unique_id);
    let (reactions, all_reactions) = reaction_dtos(&item, &m.reactions, media);
    let (readers, hidden_readers) = reader_labels(&m.read_by);
    let reply = m.reply.as_ref();
    let (body, preview_body) = row_body_and_preview(&item, m, media);
    let mut dto = MessageDto {
        unique_id: SharedString::from(&m.unique_id),
        local_id: m
            .local_id
            .as_deref()
            .map(SharedString::from)
            .unwrap_or_default(),
        sender: SharedString::from(message_sender_label(m)),
        sender_id: SharedString::from(&m.sender),
        pronouns: pronoun_labels(&m.sender_pronouns)
            .into_iter()
            .map(SharedString::from)
            .collect(),
        body: body.rich.plain,
        styled: body.rich.styled,
        has_links: body.rich.has_links,
        mentions_you: body.rich.mentions_you,
        body_lines: body.lines,
        emote_only: body.emote_only,
        spoilers: body.spoilers,
        timestamp: SharedString::from(&message_timestamp_label(m.timestamp)),
        sent_at: if m.is_own {
            SharedString::from(message_sent_at_label(m.timestamp))
        } else {
            SharedString::new()
        },
        message_type: message_kind(&m.body),
        preview_kind: m.body.preview_kind(),
        preview_body,
        unsupported_kind: SharedString::from(unsupported_kind(&m.body)),
        event_id: SharedString::from(m.event_id.as_deref().unwrap_or_default()),
        sender_initial: SharedString::from(name_initials(&m.sender_name, &m.sender)),
        color_index: avatar_color_index(&m.sender),
        is_own: m.is_own,
        edited: m.edited,
        text_editable: m.editable_text().is_some(),
        edit_text: SharedString::from(m.editable_text().unwrap_or_default()),
        pinned,
        first_unread: m.is_first_unread,
        counts_as_unread: m.counts_as_unread(),
        send_state: m.send_state,
        send_progress: m.send_state.fraction(),
        delivery: delivery(m),
        shield: m.shield,
        shield_reason: shield_reason(m.shield),
        decryption_failure: decryption_failure(&m.body),
        readers: SharedString::from(readers),
        hidden_readers: saturating_i32(hidden_readers),
        reader_count: saturating_i32(m.read_by.total),
        has_reply: reply.is_some(),
        reply_event_id: SharedString::from(reply.map_or("", |r| r.event_id.as_str())),
        reply_sender: reply
            .map(|r| SharedString::from(r.sender.shown()))
            .unwrap_or_default(),
        reply_kind: reply.map_or(MessagePreviewKind::None, |r| r.kind),
        reply_body: reply.map(|r| preview_line(&r.body)).unwrap_or_default(),
        service_kind: m.body.service().map_or(ServiceKind::None, service_kind),
        service_target: SharedString::from(
            m.body.service().map(service_target).unwrap_or_default(),
        ),
        service_reason: service_reason(&m.body),
        image_width: 0,
        image_height: 0,
        duration: SharedString::new(),
        filename: SharedString::new(),
        size: SharedString::new(),
        map_link: SharedString::new(),
        audio_kind: AudioKind::Track,
        waveform: Vec::new(),
        mimetype: SharedString::new(),
        image_extension: SharedString::new(),
        thumbnail: None,
        media_state: MediaState::Idle,
        media_failure: MediaFailureKind::None,
        avatar: None,
        has_avatar: false,
        needs_media: false,
        reactions,
        all_reactions,
        poll_phase: PollPhase::Open,
        poll_editable: false,
        poll_choices: 0,
        poll_voters: 0,
        poll_answers: Vec::new(),
    };

    let thumbnail_path = apply_media(&mut dto, m, &item, media);
    let avatar_path = apply_sender_avatar(&mut dto, m, media);

    let thumbnail_undecoded = thumbnail_path.as_deref().is_some_and(|path| {
        dto.media_state == MediaState::Idle
            || (dto.media_state == MediaState::Ready && is_animatable(path))
    });
    let avatar_undecoded = avatar_path.is_some() && !dto.has_avatar;
    dto.needs_media = thumbnail_undecoded || avatar_undecoded;
    record_media_need(&item, thumbnail_path.as_deref(), avatar_path.as_deref());
    dto
}

fn service_reason(body: &MessageBody) -> SharedString {
    SharedString::from(
        body.service()
            .and_then(ServiceEvent::reason)
            .map(isolated)
            .unwrap_or_default(),
    )
}

fn apply_sender_avatar(
    dto: &mut MessageDto,
    m: &TimelineMessage,
    media: &dyn MediaCache,
) -> Option<PathBuf> {
    let avatar_path = m
        .sender_avatar_url
        .as_deref()
        .and_then(|mxc| media.user_avatar_path(mxc));
    if let Some(path) = &avatar_path
        && let Some(img) = peek_avatar(path)
    {
        dto.avatar = Some(img);
        dto.has_avatar = true;
    }
    avatar_path
}

pub fn enrich_to_update(delta: &EnrichmentDelta, media: &dyn MediaCache) -> EnrichUpdate {
    let item = TimelineItemKey::current(&delta.unique_id);
    let thumbnail = match delta.thumbnail {
        ThumbnailOutcome::Ready => delta
            .thumbnail_content
            .as_ref()
            .and_then(|content| media.thumbnail_path(content))
            .map_or(ThumbUpdate::Unchanged, |thumb_path| {
                match load_thumbnail(&thumb_path, &MediaSlot::Thumbnail(item.clone())) {
                    Decoded::Ready(image) => ThumbUpdate::Ready(image),
                    Decoded::Failed(failure) => ThumbUpdate::Failed(decode_failure_kind(failure)),
                    Decoded::Pending => ThumbUpdate::Unchanged,
                }
            }),
        ThumbnailOutcome::Failed(reason) => ThumbUpdate::Failed(failure_kind(reason)),
        ThumbnailOutcome::Unchanged => ThumbUpdate::Unchanged,
    };

    let avatar = delta
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.user_avatar_path(mxc))
        .and_then(|avatar_path| load_avatar_async(Some(&avatar_path), AvatarSlot::Message(item)));

    let pronouns = delta.pronouns.as_ref().map(|pronouns| {
        pronoun_labels(pronouns)
            .into_iter()
            .map(SharedString::from)
            .collect()
    });

    EnrichUpdate {
        thumbnail,
        avatar,
        pronouns,
    }
}
