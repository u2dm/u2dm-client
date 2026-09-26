use std::path::PathBuf;

use slint::{Image, SharedString, StyledText};

use super::decode::{
    AvatarSlot, DecodeFailure, Decoded, MediaSlot, TimelineItemKey, load_avatar_async,
    load_thumbnail, peek_avatar, peek_thumbnail, record_avatar_need, record_media_need,
    record_sticker_need,
};
use super::present::{
    Delivery, MessageKind, PollPhase, ServiceKind, avatar_color_index, avatar_initials, delivery,
    duration_label, file_extension, message_body_html, message_body_text, message_kind,
    message_sender_label, message_sent_at_label, message_timestamp_label, poll_phase,
    pronoun_labels, reaction_key_label, reactor_labels, reader_labels, room_activity_label,
    sender_initial, service_kind, service_target, unsupported_kind, user_initial, voice_bars,
    voter_labels,
};
use super::richtext;
use super::schema::{define_ui_enum, media_failures, media_states};
use crate::commands::view::{ChildAccess, SpaceIndexRow};
use crate::domain::media::{
    AudioKind, AudioMeta, ContentKey, FileMeta, MediaFailure, ThumbnailOutcome,
};
use crate::domain::message::{
    MessageBody, MessagePreviewKind, Reaction, ReactionSend, Reactor, RichText, SendState,
    TimelineMessage,
};
use crate::domain::poll::{Poll, PollAnswer};
use crate::domain::room::{Room, Space};
use crate::domain::space_index::{ChildKind, SpaceChild};
use crate::domain::sticker::{PackId, StickerImage, StickerPack};
use crate::domain::timeline::EnrichmentDelta;
use crate::ports::media::MediaCache;
use crate::util::format_bytes;

media_states!(define_ui_enum MediaState;);
media_failures!(define_ui_enum MediaFailureKind;);

fn failure_kind(failure: MediaFailure) -> MediaFailureKind {
    match failure {
        MediaFailure::NoSource => MediaFailureKind::NoSource,
        MediaFailure::Download => MediaFailureKind::Download,
        MediaFailure::TooLarge => MediaFailureKind::TooLarge,
        MediaFailure::Storage => MediaFailureKind::Storage,
        MediaFailure::Unreadable => MediaFailureKind::Unreadable,
    }
}

pub(super) fn decode_failure_kind(failure: DecodeFailure) -> MediaFailureKind {
    match failure {
        DecodeFailure::UnsupportedFormat => MediaFailureKind::UnsupportedFormat,
        DecodeFailure::OverBudget => MediaFailureKind::TooLargeToDisplay,
        DecodeFailure::Damaged => MediaFailureKind::Damaged,
        DecodeFailure::Unreadable => MediaFailureKind::Unreadable,
    }
}

pub const GRID_COLUMNS: i32 = 5;

#[derive(Clone)]
pub struct StickerCellDto {
    pub key: SharedString,
    pub pack_id: SharedString,
    pub shortcode: SharedString,
    pub label: SharedString,
    pub image: Option<Image>,
    pub media_state: MediaState,
    pub awaited_mxc: Option<String>,
}

pub struct StickerRowDto {
    pub title: SharedString,
    pub is_header: bool,
    pub cells: Vec<StickerCellDto>,
}

pub struct StickerPackDto {
    pub id: SharedString,
    pub title: SharedString,
    pub header_row: i32,
    pub icon: Option<Image>,
    pub has_icon: bool,
    pub icon_cell_key: SharedString,
}

pub struct StickerGrid {
    pub rows: Vec<StickerRowDto>,
    pub packs: Vec<StickerPackDto>,
}

pub const CELL_KEY_SEPARATOR: char = '\u{1}';

pub fn cell_key(pack: &PackId, shortcode: &str) -> String {
    format!("{pack}{CELL_KEY_SEPARATOR}{shortcode}")
}

pub fn cell_pack(key: &str) -> &str {
    key.split_once(CELL_KEY_SEPARATOR)
        .map_or(key, |(pack, _)| pack)
}

pub fn sticker_needle(query: &str) -> String {
    query.trim().to_lowercase()
}

