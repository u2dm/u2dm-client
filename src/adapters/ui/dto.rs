use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use slint::{Image, SharedString, StyledText};

use super::decode::{
    AvatarSlot, DecodeFailure, Decoded, MediaSlot, ShownAvatar, TimelineItemKey, load_avatar_async,
    load_thumbnail, peek_avatar, peek_thumbnail, record_avatar_need, record_custom_emoji_need,
    record_media_need, record_pack_cell_need,
};
use super::emote::slug;
use super::present::{
    Delivery, MessageKind, PollPhase, ServiceKind, ShieldReason, avatar_color_index,
    avatar_initials, decryption_failure, delivery, duration_label, file_extension, log_level_name,
    log_time_label, message_body_text, message_kind, message_sender_label, message_sent_at_label,
    message_timestamp_label, name_initials, poll_phase, preview_subject, pronoun_labels,
    reaction_key_label, reactor_labels, reader_labels, room_activity_label, sender_initial,
    service_kind, service_target, shield_reason, unsupported_kind, voice_bars, voter_labels,
};
use super::richtext::{self, Cover, InlineContent, InlineLine, InlinePiece, RoomWord, Spoilers};
use super::schema::{
    call_notices, call_offers, call_screens, call_tile_marks, define_ui_enum, define_ui_names,
    emoji_arts, media_failures, media_states, member_row_kinds, pack_surfaces, room_call_marks,
    room_row_kinds, sheet_previews, spoiler_states, test_phases, variant_named,
};
use super::spoilers::{keep_covered_body, spoilers_on};
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{
    CallFailure, CallNotice, CallPerson, CallTile, CantDecrypt, ChildAccess, DeviceOption,
    DeviceStatus, InviteeRow, InviteeState, JoinOffer, MissingMedia, RoomCard, RosterRow,
    ScreenView, SheetPreview, SpaceIndexRow, SpaceMatch, TestPhase, TileKind, TileState,
};
use crate::domain::call::ring::RingKind;
use crate::domain::departure::{Removal, RemovalKind};
use crate::domain::image_pack::{ImagePack, ImageUsage, PackId, PackImage, PackOrigin, PackTitle};
use crate::domain::invite::{Invite, InviteKind};
use crate::domain::media::{
    AudioKind, AudioMeta, ContentKey, FileMeta, MediaFailure, ThumbnailOutcome,
};
use crate::domain::member_name::{MemberName, isolated};
use crate::domain::message::{
    CustomEmoji, DecryptionFailure, MessageBody, MessagePreviewKind, Reaction, ReactionSend,
    Reactor, RichText, SendState, ServiceEvent, Shield, TimelineMessage,
};
use crate::domain::poll::{Poll, PollAnswer};
use crate::domain::room::{Room, Space};
use crate::domain::room_info::{MemberRole, Reader, RosterMember};
use crate::domain::room_log::{LogLevel, LogLine};
use crate::domain::room_preview::RoomPreview;
use crate::domain::sessions::OwnSession;
use crate::domain::space_index::{ChildKind, SpaceChild};
use crate::domain::timeline::EnrichmentDelta;
use crate::domain::user_info::{DeviceTrust, Pronouns, UserDevice, UserProfile};
use crate::ports::media::MediaCache;
use crate::util::format_bytes;

media_states!(define_ui_enum MediaState;);
media_states!(define_ui_names MediaState;);
media_failures!(define_ui_enum MediaFailureKind;);
member_row_kinds!(define_ui_enum MemberRowKind;);
room_row_kinds!(define_ui_enum RoomRowKind;);
room_call_marks!(define_ui_enum RoomCallMark;);
call_offers!(define_ui_enum CallOfferMark;);
sheet_previews!(define_ui_enum SheetPreviewMark;);
test_phases!(define_ui_enum TestPhaseMark;);
call_screens!(define_ui_enum CallScreenMark;);
call_tile_marks!(define_ui_enum CallTileMark;);
call_notices!(define_ui_enum CallNoticeMark;);
emoji_arts!(define_ui_enum EmojiArt;);
spoiler_states!(define_ui_enum SpoilerState;);
pack_surfaces!(define_ui_enum PackSurface;);
pack_surfaces!(variant_named pack_surface_named PackSurface;);

pub const REVEALABLE: Spoilers = Spoilers::Covered(Cover::Link);
pub const INVITED_HEADING_ROW: &str = "invited-heading";
pub const INVITES_HEADING_ROW: &str = "invites-heading";
pub const ROOMS_HEADING_ROW: &str = "rooms-heading";
pub const SPACES_HEADING_ROW: &str = "spaces-heading";
const PRONOUN_SEPARATOR: &str = " · ";

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

const EMOTE_SUGGESTION_LIMIT: usize = 20;
const SLUG_DELIMITER: char = '/';

#[derive(Clone, PartialEq, Eq)]
pub struct PackBrowse {
    pub surface: PackSurface,
    pub needle: String,
    pub columns: i32,
}

impl PackBrowse {
    pub fn new(surface: PackSurface, query: &str, columns: i32) -> Self {
        Self {
            surface,
            needle: pack_needle(query),
            columns: columns.max(1),
        }
    }
}

#[derive(Clone)]
pub struct PackCellDto {
    pub key: SharedString,
    pub pack_id: SharedString,
    pub shortcode: SharedString,
    pub label: SharedString,
    pub image: Option<Image>,
    pub media_state: MediaState,
    pub awaited_mxc: Option<String>,
}

pub struct PackRowDto {
    pub title: SharedString,
    pub untitled: bool,
    pub pack_id: SharedString,
    pub origin: PackOrigin,
    pub is_header: bool,
    pub cells: Vec<PackCellDto>,
}

pub struct PackTabDto {
    pub id: SharedString,
    pub title: SharedString,
    pub untitled: bool,
    pub header_row: i32,
    pub icon: Option<Image>,
    pub has_icon: bool,
    pub icon_cell_key: SharedString,
    pub avatar: Option<PackAvatarDto>,
}

