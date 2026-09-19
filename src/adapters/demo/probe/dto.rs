use serde::Serialize;

use super::names;
use crate::commands::messages::UserMessage;
use crate::commands::view::{
    AppViewState, AttachmentView, AudioView, DirectoryView, LifecycleView, PaginationView,
    StickerView, Toast, TrackFile, UnsentMessage, VideoView,
};
use crate::domain::room::{Room, Space};
use crate::domain::sticker::StickerPack;
use crate::domain::sync::ConnectionStatus;

#[derive(Serialize)]
pub struct ViewDto {
    lifecycle: LifecycleDto,
    connection: ConnectionDto,
    directory: DirectoryDto,
    pagination: PaginationDto,
    stickers: StickersDto,
    attachment: AttachmentDto,
    video: VideoDto,
    audio: Option<NowPlayingDto>,
    unsent: Option<UnsentDto>,
    toast: ToastDto,
}

#[derive(Serialize)]
struct MessageDto {
    kind: &'static str,
    detail: String,
}

#[derive(Serialize)]
struct LifecycleDto {
    step: &'static str,
    activity: &'static str,
    method: &'static str,
    resolved_homeserver: String,
    user_id: String,
    has_avatar: bool,
    messages: Vec<MessageDto>,
}

#[derive(Serialize)]
struct ConnectionDto {
    status: &'static str,
    detail: Option<String>,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct RoomDto {
    id: String,
    name: String,
    is_direct: bool,
    member_count: u64,
    has_unread: bool,
    has_mentions: bool,
    has_activity: bool,
    muted: bool,
    alert: bool,
    mention: bool,
    hint: bool,
    last_activity_ts: u64,
    last_message_sender: Option<String>,
    last_message_body: String,
    last_message_is_own: bool,
    last_message_edited: bool,
}

#[derive(Serialize)]
struct SpaceDto {
    id: String,
    name: String,
    child_room_ids: Vec<String>,
    child_space_ids: Vec<String>,
    order: Option<String>,
    alert: bool,
    mention: bool,
    hint: bool,
}

#[derive(Serialize)]
struct DirectoryDto {
    space_id: String,
    subspace_id: String,
    rooms: Vec<RoomDto>,
    spaces: Vec<SpaceDto>,
    subspaces: Vec<SpaceDto>,
}

#[derive(Serialize)]
struct PaginationDto {
    generation: i32,
    backwards_loading: bool,
    forwards_loading: bool,
    new_messages: u32,
}

#[derive(Serialize)]
struct PackDto {
    id: String,
    title: String,
    shortcodes: Vec<String>,
}

#[derive(Serialize)]
struct StickersDto {
    generation: i32,
    ready_images: usize,
    room_encrypted: bool,
    loading: bool,
    packs: Vec<PackDto>,
}

#[derive(Serialize)]
struct AttachmentDto {
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
struct VideoDto {
    visible: bool,
    loading: bool,
    has_path: bool,
    error: &'static str,
}

#[derive(Serialize)]
struct UnsentDto {
    submission: i32,
    room_id: String,
    body: String,
    reply_to: Option<String>,
}

#[derive(Serialize)]
struct NowPlayingDto {
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

#[derive(Serialize)]
struct ToastDto {
    kind: &'static str,
    detail: Option<String>,
}

pub fn view(source: &AppViewState) -> ViewDto {
    ViewDto {
        lifecycle: lifecycle(&source.lifecycle),
        connection: connection(&source.connection),
        directory: directory(&source.directory),
        pagination: pagination(source.pagination),
        stickers: stickers(&source.stickers),
        attachment: attachment(&source.attachment),
        video: video(&source.video),
        audio: audio(&source.audio),
        unsent: source.unsent.as_ref().map(unsent),
        toast: toast(&source.toast),
    }
}

fn message(source: &UserMessage) -> MessageDto {
    MessageDto {
        kind: names::user_message_kind(source.kind),
        detail: source.detail.clone(),
    }
}

fn lifecycle(source: &LifecycleView) -> LifecycleDto {
    LifecycleDto {
        step: names::login_step(source.step),
        activity: names::login_activity(source.activity),
        method: names::login_method(source.method),
        resolved_homeserver: source.resolved_homeserver.clone(),
        user_id: source.user_id.clone(),
        has_avatar: source.avatar_path.is_some(),
        messages: source.messages.iter().map(message).collect(),
    }
}

fn connection(source: &ConnectionStatus) -> ConnectionDto {
    ConnectionDto {
        status: names::connection_status(source),
        detail: match source {
            ConnectionStatus::Error(detail) => Some(detail.clone()),
            _ => None,
        },
    }
}

fn room(source: &Room) -> RoomDto {
    RoomDto {
        id: source.id.to_string(),
        name: source.display_name.clone(),
        is_direct: source.is_direct,
        member_count: source.member_count,
        has_unread: source.has_unread,
        has_mentions: source.has_mentions,
        has_activity: source.has_activity,
        muted: source.muted(),
        alert: source.alert(),
        mention: source.mention(),
        hint: source.hint(),
        last_activity_ts: source.last_activity_ts,
        last_message_sender: source.last_message_sender.clone(),
        last_message_body: source.last_message_body.clone(),
        last_message_is_own: source.last_message_is_own,
        last_message_edited: source.last_message_edited,
    }
}

fn space(source: &Space) -> SpaceDto {
    SpaceDto {
        id: source.id.clone(),
        name: source.name.clone(),
        child_room_ids: source.child_room_ids.clone(),
        child_space_ids: source.child_space_ids.clone(),
        order: source.order.clone(),
        alert: source.alert,
        mention: source.mention,
        hint: source.hint,
    }
}

fn directory(source: &DirectoryView) -> DirectoryDto {
    DirectoryDto {
        space_id: source.space_id.clone(),
        subspace_id: source.subspace_id.clone(),
        rooms: source.rooms.iter().map(|entry| room(entry)).collect(),
        spaces: source.spaces.iter().map(space).collect(),
        subspaces: source.subspaces.iter().map(space).collect(),
    }
}

fn pagination(source: PaginationView) -> PaginationDto {
    PaginationDto {
        generation: source.generation,
        backwards_loading: source.backwards_loading,
        forwards_loading: source.forwards_loading,
        new_messages: source.new_messages,
    }
}

fn pack(source: &StickerPack) -> PackDto {
    PackDto {
        id: source.id.to_string(),
        title: source.title.clone(),
        shortcodes: source
            .images
            .iter()
            .map(|image| image.shortcode.clone())
            .collect(),
    }
}

fn stickers(source: &StickerView) -> StickersDto {
    StickersDto {
        generation: source.generation,
        ready_images: source.ready_images,
        room_encrypted: source.room_encrypted,
        loading: source.loading,
        packs: source.packs.iter().map(pack).collect(),
    }
}

fn attachment(source: &AttachmentView) -> AttachmentDto {
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

fn video(source: &VideoView) -> VideoDto {
    VideoDto {
        visible: source.visible,
        loading: source.loading,
        has_path: source.path.is_some(),
        error: names::user_message_kind(source.error),
    }
}

fn unsent(source: &UnsentMessage) -> UnsentDto {
    UnsentDto {
        submission: source.submission,
        room_id: source.room_id.to_string(),
        body: source.draft.body.clone(),
        reply_to: source
            .draft
            .reply
            .as_ref()
            .map(|reply| reply.event_id.clone()),
    }
}

fn audio(source: &AudioView) -> Option<NowPlayingDto> {
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

fn toast(source: &Toast) -> ToastDto {
    match source {
        Toast::None => ToastDto {
            kind: "none",
            detail: None,
        },
        Toast::Error(message) => ToastDto {
            kind: names::user_message_kind(message.kind),
            detail: Some(message.detail.clone()),
        },
        Toast::FileSaved(path) => ToastDto {
            kind: "file-saved",
            detail: Some(path.clone()),
        },
    }
}