pub fn sticker_grid(packs: &[StickerPack], needle: &str, media: &dyn MediaCache) -> StickerGrid {
    let per_row = usize::try_from(GRID_COLUMNS.max(1)).unwrap_or(1);
    let mut grid = StickerGrid {
        rows: Vec::new(),
        packs: Vec::new(),
    };

    for pack in packs {
        let whole_pack = needle.is_empty() || pack.title.to_lowercase().contains(needle);
        let cells: Vec<StickerCellDto> = pack
            .images
            .iter()
            .filter(|image| whole_pack || sticker_matches(image, needle))
            .map(|image| sticker_cell(pack, image, media))
            .collect();
        if cells.is_empty() {
            continue;
        }

        let icon = cells.first().and_then(|cell| cell.image.clone());
        grid.packs.push(StickerPackDto {
            id: SharedString::from(pack.id.as_ref()),
            title: SharedString::from(&pack.title),
            header_row: i32::try_from(grid.rows.len()).unwrap_or(0),
            has_icon: icon.is_some(),
            icon,
            icon_cell_key: cells
                .first()
                .map(|cell| cell.key.clone())
                .unwrap_or_default(),
        });
        grid.rows.push(StickerRowDto {
            title: SharedString::from(&pack.title),
            is_header: true,
            cells: Vec::new(),
        });
        for chunk in cells.chunks(per_row) {
            grid.rows.push(StickerRowDto {
                title: SharedString::new(),
                is_header: false,
                cells: chunk.to_vec(),
            });
        }
    }

    grid
}

fn sticker_matches(image: &StickerImage, needle: &str) -> bool {
    image.shortcode.to_lowercase().contains(needle) || image.body.to_lowercase().contains(needle)
}

pub enum StickerArt {
    Ready(Image),
    Failed,
    Decoding,
    Downloading,
}

pub fn sticker_art(key: &str, mxc: &str, media: &dyn MediaCache) -> StickerArt {
    let path = media.sticker_path(mxc);
    record_sticker_need(key, path.as_deref());
    if let Some(path) = path {
        return match peek_thumbnail(&path, &MediaSlot::StickerCell(key.to_owned())) {
            Decoded::Ready(decoded) => StickerArt::Ready(decoded),
            Decoded::Failed(_) => StickerArt::Failed,
            Decoded::Pending => StickerArt::Decoding,
        };
    }
    if media.sticker_failed(mxc) {
        StickerArt::Failed
    } else {
        StickerArt::Downloading
    }
}

fn sticker_cell(
    pack: &StickerPack,
    image: &StickerImage,
    media: &dyn MediaCache,
) -> StickerCellDto {
    let key = cell_key(&pack.id, &image.shortcode);
    let (art, media_state, awaited_mxc) = match sticker_art(&key, &image.mxc, media) {
        StickerArt::Ready(decoded) => (Some(decoded), MediaState::Ready, None),
        StickerArt::Failed => (None, MediaState::Failed, None),
        StickerArt::Decoding => (None, MediaState::Idle, None),
        StickerArt::Downloading => (None, MediaState::Idle, Some(image.mxc.clone())),
    };
    StickerCellDto {
        key: SharedString::from(&key),
        pack_id: SharedString::from(pack.id.as_ref()),
        shortcode: SharedString::from(&image.shortcode),
        label: SharedString::from(&image.body),
        image: art,
        media_state,
        awaited_mxc,
    }
}

pub const REACTION_CHIP_CAP: usize = 6;