pub struct PackAvatarDto {
    pub downloading_mxc: Option<String>,
    pub fallback_cell_key: SharedString,
}

struct PackIcon {
    image: Option<Image>,
    cell_key: SharedString,
    avatar: Option<PackAvatarDto>,
}

pub struct PackGrid {
    pub rows: Vec<PackRowDto>,
    pub packs: Vec<PackTabDto>,
    pub has_packs: bool,
}

pub const CELL_KEY_SEPARATOR: char = '\u{1}';

pub fn cell_key(pack: &PackId, shortcode: &str) -> String {
    format!("{pack}{CELL_KEY_SEPARATOR}{shortcode}")
}

pub fn cell_pack(key: &str) -> &str {
    key.split_once(CELL_KEY_SEPARATOR)
        .map_or(key, |(pack, _)| pack)
}

pub fn pack_needle(query: &str) -> String {
    query.trim().to_lowercase()
}

pub fn pack_grid(packs: &[ImagePack], browse: &PackBrowse, media: &dyn MediaCache) -> PackGrid {
    match browse.surface {
        PackSurface::StickerGrid => sectioned_grid(
            packs,
            &browse.needle,
            ImageUsage::offers_sticker,
            browse.columns,
            media,
        ),
        PackSurface::EmoteGrid => sectioned_grid(
            packs,
            &browse.needle,
            ImageUsage::offers_emoticon,
            browse.columns,
            media,
        ),
        PackSurface::EmoteSuggestions => suggestion_list(packs, &browse.needle, media),
    }
}

fn sectioned_grid(
    packs: &[ImagePack],
    needle: &str,
    offered: fn(ImageUsage) -> bool,
    columns: i32,
    media: &dyn MediaCache,
) -> PackGrid {
    let per_row = usize::try_from(columns.max(1)).unwrap_or(1);
    let mut grid = PackGrid {
        rows: Vec::new(),
        packs: Vec::new(),
        has_packs: packs
            .iter()
            .any(|pack| pack.images.iter().any(|image| offered(image.usage))),
    };

    for pack in packs {
        let whole_pack = needle.is_empty() || pack_title_matches(pack, needle);
        let cells: Vec<PackCellDto> = pack
            .images
            .iter()
            .filter(|image| offered(image.usage))
            .filter(|image| whole_pack || pack_image_matches(image, needle))
            .map(|image| pack_cell(pack, image, media))
            .collect();
        if cells.is_empty() {
            continue;
        }

        let id = SharedString::from(pack.id.as_ref());
        let (title, untitled) = pack_title(pack);
        let icon = pack_icon(pack, &cells, media);
        grid.packs.push(PackTabDto {
            id: id.clone(),
            title: title.clone(),
            untitled,
            header_row: i32::try_from(grid.rows.len()).unwrap_or(0),
            has_icon: icon.image.is_some(),
            icon: icon.image,
            icon_cell_key: icon.cell_key,
            avatar: icon.avatar,
        });
        grid.rows.push(PackRowDto {
            title,
            untitled,
            pack_id: id.clone(),
            origin: pack.origin,
            is_header: true,
            cells: Vec::new(),
        });
        for chunk in cells.chunks(per_row) {
            grid.rows.push(PackRowDto {
                title: SharedString::new(),
                untitled: false,
                pack_id: id.clone(),
                origin: pack.origin,
                is_header: false,
                cells: chunk.to_vec(),
            });
        }
    }

    grid
}

fn suggestion_list(packs: &[ImagePack], needle: &str, media: &dyn MediaCache) -> PackGrid {
    let (shortcode, wanted_pack) = needle
        .split_once(SLUG_DELIMITER)
        .map_or((needle, None), |(shortcode, pack)| (shortcode, Some(pack)));
    let mut prefixed = Vec::new();
    let mut inner = Vec::new();
    for pack in packs
        .iter()
        .filter(|pack| wanted_pack.is_none_or(|wanted| slug(pack).starts_with(wanted)))
    {
        for image in pack
            .images
            .iter()
            .filter(|image| image.usage.offers_emoticon())
        {
            let lowered = image.shortcode.to_lowercase();
            if lowered.starts_with(shortcode) {
                prefixed.push((pack, image));
            } else if lowered.contains(shortcode) {
                inner.push((pack, image));
            }
        }
    }
    let rows = prefixed
        .into_iter()
        .chain(inner)
        .take(EMOTE_SUGGESTION_LIMIT)
        .map(|(pack, image)| {
            let (title, untitled) = pack_title(pack);
            PackRowDto {
                title,
                untitled,
                pack_id: SharedString::from(pack.id.as_ref()),
                origin: pack.origin,
                is_header: false,
                cells: vec![pack_cell(pack, image, media)],
            }
        })
        .collect();
    PackGrid {
        rows,
        packs: Vec::new(),
        has_packs: packs.iter().any(|pack| {
            pack.images
                .iter()
                .any(|image| image.usage.offers_emoticon())
        }),
    }
}

fn pack_title(pack: &ImagePack) -> (SharedString, bool) {
    match &pack.title {
        PackTitle::Named(name) => (SharedString::from(name), false),
        PackTitle::Personal => (SharedString::new(), true),
    }
}

fn pack_title_matches(pack: &ImagePack, needle: &str) -> bool {
    match &pack.title {
        PackTitle::Named(name) => name.to_lowercase().contains(needle),
        PackTitle::Personal => false,
    }
}

fn pack_icon(pack: &ImagePack, cells: &[PackCellDto], media: &dyn MediaCache) -> PackIcon {
    let first = cells.first();
    let first_cell_key = first.map(|cell| cell.key.clone()).unwrap_or_default();
    let avatar_file = pack
        .avatar
        .as_ref()
        .map(|mxc| (mxc, media.sticker_path(mxc)));
    match avatar_file {
        Some((mxc, path)) if path.is_some() || !media.sticker_failed(mxc) => PackIcon {
            image: load_avatar_async(path.as_deref(), AvatarSlot::Pack(pack.id.to_string())),
            cell_key: SharedString::new(),
            avatar: Some(PackAvatarDto {
                downloading_mxc: path.is_none().then(|| mxc.clone()),
                fallback_cell_key: first_cell_key,
            }),
        },
        _ => PackIcon {
            image: first.and_then(|cell| cell.image.clone()),
            cell_key: first_cell_key,
            avatar: None,
        },
    }
}

