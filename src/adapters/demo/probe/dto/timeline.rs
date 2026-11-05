use serde::Serialize;

use crate::adapters::demo::probe::names;
use crate::commands::ui::Draft;
use crate::commands::view::{
    AttachmentView, AudioView, CopiedLink, NotificationsView, PackView, PaginationView, PinnedView,
    SourceState, TrackFile, UnsentMessage, VideoView,
};
use crate::domain::event_size::Oversize;
use crate::domain::image_pack::{ImagePack, ImageUsage, PackOrigin, PackTitle};
use crate::domain::member_name::NamedUser;
use crate::domain::message::{EditKind, PinnedMessage};
use crate::domain::push::NoticeContent;
use crate::domain::timeline::{OlderHistory, SourceEncryption};

#[derive(Serialize)]
pub(super) struct NotificationDto {
    serial: u64,
    room_id: String,
    room_name: String,
    kind: &'static str,
    sender: Option<String>,
    highlight: bool,
    sound: bool,
}

#[derive(Serialize)]
pub(super) struct SourceDto {
    status: &'static str,
    event_id: Option<String>,
    encryption: Option<&'static str>,
    json: Option<String>,
    edit_json: Option<String>,
    encryption_json: Option<String>,
}

#[derive(Serialize)]
pub(super) struct LinkDto {
    serial: i32,
    url: String,
}

#[derive(Serialize)]
pub(super) struct PaginationDto {
    generation: i32,
    backwards_loading: bool,
    older_history: &'static str,
    forwards_loading: bool,
    new_messages: u32,
}

#[derive(Serialize)]
struct PinnedMessageDto {
    event_id: String,
    kind: &'static str,
    body: String,
}

#[derive(Serialize)]
pub(super) struct PinnedDto {
    room_id: Option<String>,
    shown: usize,
    messages: Vec<PinnedMessageDto>,
    pinned_ids: Vec<String>,
}

#[derive(Serialize)]
struct PackDto {
    id: String,
    title: String,
    untitled: bool,
    origin: &'static str,
    shortcodes: Vec<String>,
    stickers: Vec<String>,
}

#[derive(Serialize)]
pub(super) struct PacksDto {
    generation: i32,
    ready_images: usize,
    room_encrypted: bool,
    loading: bool,
    packs: Vec<PackDto>,
}

#[derive(Serialize)]
pub(super) struct AttachmentDto {
    visible: bool,
    filename: String,
    mimetype: String,
    size: u64,
    width: u32,
    height: u32,
    kind: &'static str,
    duration_ms: Option<u128>,
    has_preview: bool,
    sending: bool,
    error: &'static str,
    error_detail: String,
}

#[derive(Serialize)]
pub(super) struct VideoDto {
    visible: bool,
    loading: bool,
    has_path: bool,
    error: &'static str,
}

#[derive(Serialize)]
pub(super) struct UnsentDto {
    submission: i32,
    room_id: String,
    body: String,
    reply_to: Option<String>,
    edits: Option<String>,
    caption: bool,
    emotes: Vec<String>,
    shorten_by: Option<u8>,
}

#[derive(Serialize)]
pub(super) struct NowPlayingDto {
    request: u64,
    room_id: String,
    event_id: String,
    sender: String,
    kind: &'static str,
    title: String,
    duration_ms: Option<u128>,
    has_waveform: bool,
    downloaded: bool,
}

pub(super) fn notifications(source: &NotificationsView) -> Vec<NotificationDto> {
    source
        .shown
        .iter()
        .map(|shown| {
            let notice = &shown.notice;
            let (kind, sender) = match &notice.content {
                NoticeContent::Message { sender, .. } => ("message", sender.as_ref()),
                NoticeContent::Service { actor, .. } => ("service", Some(actor)),
                NoticeContent::Invite { inviter } => ("invite", inviter.as_ref()),
            };
            NotificationDto {
                serial: shown.serial,
                room_id: notice.room_id.to_string(),
                room_name: notice.room_name.clone(),
                kind,
                sender: sender.map(NamedUser::shown),
                highlight: notice.tweaks.highlight,
                sound: notice.tweaks.sound,
            }
        })
        .collect()
}

pub(super) fn link(source: &CopiedLink) -> LinkDto {
    LinkDto {
        serial: source.serial,
        url: source.url.clone(),
    }
}

pub(super) fn event_source(state: &SourceState) -> SourceDto {
    let status = names::source_status(state);
    match state {
        SourceState::Closed => SourceDto {
            status,
            event_id: None,
            encryption: None,
            json: None,
            edit_json: None,
            encryption_json: None,
        },
        SourceState::Locating { event_id } | SourceState::Unavailable { event_id } => SourceDto {
            status,
            event_id: Some(event_id.clone()),
            encryption: None,
            json: None,
            edit_json: None,
            encryption_json: None,
        },
        SourceState::Ready(source) => SourceDto {
            status,
            event_id: Some(source.event_id.clone()),
            encryption: Some(names::source_encryption(&source.encryption)),
            json: Some(source.json.clone()),
            edit_json: source.edit_json.clone(),
            encryption_json: match &source.encryption {
                SourceEncryption::Decrypted { details } => Some(details.clone()),
                SourceEncryption::Plain | SourceEncryption::Undecryptable => None,
            },
        },
    }
}