#[derive(Clone)]
pub struct ReactorAvatarDto {
    pub user_id: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

#[derive(Clone)]
pub struct ReactionDto {
    pub key: SharedString,
    pub label: SharedString,
    pub count: i32,
    pub mine: bool,
    pub send: ReactionSend,
    pub overflow: bool,
    pub reactors: SharedString,
    pub hidden_reactors: i32,
    pub avatars: Vec<ReactorAvatarDto>,
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
    pub first_unread: bool,
    pub counts_as_unread: bool,
    pub send_state: SendState,
    pub send_progress: f32,
    pub delivery: Delivery,
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
    pub image_width: i32,
    pub image_height: i32,
    pub duration: SharedString,
    pub filename: SharedString,
    pub size: SharedString,
    pub audio_kind: AudioKind,
    pub waveform: Vec<f32>,
    pub image_mimetype: SharedString,
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

#[allow(clippy::struct_excessive_bools)]
pub struct RoomDto {
    pub id: SharedString,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub members: i32,
    pub alert: bool,
    pub mention: bool,
    pub hint: bool,
    pub muted: bool,
    pub last_message_sender: SharedString,
    pub last_message_kind: MessagePreviewKind,
    pub last_message_body: SharedString,
    pub last_message_service_kind: ServiceKind,
    pub last_message_service_target: SharedString,
    pub last_message_is_own: bool,
    pub last_message_edited: bool,
    pub last_message_time: SharedString,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

#[allow(clippy::struct_excessive_bools)]
pub struct SpaceDto {
    pub id: SharedString,
    pub name: SharedString,
    pub alert: bool,
    pub mention: bool,
    pub hint: bool,
    pub initial: SharedString,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

pub struct SpaceChildDto {
    pub id: SharedString,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub detail: SharedString,
    pub members: i32,
    pub is_space: bool,
    pub children: i32,
    pub access: ChildAccess,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
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

fn count<T: TryInto<i32>>(value: T) -> i32 {
    value.try_into().unwrap_or(i32::MAX)
}

fn reactor_avatar_dto(
    item: &TimelineItemKey,
    reactor: &Reactor,
    media: &dyn MediaCache,
) -> ReactorAvatarDto {
    let slot = AvatarSlot::Reactor {
        item: item.clone(),
        user_id: reactor.user_id.clone(),
    };
    let path = reactor
        .avatar_url
        .as_deref()
        .and_then(|mxc| media.user_avatar_path(mxc));
    let avatar = load_avatar_async(path.as_deref(), slot);
    ReactorAvatarDto {
        user_id: SharedString::from(&reactor.user_id),
        initial: SharedString::from(user_initial(&reactor.user_id)),
        color_index: avatar_color_index(&reactor.user_id),
        has_avatar: avatar.is_some(),
        avatar,
    }
}

fn reactor_avatar_dtos(
    item: &TimelineItemKey,
    reaction: &Reaction,
    media: &dyn MediaCache,
) -> Vec<ReactorAvatarDto> {
    if !reaction.shows_reactors() {
        return Vec::new();
    }
    reaction
        .senders
        .iter()
        .map(|reactor| reactor_avatar_dto(item, reactor, media))
        .collect()
}

fn reaction_dto(
    item: &TimelineItemKey,
    reaction: &Reaction,
    media: &dyn MediaCache,
) -> ReactionDto {
    let (reactors, hidden) = reactor_labels(&reaction.senders);
    ReactionDto {
        key: SharedString::from(&reaction.key),
        label: SharedString::from(reaction_key_label(&reaction.key)),
        count: count(reaction.count()),
        mine: reaction.mine,
        send: reaction.send,
        overflow: false,
        reactors: SharedString::from(reactors),
        hidden_reactors: count(hidden),
        avatars: reactor_avatar_dtos(item, reaction, media),
    }
}

fn overflow_dto(hidden: usize) -> ReactionDto {
    ReactionDto {
        key: SharedString::new(),
        label: SharedString::new(),
        count: count(hidden),
        mine: false,
        send: ReactionSend::default(),
        overflow: true,
        reactors: SharedString::new(),
        hidden_reactors: 0,
        avatars: Vec::new(),
    }
}

fn reaction_dtos(
    item: &TimelineItemKey,
    reactions: &[Reaction],
    media: &dyn MediaCache,
) -> (Vec<ReactionDto>, Vec<ReactionDto>) {
    let all: Vec<ReactionDto> = reactions
        .iter()
        .map(|reaction| reaction_dto(item, reaction, media))
        .collect();
    if all.len() <= REACTION_CHIP_CAP {
        return (all, Vec::new());
    }
    let mut chips: Vec<ReactionDto> = all
        .get(..REACTION_CHIP_CAP)
        .map(<[ReactionDto]>::to_vec)
        .unwrap_or_default();
    chips.push(overflow_dto(all.len() - REACTION_CHIP_CAP));
    (chips, all)
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
    dto.size = size_label(meta.size);
}

fn apply_audio(dto: &mut MessageDto, meta: &AudioMeta, media: &dyn MediaCache) {
    dto.audio_kind = meta.kind;
    dto.filename = SharedString::from(&meta.filename);
    dto.size = size_label(meta.size);
    if let Some(duration) = meta.duration {
        dto.duration = SharedString::from(&duration_label(duration));
    }
    (dto.media_state, dto.media_failure) = audio_media_state(&meta.file, media);
    dto.waveform = voice_bars(meta, media.audio_waveform(&meta.file).as_ref());
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
        count: count(answer.votes()),
        share: answer.share(turnout),
        leading: !poll.is_open() && poll.is_leading(answer),
        voters: SharedString::from(voters),
        hidden_voters: count(hidden_voters),
        ..sealed
    }
}

fn apply_poll(dto: &mut MessageDto, poll: &Poll, message: &TimelineMessage) {
    let sent = message.event_id.is_some();
    let turnout = poll.voter_count();
    dto.poll_phase = poll_phase(poll);
    dto.poll_editable = sent && message.is_own && poll.editable;
    dto.poll_choices = count(poll.choice.max().min(poll.answers.len()));
    dto.poll_voters = if poll.reveals_results() {
        count(turnout)
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
    if let MessageBody::Video { meta, .. } = &m.body
        && let Some(duration) = meta.duration
    {
        dto.duration = SharedString::from(&duration_label(duration));
    }
    if let Some(meta) = m.body.audio() {
        apply_audio(dto, meta, media);
    }
    if let MessageBody::File { meta } = &m.body {
        apply_file(dto, meta);
    }
    if let Some(poll) = m.body.poll() {
        apply_poll(dto, poll, m);
    }

    let (_, meta) = m.body.media()?;
    dto.image_width = meta.width.unwrap_or(0).cast_signed();
    dto.image_height = meta.height.unwrap_or(0).cast_signed();
    dto.image_mimetype = SharedString::from(meta.mimetype.as_deref().unwrap_or_default());
    dto.image_extension = SharedString::from(
        meta.filename
            .as_deref()
            .map(file_extension)
            .unwrap_or_default()
            .to_uppercase(),
    );

    let Some(content) = meta.thumbnail.as_ref() else {
        dto.media_state = MediaState::Failed;
        dto.media_failure = MediaFailureKind::NoSource;
        return None;
    };
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

fn one_line(text: &str) -> SharedString {
    SharedString::from(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub fn preview_line(text: &RichText) -> SharedString {
    match text.html.as_deref() {
        Some(html) => one_line(&richtext::styled_body(html, &text.plain).plain),
        None => one_line(&text.plain),
    }
}

pub fn message_to_dto(m: &TimelineMessage, media: &dyn MediaCache) -> MessageDto {
    let item = TimelineItemKey::current(&m.unique_id);
    let sender_label = message_sender_label(m);
    let (reactions, all_reactions) = reaction_dtos(&item, &m.reactions, media);
    let (readers, hidden_readers) = reader_labels(&m.read_by);
    let plain = message_body_text(&m.body);
    let rich = match message_body_html(&m.body) {
        Some(html) => richtext::styled_body(html, plain),
        None => richtext::plain_body(plain),
    };
    let preview_body = one_line(&rich.plain);
    let mut dto = MessageDto {
        unique_id: SharedString::from(&m.unique_id),
        local_id: m.local_id.as_deref().map(SharedString::from).unwrap_or_default(),
        sender: SharedString::from(sender_label),
        sender_id: SharedString::from(&m.sender),
        pronouns: pronoun_labels(&m.sender_pronouns)
            .into_iter()
            .map(SharedString::from)
            .collect(),
        body: rich.plain,
        styled: rich.styled,
        has_links: rich.has_links,
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
        sender_initial: SharedString::from(avatar_initials(sender_label)),
        color_index: avatar_color_index(&m.sender),
        is_own: m.is_own,
        edited: m.edited,
        first_unread: m.is_first_unread,
        counts_as_unread: m.counts_as_unread(),
        send_state: m.send_state,
        send_progress: m.send_state.fraction(),
        delivery: delivery(m),
        readers: SharedString::from(readers),
        hidden_readers: count(hidden_readers),
        reader_count: count(m.read_by.total),
        has_reply: m.reply.is_some(),
        reply_event_id: SharedString::from(m.reply.as_ref().map_or("", |r| r.event_id.as_str())),
        reply_sender: SharedString::from(m.reply.as_ref().map_or("", |r| r.sender.as_str())),
        reply_kind: m
            .reply
            .as_ref()
            .map_or(MessagePreviewKind::None, |r| r.kind),
        reply_body: m
            .reply
            .as_ref()
            .map(|r| preview_line(&r.body))
            .unwrap_or_default(),
        service_kind: m.body.service().map_or(ServiceKind::None, service_kind),
        service_target: SharedString::from(m.body.service().map_or("", service_target)),
        image_width: 0,
        image_height: 0,
        duration: SharedString::new(),
        filename: SharedString::new(),
        size: SharedString::new(),
        audio_kind: AudioKind::Track,
        waveform: Vec::new(),
        image_mimetype: SharedString::new(),
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

    let thumbnail_undecoded = thumbnail_path.is_some() && dto.media_state == MediaState::Idle;
    let avatar_undecoded = avatar_path.is_some() && !dto.has_avatar;
    dto.needs_media = thumbnail_undecoded || avatar_undecoded;
    record_media_need(&item, thumbnail_path.as_deref(), avatar_path.as_deref());
    dto
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

pub fn room_to_dto(r: &Room, media: &dyn MediaCache) -> RoomDto {
    let mut dto = RoomDto {
        id: SharedString::from(r.id.as_ref()),
        name: SharedString::from(&r.display_name),
        initial: SharedString::from(avatar_initials(&r.display_name)),
        color_index: avatar_color_index(r.id.as_ref()),
        members: if r.is_direct {
            0
        } else {
            count(r.member_count)
        },
        alert: r.alert(),
        mention: r.mention(),
        hint: r.hint(),
        muted: r.muted(),
        last_message_sender: SharedString::from(
            r.last_message_sender.as_deref().unwrap_or_default(),
        ),
        last_message_kind: r.last_message_kind,
        last_message_body: preview_line(&r.last_message_body),
        last_message_service_kind: r
            .last_message_service
            .as_ref()
            .map_or(ServiceKind::None, service_kind),
        last_message_service_target: SharedString::from(
            r.last_message_service.as_ref().map_or("", service_target),
        ),
        last_message_is_own: r.last_message_is_own,
        last_message_edited: r.last_message_edited,
        last_message_time: SharedString::from(&room_activity_label(r.last_activity_ts)),
        avatar: None,
        has_avatar: false,
    };

    if let Some(mxc) = &r.avatar_mxc
        && let Some(avatar_path) = media.room_avatar_path(mxc)
        && let Some(img) = peek_avatar(&avatar_path)
    {
        dto.avatar = Some(img);
        dto.has_avatar = true;
    }

    dto
}

pub fn record_room_avatar_need(r: &Room, media: &dyn MediaCache) {
    let avatar_path = r
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.room_avatar_path(mxc));
    record_avatar_need(
        &AvatarSlot::Room(r.id.as_ref().to_owned()),
        avatar_path.as_deref(),
    );
}

pub fn space_to_dto(s: &Space, media: &dyn MediaCache) -> SpaceDto {
    let mut dto = SpaceDto {
        id: SharedString::from(&s.id),
        name: SharedString::from(&s.name),
        alert: s.alert,
        mention: s.mention,
        hint: s.hint,
        initial: SharedString::from(sender_initial(&s.name)),
        avatar: None,
        has_avatar: false,
    };

    if let Some(mxc) = &s.avatar_mxc
        && let Some(avatar_path) = media.space_avatar_path(mxc)
        && let Some(img) = peek_avatar(&avatar_path)
    {
        dto.avatar = Some(img);
        dto.has_avatar = true;
    }

    dto
}

pub fn prefetch_space_avatar(s: &Space, media: &dyn MediaCache) {
    let avatar_path = s
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.space_avatar_path(mxc));
    load_avatar_async(avatar_path.as_deref(), AvatarSlot::Space(s.id.clone()));
}

pub fn space_child_to_dto(row: &SpaceIndexRow, media: &dyn MediaCache) -> SpaceChildDto {
    let child = &row.child;
    let (is_space, children) = match child.kind {
        ChildKind::Room => (false, 0),
        ChildKind::Space { children } => (true, count(children)),
    };
    let avatar = space_child_avatar_path(child, media).and_then(|path| peek_avatar(&path));
    SpaceChildDto {
        id: SharedString::from(child.id.as_ref()),
        name: SharedString::from(&child.name),
        initial: SharedString::from(avatar_initials(&child.name)),
        color_index: avatar_color_index(child.id.as_ref()),
        detail: SharedString::from(space_child_detail(child)),
        members: count(child.member_count),
        is_space,
        children,
        access: row.access,
        has_avatar: avatar.is_some(),
        avatar,
    }
}

pub fn request_space_child_avatar(row: &SpaceIndexRow, media: &dyn MediaCache) {
    let path = space_child_avatar_path(&row.child, media);
    load_avatar_async(
        path.as_deref(),
        AvatarSlot::SpaceChild(row.child.id.to_string()),
    );
}

fn space_child_avatar_path(child: &SpaceChild, media: &dyn MediaCache) -> Option<PathBuf> {
    let mxc = child.avatar_mxc.as_deref()?;
    match child.kind {
        ChildKind::Room => media.room_avatar_path(mxc),
        ChildKind::Space { .. } => media.space_avatar_path(mxc),
    }
}

fn space_child_detail(child: &SpaceChild) -> &str {
    child
        .topic
        .as_deref()
        .and_then(|topic| topic.lines().find(|line| !line.trim().is_empty()))
        .or(child.alias.as_deref())
        .unwrap_or_default()
}