fn pack_image_matches(image: &PackImage, needle: &str) -> bool {
    image.shortcode.to_lowercase().contains(needle) || image.body.to_lowercase().contains(needle)
}

pub enum PackArt {
    Ready(Image),
    Failed,
    Decoding,
    Downloading,
}

pub fn pack_art(key: &str, mxc: &str, media: &dyn MediaCache) -> PackArt {
    let path = media.sticker_path(mxc);
    record_pack_cell_need(key, path.as_deref());
    if let Some(path) = path {
        return match peek_thumbnail(&path, &MediaSlot::PackCell(key.to_owned())) {
            Decoded::Ready(decoded) => PackArt::Ready(decoded),
            Decoded::Failed(_) => PackArt::Failed,
            Decoded::Pending => PackArt::Decoding,
        };
    }
    if media.sticker_failed(mxc) {
        PackArt::Failed
    } else {
        PackArt::Downloading
    }
}

fn pack_cell(pack: &ImagePack, image: &PackImage, media: &dyn MediaCache) -> PackCellDto {
    let key = cell_key(&pack.id, &image.shortcode);
    let (art, media_state, awaited_mxc) = match pack_art(&key, &image.mxc, media) {
        PackArt::Ready(decoded) => (Some(decoded), MediaState::Ready, None),
        PackArt::Failed => (None, MediaState::Failed, None),
        PackArt::Decoding => (None, MediaState::Idle, None),
        PackArt::Downloading => (None, MediaState::Idle, Some(image.mxc.clone())),
    };
    PackCellDto {
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
    pub art: EmojiArt,
    pub image: Option<Image>,
    pub count: i32,
    pub mine: bool,
    pub send: ReactionSend,
    pub overflow: bool,
    pub reactors: SharedString,
    pub hidden_reactors: i32,
    pub avatars: Vec<ReactorAvatarDto>,
}

#[derive(Clone)]
pub struct BodyPieceDto {
    pub styled: StyledText,
    pub key: SharedString,
    pub art: EmojiArt,
    pub image: Option<Image>,
    pub spaced: bool,
}

#[derive(Clone)]
pub struct BodyLineDto {
    pub pieces: Vec<BodyPieceDto>,
    pub words: StyledText,
    pub gaps: i32,
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
    pub kind: RoomRowKind,
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
    pub space_rooms: i32,
    pub space_parent: SharedString,
    pub invite_count: i32,
    pub invite_kind: InviteKind,
    pub inviter: SharedString,
    pub inviter_id: SharedString,
    pub removal: RemovalKind,
    pub removed_by: SharedString,
    pub call: RoomCallMark,
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

pub struct MemberRowDto {
    pub user_id: SharedString,
    pub kind: MemberRowKind,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub role: MemberRole,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

pub struct InviteeDto {
    pub user_id: SharedString,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub state: InviteeState,
    pub failure: UserMessageKind,
    pub failure_detail: SharedString,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

#[allow(clippy::struct_excessive_bools)]
pub struct CallTileDto {
    pub key: SharedString,
    pub member: SharedString,
    pub user_id: SharedString,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub has_avatar: bool,
    pub avatar: Option<Image>,
    pub own: bool,
    pub state: CallTileMark,
    pub muted: bool,
    pub speaking: bool,
    pub feed: SharedString,
    pub has_frame: bool,
    pub frame: Option<Image>,
    pub source_width: i32,
    pub source_height: i32,
    pub mirrored: bool,
    pub screen: bool,
}

#[allow(clippy::struct_excessive_bools)]
pub struct CallPersonDto {
    pub key: SharedString,
    pub user_id: SharedString,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub has_avatar: bool,
    pub avatar: Option<Image>,
    pub you: bool,
    pub devices: i32,
    pub muted: bool,
    pub sharing: bool,
    pub sees_no_video: bool,
    pub exposes_e2ee: bool,
    pub state: CallTileMark,
}

pub struct CallDeviceDto {
    pub key: SharedString,
    pub name: SharedString,
    pub status: DeviceStatus,
    pub chosen: bool,
}

pub struct UserDeviceDto {
    pub device_id: SharedString,
    pub name: SharedString,
    pub trust: DeviceTrust,
    pub last_seen: SharedString,
    pub ip: SharedString,
}

pub struct LogLineDto {
    pub time: SharedString,
    pub level: LogLevel,
    pub level_name: SharedString,
    pub target: SharedString,
    pub text: SharedString,
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
        initial: SharedString::from(name_initials(&reactor.name, &reactor.user_id)),
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

fn downloaded_emoji_art(
    item: &TimelineItemKey,
    mxc: &str,
    media: &dyn MediaCache,
) -> (EmojiArt, Option<Image>) {
    let Some(path) = media.custom_emoji_path(mxc) else {
        return (EmojiArt::Pending, None);
    };
    record_custom_emoji_need(item, mxc, &path);
    match peek_thumbnail(&path, &MediaSlot::CustomEmoji(mxc.to_owned())) {
        Decoded::Ready(image) => (EmojiArt::Ready, Some(image)),
        Decoded::Failed(_) => (EmojiArt::Failed, None),
        Decoded::Pending => (EmojiArt::Pending, None),
    }
}

fn emoji_art(
    item: &TimelineItemKey,
    mxc: &str,
    state: CustomEmoji,
    media: &dyn MediaCache,
) -> (EmojiArt, Option<Image>) {
    match state {
        CustomEmoji::Downloading => (EmojiArt::Pending, None),
        CustomEmoji::Unavailable => (EmojiArt::Failed, None),
        CustomEmoji::Downloaded => downloaded_emoji_art(item, mxc, media),
    }
}

fn reaction_art(
    item: &TimelineItemKey,
    reaction: &Reaction,
    media: &dyn MediaCache,
) -> (EmojiArt, Option<Image>) {
    match reaction.image {
        None => (EmojiArt::Text, None),
        Some(state) => emoji_art(item, &reaction.key, state, media),
    }
}

fn reaction_dto(
    item: &TimelineItemKey,
    reaction: &Reaction,
    media: &dyn MediaCache,
) -> ReactionDto {
    let (reactors, hidden) = reactor_labels(reaction);
    let (art, image) = reaction_art(item, reaction, media);
    let label = match art {
        EmojiArt::Text => SharedString::from(reaction_key_label(&reaction.key)),
        EmojiArt::Pending | EmojiArt::Ready | EmojiArt::Failed => SharedString::new(),
    };
    ReactionDto {
        key: SharedString::from(&reaction.key),
        label,
        art,
        image,
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
        art: EmojiArt::Text,
        image: None,
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

pub fn rich_body(text: &RichText) -> richtext::StyledBody {
    match text.html() {
        Some(html) => richtext::styled_body(
            html,
            &text.plain,
            None,
            RoomWord::Text,
            Spoilers::Covered(Cover::Text),
        ),
        None => richtext::plain_body(&text.plain, None, RoomWord::Text),
    }
}

pub fn preview_line(text: &RichText) -> SharedString {
    match text.html() {
        Some(html) => one_line(
            &richtext::styled_body(html, &text.plain, None, RoomWord::Text, REVEALABLE).plain,
        ),
        None => one_line(&text.plain),
    }
}

pub fn message_body(message: &TimelineMessage) -> richtext::StyledBody {
    let actor = emote_actor(message);
    BodyText::of(message, actor.as_deref()).styled(REVEALABLE)
}

pub struct RowBody {
    pub rich: richtext::StyledBody,
    pub lines: Vec<BodyLineDto>,
    pub emote_only: bool,
    pub spoilers: SpoilerState,
}

struct BodyText<'a> {
    body: &'a MessageBody,
    actor: Option<&'a str>,
    room_word: RoomWord,
    emoji: &'a BTreeMap<String, CustomEmoji>,
}

fn emote_actor(message: &TimelineMessage) -> Option<String> {
    matches!(message.body, MessageBody::Emote(_)).then(|| message_sender_label(message))
}

impl<'a> BodyText<'a> {
    fn of(message: &'a TimelineMessage, actor: Option<&'a str>) -> Self {
        Self {
            body: &message.body,
            actor,
            room_word: RoomWord::when(message.mentions_room),
            emoji: &message.body_emoji,
        }
    }

    fn styled(&self, spoilers: Spoilers) -> richtext::StyledBody {
        let plain = message_body_text(self.body);
        match self.body.html() {
            Some(html) => richtext::styled_body(html, plain, self.actor, self.room_word, spoilers),
            None => richtext::plain_body(plain, self.actor, self.room_word),
        }
    }

    fn row_body(
        &self,
        item: &TimelineItemKey,
        spoilers: Spoilers,
        media: &dyn MediaCache,
    ) -> RowBody {
        let (lines, emote_only) = self.inline_lines(item, spoilers, media);
        let rich = self.styled(spoilers);
        RowBody {
            spoilers: spoiler_state(rich.has_spoilers, spoilers),
            rich,
            lines,
            emote_only,
        }
    }

    fn inline_lines(
        &self,
        item: &TimelineItemKey,
        spoilers: Spoilers,
        media: &dyn MediaCache,
    ) -> (Vec<BodyLineDto>, bool) {
        let drawn: BTreeSet<String> = self
            .emoji
            .iter()
            .filter(|(_, state)| **state != CustomEmoji::Unavailable)
            .map(|(mxc, _)| mxc.clone())
            .collect();
        if drawn.is_empty() {
            return (Vec::new(), false);
        }
        let Some(inline) = self.body.html().and_then(|html| {
            richtext::inline_body(html, self.actor, self.room_word, spoilers, drawn)
        }) else {
            return (Vec::new(), false);
        };
        let lines = inline
            .lines
            .iter()
            .map(|line| body_line_dto(item, line, self.emoji, media))
            .collect();
        (lines, inline.emote_only)
    }
}

#[derive(Clone)]
pub struct CoveredBody {
    body: MessageBody,
    actor: Option<String>,
    room_word: RoomWord,
    emoji: BTreeMap<String, CustomEmoji>,
}

impl CoveredBody {
    fn of(message: &TimelineMessage, actor: Option<String>) -> Self {
        Self {
            body: message.body.clone(),
            actor,
            room_word: RoomWord::when(message.mentions_room),
            emoji: message.body_emoji.clone(),
        }
    }

    fn text(&self) -> BodyText<'_> {
        BodyText {
            body: &self.body,
            actor: self.actor.as_deref(),
            room_word: self.room_word,
            emoji: &self.emoji,
        }
    }

    pub fn revealed(&self, item: &TimelineItemKey, media: &dyn MediaCache) -> RowBody {
        self.text().row_body(item, Spoilers::Shown, media)
    }
}

fn row_body_and_preview(
    item: &TimelineItemKey,
    message: &TimelineMessage,
    media: &dyn MediaCache,
) -> (RowBody, SharedString) {
    let actor = emote_actor(message);
    let text = BodyText::of(message, actor.as_deref());
    let spoilers = spoilers_on(&message.unique_id);
    let body = text.row_body(item, spoilers, media);
    if body.rich.has_spoilers {
        keep_covered_body(&message.unique_id, CoveredBody::of(message, actor.clone()));
    }
    let preview = match (preview_subject(&message.body), spoilers) {
        (Some(subject), _) => SharedString::from(subject),
        (None, Spoilers::Shown) => text.styled(REVEALABLE).plain,
        (None, Spoilers::Covered(_)) => body.rich.plain.clone(),
    };
    (body, one_line(&preview))
}

fn spoiler_state(has_spoilers: bool, spoilers: Spoilers) -> SpoilerState {
    match (has_spoilers, spoilers) {
        (false, _) => SpoilerState::None,
        (true, Spoilers::Covered(_)) => SpoilerState::Covered,
        (true, Spoilers::Shown) => SpoilerState::Shown,
    }
}

fn body_line_dto(
    item: &TimelineItemKey,
    line: &InlineLine,
    body_emoji: &BTreeMap<String, CustomEmoji>,
    media: &dyn MediaCache,
) -> BodyLineDto {
    BodyLineDto {
        pieces: line
            .pieces
            .iter()
            .map(|piece| body_piece_dto(item, piece, body_emoji, media))
            .collect(),
        words: line.words.clone(),
        gaps: count(line.gaps),
    }
}

fn body_piece_dto(
    item: &TimelineItemKey,
    piece: &InlinePiece,
    body_emoji: &BTreeMap<String, CustomEmoji>,
    media: &dyn MediaCache,
) -> BodyPieceDto {
    let (styled, key, (art, image)) = match &piece.content {
        InlineContent::Text(styled) => {
            (styled.clone(), SharedString::new(), (EmojiArt::Text, None))
        }
        InlineContent::Emoji(mxc) => {
            let state = body_emoji
                .get(mxc)
                .copied()
                .unwrap_or(CustomEmoji::Unavailable);
            (
                StyledText::default(),
                SharedString::from(mxc),
                emoji_art(item, mxc, state, media),
            )
        }
    };
    BodyPieceDto {
        styled,
        key,
        art,
        image,
        spaced: piece.spaced,
    }
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
        local_id: m.local_id.as_deref().map(SharedString::from).unwrap_or_default(),
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
        hidden_readers: count(hidden_readers),
        reader_count: count(m.read_by.total),
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
        service_reason: SharedString::from(
            m.body
                .service()
                .and_then(ServiceEvent::reason)
                .map(isolated)
                .unwrap_or_default(),
        ),
        image_width: 0,
        image_height: 0,
        duration: SharedString::new(),
        filename: SharedString::new(),
        size: SharedString::new(),
        map_link: SharedString::new(),
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
    let avatar_path = apply_sender_avatar(&mut dto, m, media);

    let thumbnail_undecoded = thumbnail_path.is_some() && dto.media_state == MediaState::Idle;
    let avatar_undecoded = avatar_path.is_some() && !dto.has_avatar;
    dto.needs_media = thumbnail_undecoded || avatar_undecoded;
    record_media_need(&item, thumbnail_path.as_deref(), avatar_path.as_deref());
    dto
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

pub fn room_to_dto(r: &Room, media: &dyn MediaCache) -> RoomDto {
    let mut dto = RoomDto {
        id: SharedString::from(r.id.as_ref()),
        kind: RoomRowKind::Room,
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
        last_message_sender: r
            .last_message_sender
            .as_ref()
            .map(|sender| SharedString::from(sender.shown()))
            .unwrap_or_default(),
        last_message_kind: r.last_message_kind,
        last_message_body: preview_line(&r.last_message_body),
        last_message_service_kind: r
            .last_message_service
            .as_ref()
            .map_or(ServiceKind::None, service_kind),
        last_message_service_target: SharedString::from(
            r.last_message_service
                .as_ref()
                .map(service_target)
                .unwrap_or_default(),
        ),
        last_message_is_own: r.last_message_is_own,
        last_message_edited: r.last_message_edited,
        last_message_time: SharedString::from(&room_activity_label(r.last_activity_ts)),
        space_rooms: 0,
        space_parent: SharedString::new(),
        invite_count: 0,
        invite_kind: InviteKind::Room,
        inviter: SharedString::new(),
        inviter_id: SharedString::new(),
        removal: RemovalKind::Kicked,
        removed_by: SharedString::new(),
        call: call_mark(r),
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

fn call_mark(room: &Room) -> RoomCallMark {
    match room.call.as_deref() {
        None => RoomCallMark::None,
        Some(call) if call.is_video() => RoomCallMark::Video,
        Some(_) => RoomCallMark::Voice,
    }
}

pub fn record_room_avatar_need(r: &Room, media: &dyn MediaCache) -> ShownAvatar {
    let avatar_path = r
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.room_avatar_path(mxc));
    record_avatar_need(
        &AvatarSlot::Room(r.id.as_ref().to_owned()),
        avatar_path.as_deref(),
    )
}

pub fn heading_to_dto(id: &str, kind: RoomRowKind, invites: usize) -> RoomDto {
    RoomDto {
        invite_count: count(invites),
        ..bare_row_dto(id, kind)
    }
}

pub fn invite_to_dto(invite: &Invite, media: &dyn MediaCache) -> RoomDto {
    let avatar = invite_avatar_path(invite, media).and_then(|path| peek_avatar(&path));
    let inviter = invite.inviter.as_ref();
    RoomDto {
        name: SharedString::from(&invite.name),
        initial: SharedString::from(avatar_initials(&invite.name)),
        color_index: avatar_color_index(invite.id.as_ref()),
        alert: true,
        last_message_time: SharedString::from(&room_activity_label(invite.received_ts)),
        invite_kind: invite.kind,
        inviter: inviter
            .map(|inviter| SharedString::from(inviter.shown()))
            .unwrap_or_default(),
        inviter_id: inviter
            .map(|inviter| SharedString::from(isolated(&inviter.user_id)))
            .unwrap_or_default(),
        has_avatar: avatar.is_some(),
        avatar,
        ..bare_row_dto(invite.id.as_ref(), RoomRowKind::Invite)
    }
}

pub fn invite_avatar_path(invite: &Invite, media: &dyn MediaCache) -> Option<PathBuf> {
    let mxc = invite.avatar_mxc.as_deref()?;
    if invite.is_space() {
        media.space_avatar_path(mxc)
    } else {
        media.room_avatar_path(mxc)
    }
}

pub fn removal_to_dto(removal: &Removal, media: &dyn MediaCache) -> RoomDto {
    let avatar = removal_avatar_path(removal, media).and_then(|path| peek_avatar(&path));
    RoomDto {
        name: SharedString::from(&removal.name),
        initial: SharedString::from(avatar_initials(&removal.name)),
        color_index: avatar_color_index(removal.id.as_ref()),
        last_message_time: SharedString::from(&room_activity_label(removal.removed_ts)),
        invite_kind: removal_shape(removal),
        removal: removal.kind,
        removed_by: SharedString::from(removal.by.shown()),
        has_avatar: avatar.is_some(),
        avatar,
        ..bare_row_dto(removal.id.as_ref(), RoomRowKind::Removal)
    }
}

pub fn removal_shape(removal: &Removal) -> InviteKind {
    InviteKind::of(removal.is_space, removal.is_direct)
}

pub fn removal_avatar_path(removal: &Removal, media: &dyn MediaCache) -> Option<PathBuf> {
    let mxc = removal.avatar_mxc.as_deref()?;
    if removal.is_space {
        media.space_avatar_path(mxc)
    } else {
        media.room_avatar_path(mxc)
    }
}

pub fn record_removal_avatar_need(removal: &Removal, media: &dyn MediaCache) -> ShownAvatar {
    let avatar_path = removal_avatar_path(removal, media);
    record_avatar_need(
        &AvatarSlot::Room(removal.id.as_ref().to_owned()),
        avatar_path.as_deref(),
    )
}

pub fn room_preview_avatar_path(room: &RoomPreview, media: &dyn MediaCache) -> Option<PathBuf> {
    let mxc = room.avatar_mxc.as_deref()?;
    if room.is_space {
        media.space_avatar_path(mxc)
    } else {
        media.room_avatar_path(mxc)
    }
}

pub fn record_invite_avatar_need(invite: &Invite, media: &dyn MediaCache) -> ShownAvatar {
    let avatar_path = invite_avatar_path(invite, media);
    record_avatar_need(
        &AvatarSlot::Room(invite.id.as_ref().to_owned()),
        avatar_path.as_deref(),
    )
}

pub fn space_match_to_dto(m: &SpaceMatch, media: &dyn MediaCache) -> RoomDto {
    let avatar = space_match_avatar_path(m, media).and_then(|path| peek_avatar(&path));
    RoomDto {
        name: SharedString::from(&m.name),
        initial: SharedString::from(avatar_initials(&m.name)),
        color_index: avatar_color_index(&m.id),
        alert: m.flags.alert,
        mention: m.flags.mention,
        hint: m.flags.hint,
        space_rooms: count(m.rooms),
        space_parent: SharedString::from(m.parent.as_deref().unwrap_or_default()),
        has_avatar: avatar.is_some(),
        avatar,
        ..bare_row_dto(&m.id, RoomRowKind::Space)
    }
}

fn bare_row_dto(id: &str, kind: RoomRowKind) -> RoomDto {
    RoomDto {
        id: SharedString::from(id),
        kind,
        name: SharedString::new(),
        initial: SharedString::new(),
        color_index: 0,
        members: 0,
        alert: false,
        mention: false,
        hint: false,
        muted: false,
        last_message_sender: SharedString::new(),
        last_message_kind: MessagePreviewKind::None,
        last_message_body: SharedString::new(),
        last_message_service_kind: ServiceKind::None,
        last_message_service_target: SharedString::new(),
        last_message_is_own: false,
        last_message_edited: false,
        last_message_time: SharedString::new(),
        space_rooms: 0,
        space_parent: SharedString::new(),
        invite_count: 0,
        invite_kind: InviteKind::Room,
        inviter: SharedString::new(),
        inviter_id: SharedString::new(),
        removal: RemovalKind::Kicked,
        removed_by: SharedString::new(),
        call: RoomCallMark::None,
        avatar: None,
        has_avatar: false,
    }
}

pub fn request_space_match_avatar(m: &SpaceMatch, media: &dyn MediaCache) -> ShownAvatar {
    let path = space_match_avatar_path(m, media);
    let slot = AvatarSlot::Space(m.id.clone());
    let shown = record_avatar_need(&slot, path.as_deref());
    load_avatar_async(path.as_deref(), slot);
    shown
}

fn space_match_avatar_path(m: &SpaceMatch, media: &dyn MediaCache) -> Option<PathBuf> {
    media.space_avatar_path(m.avatar_mxc.as_deref()?)
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

pub fn member_row_to_dto(row: &RosterRow, media: &dyn MediaCache) -> MemberRowDto {
    let RosterRow::Member(member) = row else {
        return MemberRowDto {
            user_id: SharedString::from(INVITED_HEADING_ROW),
            kind: MemberRowKind::InvitedHeading,
            name: SharedString::new(),
            initial: SharedString::new(),
            color_index: 0,
            role: MemberRole::Member,
            avatar: None,
            has_avatar: false,
        };
    };
    let avatar = member_avatar_path(member, media).and_then(|path| peek_avatar(&path));
    person_row(&member.user_id, &member.name, member.role, avatar)
}

fn person_row(
    user_id: &str,
    name: &MemberName,
    role: MemberRole,
    avatar: Option<Image>,
) -> MemberRowDto {
    MemberRowDto {
        user_id: SharedString::from(user_id),
        kind: MemberRowKind::Member,
        name: name
            .set()
            .map(|name| SharedString::from(isolated(name)))
            .unwrap_or_default(),
        initial: SharedString::from(name_initials(name, user_id)),
        color_index: avatar_color_index(user_id),
        role,
        has_avatar: avatar.is_some(),
        avatar,
    }
}

pub fn reader_to_dto(reader: &Reader, media: &dyn MediaCache) -> MemberRowDto {
    let avatar = reader_avatar_path(reader, media).and_then(|path| peek_avatar(&path));
    person_row(&reader.user_id, &reader.name, reader.role, avatar)
}

pub fn request_reader_avatar(reader: &Reader, media: &dyn MediaCache) {
    let path = reader_avatar_path(reader, media);
    load_avatar_async(path.as_deref(), AvatarSlot::Reader(reader.user_id.clone()));
}

fn reader_avatar_path(reader: &Reader, media: &dyn MediaCache) -> Option<PathBuf> {
    media.user_avatar_path(reader.avatar_mxc.as_deref()?)
}

pub fn mention_to_dto(member: &RosterMember, media: &dyn MediaCache) -> MemberRowDto {
    let avatar = member_avatar_path(member, media).and_then(|path| peek_avatar(&path));
    person_row(&member.user_id, &member.name, member.role, avatar)
}

pub fn request_mention_avatar(member: &RosterMember, media: &dyn MediaCache) {
    let path = member_avatar_path(member, media);
    load_avatar_async(path.as_deref(), AvatarSlot::Mention(member.user_id.clone()));
}

pub fn invitee_to_dto(row: &InviteeRow, media: &dyn MediaCache) -> InviteeDto {
    let person = &row.person;
    let avatar = invitee_avatar_path(row, media).and_then(|path| peek_avatar(&path));
    InviteeDto {
        user_id: SharedString::from(person.user_id.as_ref()),
        name: person
            .name
            .set()
            .map(|name| SharedString::from(isolated(name)))
            .unwrap_or_default(),
        initial: SharedString::from(name_initials(&person.name, &person.user_id)),
        color_index: avatar_color_index(&person.user_id),
        state: row.state,
        failure: row.failure.kind,
        failure_detail: SharedString::from(row.failure.detail.as_str()),
        has_avatar: avatar.is_some(),
        avatar,
    }
}

pub fn request_invitee_avatar(row: &InviteeRow, media: &dyn MediaCache) {
    let path = invitee_avatar_path(row, media);
    load_avatar_async(
        path.as_deref(),
        AvatarSlot::Invitee(row.person.user_id.to_string()),
    );
}

fn invitee_avatar_path(row: &InviteeRow, media: &dyn MediaCache) -> Option<PathBuf> {
    media.user_avatar_path(row.person.avatar_mxc.as_deref()?)
}

pub fn user_device_to_dto(device: &UserDevice) -> UserDeviceDto {
    UserDeviceDto {
        device_id: SharedString::from(device.device_id.as_str()),
        name: device
            .name
            .as_deref()
            .map_or_else(SharedString::new, SharedString::from),
        trust: device.trust,
        last_seen: SharedString::new(),
        ip: SharedString::new(),
    }
}

pub fn own_session_to_dto(session: &OwnSession) -> UserDeviceDto {
    UserDeviceDto {
        device_id: SharedString::from(session.device_id.as_str()),
        name: session
            .name
            .as_deref()
            .map_or_else(SharedString::new, SharedString::from),
        trust: session.trust,
        last_seen: session.last_seen_ms.map_or_else(SharedString::new, |at| {
            SharedString::from(room_activity_label(at))
        }),
        ip: session
            .last_seen_ip
            .as_deref()
            .map_or_else(SharedString::new, SharedString::from),
    }
}

pub fn call_tile_to_dto(tile: &CallTile) -> CallTileDto {
    CallTileDto {
        key: SharedString::from(tile.key().to_string()),
        member: SharedString::from(tile.member.to_string()),
        user_id: SharedString::from(tile.name.user_id.as_str()),
        name: SharedString::from(tile.name.shown()),
        initial: SharedString::from(name_initials(&tile.name.name, &tile.name.user_id)),
        color_index: avatar_color_index(&tile.name.user_id),
        has_avatar: false,
        avatar: None,
        own: tile.own,
        state: call_tile_mark(tile.state),
        muted: tile.muted,
        speaking: tile.speaking,
        feed: tile
            .video
            .as_ref()
            .map_or_else(SharedString::new, |feed| SharedString::from(feed.as_ref())),
        has_frame: false,
        frame: None,
        source_width: 0,
        source_height: 0,
        mirrored: tile.is_self_view(),
        screen: tile.kind == TileKind::Screen,
    }
}

pub fn call_person_to_dto(person: &CallPerson) -> CallPersonDto {
    CallPersonDto {
        key: person.tile.as_ref().map_or_else(SharedString::new, |tile| {
            SharedString::from(tile.to_string())
        }),
        user_id: SharedString::from(person.user.user_id.as_str()),
        name: SharedString::from(person.user.shown()),
        initial: SharedString::from(name_initials(&person.user.name, &person.user.user_id)),
        color_index: avatar_color_index(&person.user.user_id),
        has_avatar: false,
        avatar: None,
        you: person.you,
        devices: i32::try_from(person.devices).unwrap_or(i32::MAX),
        muted: person.muted,
        sharing: person.sharing,
        sees_no_video: person.sees_no_video,
        exposes_e2ee: person.exposes_e2ee,
        state: call_tile_mark(person.state),
    }
}

pub fn call_device_to_dto(option: &DeviceOption) -> CallDeviceDto {
    CallDeviceDto {
        key: option
            .key
            .as_ref()
            .map_or_else(SharedString::new, |key| SharedString::from(key.as_ref())),
        name: SharedString::from(&option.name),
        status: option.status,
        chosen: option.chosen,
    }
}

const fn call_tile_mark(state: TileState) -> CallTileMark {
    match state {
        TileState::Joining => CallTileMark::Joining,
        TileState::WaitingForKeys => CallTileMark::WaitingForKeys,
        TileState::Live => CallTileMark::Live,
        TileState::PoorConnection => CallTileMark::PoorConnection,
        TileState::NotReceived(MissingMedia::Audio) => CallTileMark::AudioNotReceived,
        TileState::NotReceived(MissingMedia::Video) => CallTileMark::VideoNotReceived,
        TileState::NotReceived(MissingMedia::AudioAndVideo) => CallTileMark::AudioVideoNotReceived,
        TileState::NotReceived(MissingMedia::Screen) => CallTileMark::ScreenNotReceived,
        TileState::NotReceived(MissingMedia::Other) => CallTileMark::MediaNotReceived,
        TileState::ConnectionLost => CallTileMark::ConnectionLost,
        TileState::CantDecrypt(CantDecrypt::NoKey) => CallTileMark::CantDecrypt,
        TileState::CantDecrypt(CantDecrypt::NotCrossSigned) => CallTileMark::UnverifiedDevice,
        TileState::Unreachable => CallTileMark::Unreachable,
    }
}

pub const fn call_screen_mark(screen: ScreenView) -> CallScreenMark {
    match screen {
        ScreenView::Off => CallScreenMark::Off,
        ScreenView::Choosing => CallScreenMark::Choosing,
        ScreenView::On => CallScreenMark::On,
        ScreenView::Failed => CallScreenMark::Failed,
        ScreenView::Unavailable(_) => CallScreenMark::Unavailable,
    }
}

pub const fn call_notice_mark(notice: Option<&CallNotice>) -> CallNoticeMark {
    match notice {
        None => CallNoticeMark::None,
        Some(CallNotice::NotAnnounced(RingKind::Ring)) => CallNoticeMark::NotCalled,
        Some(CallNotice::NotAnnounced(RingKind::Notify)) => CallNoticeMark::NotAnnounced,
        Some(CallNotice::MicrophoneFailed) => CallNoticeMark::MicrophoneFailed,
        Some(CallNotice::CameraFailed) => CallNoticeMark::CameraFailed,
        Some(CallNotice::ScreenFailed) => CallNoticeMark::ScreenFailed,
        Some(CallNotice::ChoosingScreen) => CallNoticeMark::ChoosingScreen,
        Some(CallNotice::SharingNeverStarted) => CallNoticeMark::SharingNeverStarted,
        Some(CallNotice::ConnectionPoor) => CallNoticeMark::ConnectionPoor,
        Some(CallNotice::NotReceived { what, .. }) => match what {
            MissingMedia::Audio => CallNoticeMark::AudioNotReceived,
            MissingMedia::Video => CallNoticeMark::VideoNotReceived,
            MissingMedia::AudioAndVideo => CallNoticeMark::AudioVideoNotReceived,
            MissingMedia::Screen => CallNoticeMark::ScreenNotReceived,
            MissingMedia::Other => CallNoticeMark::MediaNotReceived,
        },
        Some(CallNotice::CantDecrypt {
            why: CantDecrypt::NoKey,
            ..
        }) => CallNoticeMark::CantDecrypt,
        Some(CallNotice::CantDecrypt {
            why: CantDecrypt::NotCrossSigned,
            ..
        }) => CallNoticeMark::UnverifiedDevice,
    }
}

pub const fn call_failure_message(failure: CallFailure) -> UserMessageKind {
    match failure {
        CallFailure::ServerUnreachable => UserMessageKind::CallServerUnreachable,
        CallFailure::ServerRefused => UserMessageKind::CallServerRefused,
        CallFailure::JoinFailed => UserMessageKind::CallJoinFailed,
        CallFailure::MediaFailed => UserMessageKind::CallMediaFailed,
        CallFailure::CutOff => UserMessageKind::CallCutOff,
    }
}

pub const fn call_offer_mark(offer: Option<JoinOffer>) -> CallOfferMark {
    match offer {
        None => CallOfferMark::None,
        Some(JoinOffer::Start) => CallOfferMark::Start,
        Some(JoinOffer::Join) => CallOfferMark::Join,
        Some(JoinOffer::InCall) => CallOfferMark::InCall,
        Some(JoinOffer::Refused(_)) => CallOfferMark::Refused,
    }
}

pub const fn sheet_preview_mark(preview: Option<&SheetPreview>) -> SheetPreviewMark {
    match preview {
        None => SheetPreviewMark::None,
        Some(SheetPreview::Starting) => SheetPreviewMark::Starting,
        Some(SheetPreview::Local(_)) => SheetPreviewMark::Local,
        Some(SheetPreview::InCall { .. }) => SheetPreviewMark::InCall,
        Some(SheetPreview::Elsewhere { .. }) => SheetPreviewMark::Elsewhere,
        Some(SheetPreview::Failed(_)) => SheetPreviewMark::Failed,
    }
}

pub const fn test_phase_mark(phase: Option<TestPhase>) -> TestPhaseMark {
    match phase {
        None => TestPhaseMark::None,
        Some(TestPhase::Starting) => TestPhaseMark::Starting,
        Some(TestPhase::Running) => TestPhaseMark::Running,
        Some(TestPhase::Failed) => TestPhaseMark::Failed,
        Some(TestPhase::Lost) => TestPhaseMark::Lost,
    }
}

pub fn log_line_to_dto(line: &LogLine) -> LogLineDto {
    LogLineDto {
        time: SharedString::from(log_time_label(line.at)),
        level: line.level,
        level_name: SharedString::from(log_level_name(line.level)),
        target: SharedString::from(line.target),
        text: SharedString::from(line.text.as_str()),
    }
}

pub fn request_member_avatar(row: &RosterRow, media: &dyn MediaCache) {
    let RosterRow::Member(member) = row else {
        return;
    };
    let path = member_avatar_path(member, media);
    load_avatar_async(path.as_deref(), AvatarSlot::Member(member.user_id.clone()));
}

fn member_avatar_path(member: &RosterMember, media: &dyn MediaCache) -> Option<PathBuf> {
    media.user_avatar_path(member.avatar_mxc.as_deref()?)
}

pub fn load_room_info_avatar(card: &RoomCard, media: &dyn MediaCache) -> Option<Image> {
    let path = card
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.room_avatar_path(mxc));
    load_avatar_async(path.as_deref(), AvatarSlot::RoomInfo)
}

pub fn load_user_info_avatar(profile: &UserProfile, media: &dyn MediaCache) -> Option<Image> {
    let path = profile
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.user_avatar_path(mxc));
    load_avatar_async(path.as_deref(), AvatarSlot::UserInfo)
}

pub fn user_info_pronouns(profile: &UserProfile) -> SharedString {
    match &profile.pronouns {
        Pronouns::Known(pronouns) => {
            SharedString::from(pronoun_labels(pronouns).join(PRONOUN_SEPARATOR))
        }
        Pronouns::Unknown => SharedString::new(),
    }
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