pub(super) fn pagination(source: PaginationView) -> PaginationDto {
    PaginationDto {
        generation: source.generation,
        backwards_loading: source.older_history == OlderHistory::Loading,
        older_history: older_history(source.older_history),
        forwards_loading: source.forwards_loading,
        new_messages: source.new_messages,
    }
}

fn older_history(source: OlderHistory) -> &'static str {
    match source {
        OlderHistory::Unknown => "unknown",
        OlderHistory::Available => "available",
        OlderHistory::Loading => "loading",
        OlderHistory::Failed => "failed",
        OlderHistory::Ended => "ended",
    }
}

fn pinned_message(source: &PinnedMessage) -> PinnedMessageDto {
    PinnedMessageDto {
        event_id: source.event_id.clone(),
        kind: names::preview_kind(source.kind),
        body: source.body.plain.clone(),
    }
}

pub(super) fn pinned(source: &PinnedView) -> PinnedDto {
    PinnedDto {
        room_id: source.room_id.as_ref().map(ToString::to_string),
        shown: source.shown,
        messages: source.messages.iter().map(pinned_message).collect(),
        pinned_ids: source.pinned_ids.iter().cloned().collect(),
    }
}

fn pack(source: &ImagePack) -> PackDto {
    let (title, untitled) = match &source.title {
        PackTitle::Named(name) => (name.clone(), false),
        PackTitle::Personal => (String::new(), true),
    };
    let origin = match source.origin {
        PackOrigin::Personal => "personal",
        PackOrigin::Room => "room",
        PackOrigin::Global => "global",
    };
    let shortcodes = |offered: fn(ImageUsage) -> bool| {
        source
            .images
            .iter()
            .filter(|image| offered(image.usage))
            .map(|image| image.shortcode.clone())
            .collect()
    };
    PackDto {
        id: source.id.to_string(),
        title,
        untitled,
        origin,
        shortcodes: shortcodes(|_| true),
        stickers: shortcodes(ImageUsage::offers_sticker),
    }
}

pub(super) fn image_packs(source: &PackView) -> PacksDto {
    PacksDto {
        generation: source.generation,
        ready_images: source.ready_images,
        room_encrypted: source.room_encrypted,
        loading: source.loading,
        packs: source.packs.iter().map(pack).collect(),
    }
}

pub(super) fn attachment(source: &AttachmentView) -> AttachmentDto {
    AttachmentDto {
        visible: source.visible,
        filename: source.filename.clone(),
        mimetype: source.mimetype.clone(),
        size: source.size,
        width: source.width,
        height: source.height,
        kind: names::attachment_kind(source.kind),
        duration_ms: source.duration.map(|value| value.as_millis()),
        has_preview: source.preview_path.is_some(),
        sending: source.sending,
        error: names::user_message_kind(source.error),
        error_detail: source.error_detail.clone(),
    }
}

pub(super) fn video(source: &VideoView) -> VideoDto {
    VideoDto {
        visible: source.visible,
        loading: source.loading,
        has_path: source.path.is_some(),
        error: names::user_message_kind(source.error),
    }
}

pub(super) fn unsent(source: &UnsentMessage) -> UnsentDto {
    let (reply_to, edits, caption) = match &source.draft {
        Draft::Message(message) => (
            message.reply.as_ref().map(|reply| reply.event_id.clone()),
            None,
            false,
        ),
        Draft::Edit(edit) => (
            None,
            edit.target
                .event_id()
                .or_else(|| edit.target.local_id())
                .map(str::to_owned),
            edit.kind == EditKind::Caption,
        ),
    };
    UnsentDto {
        submission: source.submission,
        room_id: source.room_id.to_string(),
        body: source.draft.body().to_owned(),
        reply_to,
        edits,
        caption,
        emotes: source
            .draft
            .emotes()
            .iter()
            .map(|emote| emote.token.clone())
            .collect(),
        shorten_by: source.oversize.map(Oversize::shorten_by),
    }
}

pub(super) fn audio(source: &AudioView) -> Option<NowPlayingDto> {
    let now = source.now_playing.as_ref()?;
    Some(NowPlayingDto {
        request: now.request,
        room_id: now.room_id.to_string(),
        event_id: now.event_id.clone(),
        sender: now.sender.clone(),
        kind: names::audio_kind(now.meta.kind),
        title: now.meta.filename.clone(),
        duration_ms: now.meta.duration.map(|value| value.as_millis()),
        has_waveform: now.meta.waveform.is_some(),
        downloaded: matches!(now.file, TrackFile::Ready(_)),
    })
}
