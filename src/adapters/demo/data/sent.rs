use std::collections::BTreeMap;

use super::history::{messages, own_sender_name};
use super::{MINUTE_MS, UNKNOWN_SENDER, data, own_user, pronouns};
use crate::adapters::demo::dto::RoomDto;
use crate::adapters::demo::{media, powers};
use crate::adapters::markdown::{self, Composed, RoomMentions};
use crate::domain::emote::Emote;
use crate::domain::image_pack::PackImage;
use crate::domain::media::{
    AudioKind, AudioMeta, FileMeta, ImageMeta, OutgoingAttachment, VideoMeta,
};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::message::{
    MessageBody, ReadBy, ReplyInfo, RichText, SendState, ServiceEvent, Shield, TimelineMessage,
};
use crate::domain::poll::{Poll, PollAnswer, PollDraft, PollStatus};
use crate::domain::room::{Room, RoomId};
use crate::util::unix_now_ms;

const SENT_STICKER_EXTENT: u32 = 512;

const STICKER_ASSET_MARKER: char = '#';

pub fn own_composition(composer: &str, emotes: &[Emote]) -> Composed {
    let notify_room = powers::messages().notify_room;
    markdown::compose(
        composer,
        own_user(),
        RoomMentions::when(notify_room),
        emotes,
    )
}

pub struct SentText {
    pub text: RichText,
    pub mentions_room: bool,
}

pub fn sent_text(composer: &str, emotes: &[Emote]) -> SentText {
    let sent = own_composition(composer, emotes);
    let html = sent.html.as_deref().map(markdown::sanitized);
    let text = markdown::own_text(&sent.markdown, html.as_deref());
    SentText {
        text,
        mentions_room: sent.mentions.room,
    }
}

pub fn own_message(
    sequence: u64,
    body: &str,
    emotes: &[Emote],
    reply: Option<ReplyInfo>,
    send_state: SendState,
) -> TimelineMessage {
    let id = format!("demo-sent-{sequence}");
    let settled = send_state == SendState::Sent;
    let sent = sent_text(body, emotes);
    let body = MessageBody::Text(sent.text);
    TimelineMessage {
        event_id: settled.then(|| id.clone()),
        local_id: (!settled).then(|| format!("local:{id}")),
        mentions_room: sent.mentions_room,
        reply,
        editable: true,
        send_state,
        ..own_post(&id, body)
    }
}

fn own_post(id: &str, body: MessageBody) -> TimelineMessage {
    TimelineMessage {
        sender_name: own_sender_name(),
        body_emoji: media::emoji_states(&body),
        is_own: true,
        ..posted(id, own_user(), unix_now_ms(), body)
    }
}

pub(in crate::adapters::demo) fn minutes_before(now_ms: u64, minutes: u64) -> u64 {
    now_ms.saturating_sub(minutes.saturating_mul(MINUTE_MS))
}

pub fn posted(id: &str, sender: &str, at: u64, body: MessageBody) -> TimelineMessage {
    TimelineMessage {
        unique_id: id.to_owned(),
        event_id: Some(id.to_owned()),
        local_id: None,
        sender: sender.to_owned(),
        sender_name: MemberName::Unset,
        sender_avatar_url: Some(sender.to_owned()),
        sender_pronouns: Vec::new(),
        body_emoji: BTreeMap::new(),
        body,
        mentions_room: false,
        timestamp: at,
        is_own: false,
        reply: None,
        edited: false,
        editable: false,
        is_first_unread: false,
        send_state: SendState::Sent,
        shield: Shield::None,
        reactions: Vec::new(),
        read_by: ReadBy::default(),
    }
}

pub fn own_poll(sequence: u64, draft: &PollDraft, send_state: SendState) -> TimelineMessage {
    let poll = Poll {
        question: draft.question().to_owned(),
        disclosure: draft.disclosure(),
        choice: draft.choice(),
        answers: draft
            .answers()
            .iter()
            .enumerate()
            .map(|(index, text)| PollAnswer {
                id: format!("demo-sent-{sequence}-answer-{index}"),
                text: text.clone(),
                voters: Vec::new(),
            })
            .collect(),
        status: PollStatus::Open,
        editable: true,
    };
    TimelineMessage {
        body: MessageBody::Poll(poll),
        mentions_room: false,
        ..own_message(sequence, draft.question(), &[], None, send_state)
    }
}

pub fn own_sticker(sequence: u64, image: &PackImage, reply: Option<ReplyInfo>) -> TimelineMessage {
    let id = format!(
        "demo-sent-{sequence}{STICKER_ASSET_MARKER}{}",
        media::mxc_asset(&image.mxc)
    );
    let body = MessageBody::Sticker {
        alt: image.body.clone(),
        meta: ImageMeta {
            width: Some(SENT_STICKER_EXTENT),
            height: Some(SENT_STICKER_EXTENT),
            mimetype: None,
            filename: None,
            thumbnail: Some(media::content_of(&id)),
        },
    };
    TimelineMessage {
        reply,
        ..own_post(&id, body)
    }
}

pub fn own_verification_request(sequence: u64, target: NamedUser) -> TimelineMessage {
    let id = format!("demo-sent-{sequence}-verification-request");
    let body = MessageBody::Service(ServiceEvent::VerificationRequested { target });
    own_post(&id, body)
}

pub fn own_attachment(
    sequence: u64,
    attachment: &OutgoingAttachment,
    reply: Option<ReplyInfo>,
) -> TimelineMessage {
    let id = format!("demo-sent-{sequence}");
    let picked = &attachment.picked;
    let sent = attachment
        .caption
        .as_deref()
        .map(|caption| sent_text(caption, &attachment.caption_emotes));
    let mentions_room = sent.as_ref().is_some_and(|sent| sent.mentions_room);
    let caption = || sent.as_ref().map(|sent| sent.text.clone());
    let (width, height) = picked.dimensions.unzip();
    let image_meta = || ImageMeta {
        width,
        height,
        mimetype: Some(picked.mimetype.clone()),
        filename: Some(picked.filename.clone()),
        thumbnail: Some(media::content_of(&id)),
    };
    let body = if attachment.as_document {
        MessageBody::File {
            caption: caption(),
            meta: FileMeta {
                filename: picked.filename.clone(),
                mimetype: Some(picked.mimetype.clone()),
                size: Some(picked.size),
            },
        }
    } else if picked.is_video() {
        MessageBody::Video {
            caption: caption(),
            meta: VideoMeta {
                image: image_meta(),
                duration: picked.duration,
                size: Some(picked.size),
            },
        }
    } else if picked.is_audio() {
        MessageBody::Audio {
            caption: caption(),
            meta: AudioMeta {
                kind: AudioKind::Track,
                file: media::content_of(&id),
                filename: picked.filename.clone(),
                mimetype: Some(picked.mimetype.clone()),
                duration: picked.duration,
                size: Some(picked.size),
                waveform: picked.waveform.clone(),
            },
        }
    } else if picked.is_image() {
        MessageBody::Image {
            caption: caption(),
            meta: image_meta(),
        }
    } else {
        MessageBody::File {
            caption: caption(),
            meta: FileMeta {
                filename: picked.filename.clone(),
                mimetype: Some(picked.mimetype.clone()),
                size: Some(picked.size),
            },
        }
    };
    TimelineMessage {
        mentions_room,
        reply,
        editable: true,
        ..own_post(&id, body)
    }
}

pub fn sticker_asset_in(event_id: &str) -> Option<&str> {
    event_id
        .split_once(STICKER_ASSET_MARKER)
        .map(|(_, asset)| asset)
}

pub fn body_preview(body: &MessageBody) -> RichText {
    match body {
        MessageBody::Text(text) | MessageBody::Notice(text) | MessageBody::Emote(text) => {
            text.clone()
        }
        MessageBody::Image { caption, .. }
        | MessageBody::Video { caption, .. }
        | MessageBody::Audio { caption, .. } => caption.clone().unwrap_or_default(),
        MessageBody::Sticker { alt, .. } => RichText::plain(alt.clone()),
        MessageBody::File { meta, .. } => RichText::plain(meta.filename.clone()),
        MessageBody::Location(location) => RichText::plain(location.description.clone()),
        MessageBody::Poll(poll) => RichText::plain(poll.question.clone()),
        MessageBody::Service(_) | MessageBody::UnableToDecrypt(_) => RichText::default(),
        MessageBody::Unsupported { fallback, .. } => RichText::plain(fallback.clone()),
    }
}

pub fn sender_of(message: &TimelineMessage) -> NamedUser {
    NamedUser::new(message.sender.clone(), message.sender_name.clone())
}

pub fn newest_other_sender(room_id: &RoomId) -> Option<NamedUser> {
    let own = own_user();
    messages(room_id)
        .iter()
        .rev()
        .find(|message| message.sender != own)
        .map(sender_of)
}

pub fn arrived_message(sequence: u64, sender: &NamedUser, body: MessageBody) -> TimelineMessage {
    let id = format!("demo-arrived-{sequence}");
    TimelineMessage {
        sender_name: sender.name.clone(),
        sender_pronouns: pronouns(&sender.user_id),
        ..posted(&id, &sender.user_id, unix_now_ms(), body)
    }
}

pub(super) fn last_message_only(room_id: &RoomId) -> Vec<TimelineMessage> {
    let Some(dto) = data().rooms.iter().find(|room| room.id == room_id.as_ref()) else {
        return Vec::new();
    };
    if dto.last_message.body.is_empty() {
        return Vec::new();
    }

    vec![synthesized_message(dto, &dto.to_room(unix_now_ms(), None))]
}

fn synthesized_message(dto: &RoomDto, room: &Room) -> TimelineMessage {
    let (sender, sender_name) = if dto.last_message.own {
        (own_user().to_owned(), own_sender_name())
    } else {
        let sender = dto
            .last_message
            .sender_id
            .clone()
            .unwrap_or_else(|| UNKNOWN_SENDER.to_owned());
        let name = room
            .last_message_sender
            .as_ref()
            .map(|named| named.name.clone())
            .unwrap_or_default();
        (sender, name)
    };
    let id = format!("demo-{}-last", dto.id.trim_start_matches('!'));
    let body = MessageBody::Text(room.last_message_body.clone());

    TimelineMessage {
        sender_pronouns: pronouns(&sender),
        sender_name,
        body_emoji: media::emoji_states(&body),
        is_own: dto.last_message.own,
        edited: room.last_message_edited,
        editable: dto.last_message.own,
        ..posted(&id, &sender, room.last_activity_ts, body)
    }
}
