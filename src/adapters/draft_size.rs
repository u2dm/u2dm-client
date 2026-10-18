use mime::Mime;
use ruma::events::poll::unstable_start::{
    NewUnstablePollStartEventContent, ReplacementUnstablePollStartEventContent,
    UnstablePollStartEventContent,
};
use ruma::events::relation::Thread;
use ruma::events::room::message::{
    AddMentions, AudioInfo, AudioMessageEventContent, FileInfo, FileMessageEventContent,
    ForwardThread, ImageMessageEventContent, MessageType, Relation, ReplacementMetadata,
    ReplyMetadata, RoomMessageEventContent, RoomMessageEventContentWithoutRelation,
    TextMessageEventContent, UnstableAmplitude, UnstableAudioDetailsContentBlock, VideoInfo,
    VideoMessageEventContent,
};
use ruma::events::room::{EncryptedFile, ImageInfo, MediaSource, ThumbnailInfo};
use ruma::events::{AnyMessageLikeEventContent, Mentions};
use ruma::{OwnedEventId, OwnedMxcUri, OwnedUserId, UInt};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::markdown::{self, RoomMentions};
use super::poll_content;
use crate::domain::emote::Emote;
use crate::domain::event_size::{
    AES_BLOCK_BYTES, AES_KEY_BYTES, AUTH_EVENTS_MAX, ED25519_SIGNATURE_BYTES, Encryption, EventIds,
    EventShape, EventSize, ID_MAX_BYTES, JSON_INTEGER_MAX, PREV_EVENTS_MAX, PUBLIC_KEY_BYTES,
    SHA256_BYTES, Wrapping, unpadded_base64_len,
};
use crate::domain::media::OutgoingAttachment;
use crate::domain::message::{EditKind, EditTarget, MessageEdit};
use crate::domain::poll::PollDraft;

pub const MESSAGE_TYPE: &str = "m.room.message";
pub const POLL_START_TYPE: &str = "org.matrix.msc3381.poll.start";
pub const RELATION_FIELD: &str = "m.relates_to";

const ENCRYPTED_TYPE: &str = "m.room.encrypted";
const MEGOLM_ALGORITHM: &str = "m.megolm.v1.aes-sha2";
const ATTACHMENT_KEY_TYPE: &str = "oct";
const ATTACHMENT_KEY_OPERATIONS: [&str; 2] = ["encrypt", "decrypt"];
const ATTACHMENT_CIPHER: &str = "A256CTR";
const ATTACHMENT_FORMAT: &str = "v2";
const SIGNING_ALGORITHM: &str = "ed25519";
const SHORTEST_KEY_VERSION: &str = "0";
const SHORTEST_MEDIA_ID: &str = "0";
const LONGEST_THUMBNAIL_TYPE: Mime = mime::IMAGE_JPEG;
const EVENT_ID_SIGIL: char = '$';
const USER_ID_SIGIL: char = '@';
const SERVER_SEPARATOR: char = ':';
const FILLER: char = 'a';

#[derive(Debug, Clone, Copy)]
pub struct RoomFacts<'a> {
    pub room_id: &'a str,
    pub sender: &'a str,
    pub device_id: &'a str,
    pub encryption: Encryption,
    pub event_ids: EventIds,
    pub room_mentions: RoomMentions,
}

pub fn json_len(value: &impl Serialize) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |json| json.len())
}

pub fn event_size(content: &Value, event_type: &str, facts: &RoomFacts<'_>) -> EventSize {
    EventSize::of(
        shape(facts, event_type, content.get(RELATION_FIELD)),
        json_len(content),
    )
}

pub fn message(text: &str, emotes: &[Emote], facts: &RoomFacts<'_>) -> EventSize {
    sized(
        &RoomMessageEventContent::from(text_content(text, emotes, facts)),
        MESSAGE_TYPE,
        facts,
    )
}

pub fn reply(text: &str, emotes: &[Emote], replied_to: &str, facts: &RoomFacts<'_>) -> EventSize {
    measured(
        replying(text_content(text, emotes, facts), replied_to, facts),
        MESSAGE_TYPE,
        facts,
    )
}

pub fn edit(edit: &MessageEdit, facts: &RoomFacts<'_>) -> EventSize {
    match edit.kind {
        EditKind::Message => measured(revised(edit, facts), MESSAGE_TYPE, facts),
        EditKind::Caption => measured(recaptioned(edit, facts), MESSAGE_TYPE, facts),
    }
}

fn recaptioned(edit: &MessageEdit, facts: &RoomFacts<'_>) -> Option<RoomMessageEventContent> {
    if edit.body.is_empty() {
        return None;
    }
    let caption = text_content(&edit.body, &edit.emotes, facts);
    Some(match &edit.target {
        EditTarget::Sent(target) => caption.make_replacement(ReplacementMetadata::new(
            event_id_or_longest(target, facts.event_ids)?,
            Some(Mentions::new()),
        )),
        EditTarget::Queued(_) => caption.into(),
    })
}

fn revised(edit: &MessageEdit, facts: &RoomFacts<'_>) -> Option<RoomMessageEventContent> {
    let mut revised = text_content(&edit.body, &edit.emotes, facts);
    revised
        .mentions
        .get_or_insert_with(Mentions::new)
        .user_ids
        .insert(longest_user_id()?);
    Some(match &edit.target {
        EditTarget::Sent(target) => revised.make_replacement(ReplacementMetadata::new(
            event_id_or_longest(target, facts.event_ids)?,
            Some(Mentions::new()),
        )),
        EditTarget::Queued(_) => revised.with_relation(Some(Relation::Thread(Thread::plain(
            longest_event_id(facts.event_ids)?,
            longest_event_id(facts.event_ids)?,
        )))),
    })
}

pub fn poll_start(draft: &PollDraft, facts: &RoomFacts<'_>) -> EventSize {
    measured(
        poll_content::start_content(draft).ok(),
        POLL_START_TYPE,
        facts,
    )
}

pub fn poll_edit(draft: &PollDraft, target: &str, facts: &RoomFacts<'_>) -> EventSize {
    measured(
        poll_replacement(draft, target, facts),
        POLL_START_TYPE,
        facts,
    )
}

fn poll_replacement(
    draft: &PollDraft,
    target: &str,
    facts: &RoomFacts<'_>,
) -> Option<AnyMessageLikeEventContent> {
    let AnyMessageLikeEventContent::UnstablePollStart(UnstablePollStartEventContent::New(new)) =
        poll_content::start_content(draft).ok()?
    else {
        return None;
    };
    let NewUnstablePollStartEventContent {
        poll_start, text, ..
    } = new;
    Some(AnyMessageLikeEventContent::UnstablePollStart(
        UnstablePollStartEventContent::Replacement(
            ReplacementUnstablePollStartEventContent::plain_text(
                text.unwrap_or_default(),
                poll_start,
                event_id_or_longest(target, facts.event_ids)?,
            ),
        ),
    ))
}

pub struct MediaDraft<'a> {
    pub attachment: &'a OutgoingAttachment,
    pub content_type: &'a str,
    pub thumbnail: bool,
}

pub fn media(draft: &MediaDraft<'_>, facts: &RoomFacts<'_>) -> EventSize {
    measured(media_content(draft, facts), MESSAGE_TYPE, facts)
}

fn media_content(draft: &MediaDraft<'_>, facts: &RoomFacts<'_>) -> Option<RoomMessageEventContent> {
    let attachment = draft.attachment;
    let caption = attachment.caption.as_deref().map(|caption| {
        markdown::compose(
            caption,
            facts.sender,
            facts.room_mentions,
            &attachment.caption_emotes,
        )
    });
    let msgtype = media_type(
        draft,
        caption.as_ref().map(markdown::Composed::content),
        facts,
    )?;
    let mut content = RoomMessageEventContentWithoutRelation::new(msgtype);
    content.mentions = caption.map(|composed| composed.mentions);
    match attachment.reply_to.as_deref() {
        Some(replied_to) => replying(content, replied_to, facts),
        None => Some(content.into()),
    }
}

fn media_type(
    draft: &MediaDraft<'_>,
    caption: Option<TextMessageEventContent>,
    facts: &RoomFacts<'_>,
) -> Option<MessageType> {
    let picked = &draft.attachment.picked;
    let (body, formatted, filename) = match caption {
        Some(caption) => (
            caption.body,
            caption.formatted,
            Some(picked.filename.clone()),
        ),
        None => (picked.filename.clone(), None, None),
    };
    let source = media_source(facts)?;
    let thumbnail = if draft.thumbnail {
        Some((media_source(facts)?, Box::new(thumbnail_info())))
    } else {
        None
    };
    let (thumbnail_source, thumbnail_info) = thumbnail.unzip();
    let size = UInt::new(picked.size);
    let (width, height) = picked.dimensions.map_or((None, None), |(w, h)| {
        (UInt::new(w.into()), UInt::new(h.into()))
    });
    let mimetype = Some(draft.content_type.to_owned());
    let msgtype = match draft.content_type.split('/').next().unwrap_or_default() {
        "image" => {
            let mut info = ImageInfo::new();
            info.height = height;
            info.width = width;
            info.size = size;
            info.mimetype = mimetype;
            info.thumbnail_source = thumbnail_source;
            info.thumbnail_info = thumbnail_info;
            let mut content = ImageMessageEventContent::new(body, source);
            content.info = Some(Box::new(info));
            content.formatted = formatted;
            content.filename = filename;
            MessageType::Image(content)
        }
        "audio" => {
            let mut content = AudioMessageEventContent::new(body, source);
            content.formatted = formatted;
            content.filename = filename;
            if let (Some(duration), Some(waveform)) = (picked.duration, picked.waveform.as_ref()) {
                let waveform = waveform
                    .levels()
                    .iter()
                    .map(|level| {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let amplitude =
                            (level.clamp(0.0, 1.0) * f32::from(UnstableAmplitude::MAX)) as u16;
                        UnstableAmplitude::from(amplitude)
                    })
                    .collect();
                content.audio = Some(UnstableAudioDetailsContentBlock::new(duration, waveform));
            }
            let mut info = AudioInfo::new();
            info.duration = picked.duration;
            info.size = size;
            info.mimetype = mimetype;
            MessageType::Audio(content.info(Box::new(info)))
        }
        "video" => {
            let mut info = VideoInfo::new();
            info.duration = picked.duration;
            info.height = height;
            info.width = width;
            info.size = size;
            info.mimetype = mimetype;
            info.thumbnail_source = thumbnail_source;
            info.thumbnail_info = thumbnail_info;
            let mut content = VideoMessageEventContent::new(body, source);
            content.info = Some(Box::new(info));
            content.formatted = formatted;
            content.filename = filename;
            MessageType::Video(content)
        }
        _ => {
            let mut info = FileInfo::new();
            info.size = size;
            info.mimetype = mimetype;
            info.thumbnail_source = thumbnail_source;
            info.thumbnail_info = thumbnail_info;
            let mut content = FileMessageEventContent::new(body, source);
            content.info = Some(Box::new(info));
            content.formatted = formatted;
            content.filename = filename;
            MessageType::File(content)
        }
    };
    Some(msgtype)
}

fn media_source(facts: &RoomFacts<'_>) -> Option<MediaSource> {
    let url = OwnedMxcUri::from(format!(
        "mxc://{}/{SHORTEST_MEDIA_ID}",
        server_name(facts.sender)
    ));
    match facts.encryption {
        Encryption::Plain => Some(MediaSource::Plain(url)),
        Encryption::Megolm => {
            let file = json!({
                "url": url,
                "key": {
                    "kty": ATTACHMENT_KEY_TYPE,
                    "key_ops": ATTACHMENT_KEY_OPERATIONS,
                    "alg": ATTACHMENT_CIPHER,
                    "k": filler(unpadded_base64_len(AES_KEY_BYTES)),
                    "ext": true,
                },
                "iv": filler(unpadded_base64_len(AES_BLOCK_BYTES)),
                "hashes": { "sha256": filler(unpadded_base64_len(SHA256_BYTES)) },
                "v": ATTACHMENT_FORMAT,
            });
            serde_json::from_value::<EncryptedFile>(file)
                .ok()
                .map(|file| MediaSource::Encrypted(Box::new(file)))
        }
    }
}

fn thumbnail_info() -> ThumbnailInfo {
    let largest = UInt::new(JSON_INTEGER_MAX);
    let mut info = ThumbnailInfo::new();
    info.height = largest;
    info.width = largest;
    info.size = largest;
    info.mimetype = Some(LONGEST_THUMBNAIL_TYPE.to_string());
    info
}

fn text_content(
    text: &str,
    emotes: &[Emote],
    facts: &RoomFacts<'_>,
) -> RoomMessageEventContentWithoutRelation {
    let composed = markdown::compose(text, facts.sender, facts.room_mentions, emotes);
    RoomMessageEventContentWithoutRelation::new(MessageType::Text(composed.content()))
        .add_mentions(composed.mentions)
}

fn replying(
    content: RoomMessageEventContentWithoutRelation,
    replied_to: &str,
    facts: &RoomFacts<'_>,
) -> Option<RoomMessageEventContent> {
    let replied_to = event_id_or_longest(replied_to, facts.event_ids)?;
    let thread = Thread::plain(longest_event_id(facts.event_ids)?, replied_to.clone());
    Some(content.make_reply_to(
        ReplyMetadata::new(&replied_to, &longest_user_id()?, Some(&thread)),
        ForwardThread::Yes,
        AddMentions::Yes,
    ))
}

fn measured(content: Option<impl Serialize>, event_type: &str, facts: &RoomFacts<'_>) -> EventSize {
    match content {
        Some(content) => sized(&content, event_type, facts),
        None => unmeasured(event_type, facts),
    }
}

fn sized(content: &impl Serialize, event_type: &str, facts: &RoomFacts<'_>) -> EventSize {
    match serde_json::to_value(content) {
        Ok(content) => event_size(&content, event_type, facts),
        Err(_) => unmeasured(event_type, facts),
    }
}

fn unmeasured(event_type: &str, facts: &RoomFacts<'_>) -> EventSize {
    EventSize::of(shape(facts, event_type, None), 0)
}

fn shape(facts: &RoomFacts<'_>, event_type: &str, relation: Option<&Value>) -> EventShape {
    match facts.encryption {
        Encryption::Plain => EventShape {
            envelope: envelope(facts, event_type),
            wrapping: Wrapping::Plain,
        },
        Encryption::Megolm => EventShape {
            envelope: envelope(facts, ENCRYPTED_TYPE),
            wrapping: megolm_wrapping(facts, event_type, relation),
        },
    }
}

fn envelope(facts: &RoomFacts<'_>, wire_type: &str) -> usize {
    json_len(&pdu(facts, wire_type)).saturating_sub(json_len(&Map::new()))
}

fn pdu(facts: &RoomFacts<'_>, wire_type: &str) -> Value {
    let hash = filler(unpadded_base64_len(SHA256_BYTES));
    let mut key = Map::new();
    key.insert(
        format!("{SIGNING_ALGORITHM}:{SHORTEST_KEY_VERSION}"),
        Value::String(filler(unpadded_base64_len(ED25519_SIGNATURE_BYTES))),
    );
    let mut signatures = Map::new();
    signatures.insert(server_name(facts.sender).to_owned(), Value::Object(key));
    let mut pdu = json!({
        "content": {},
        "depth": JSON_INTEGER_MAX,
        "hashes": { "sha256": hash },
        "origin_server_ts": JSON_INTEGER_MAX,
        "room_id": facts.room_id,
        "sender": facts.sender,
        "signatures": signatures,
        "type": wire_type,
        "unsigned": { "age": JSON_INTEGER_MAX },
    });
    let reference = match facts.event_ids {
        EventIds::Hashed => json!(hashed_event_id()),
        EventIds::WithDomain => json!([domain_event_id(), { "sha256": hash }]),
    };
    if let Some(fields) = pdu.as_object_mut() {
        if facts.event_ids == EventIds::WithDomain {
            fields.insert("event_id".to_owned(), json!(domain_event_id()));
        }
        fields.insert(
            "auth_events".to_owned(),
            Value::Array(vec![reference.clone(); AUTH_EVENTS_MAX]),
        );
        fields.insert(
            "prev_events".to_owned(),
            Value::Array(vec![reference; PREV_EVENTS_MAX]),
        );
    }
    pdu
}

fn megolm_wrapping(facts: &RoomFacts<'_>, event_type: &str, relation: Option<&Value>) -> Wrapping {
    let payload = json!({ "type": event_type, "content": {}, "room_id": facts.room_id });
    let key = filler(unpadded_base64_len(PUBLIC_KEY_BYTES));
    let mut fields = json!({
        "algorithm": MEGOLM_ALGORITHM,
        "ciphertext": "",
        "device_id": facts.device_id,
        "sender_key": key,
        "session_id": key,
    });
    if let (Some(relation), Some(fields)) = (relation, fields.as_object_mut()) {
        fields.insert(RELATION_FIELD.to_owned(), relation.clone());
    }
    Wrapping::Megolm {
        payload: json_len(&payload).saturating_sub(json_len(&Map::new())),
        fields: json_len(&fields),
    }
}

fn server_name(user_id: &str) -> &str {
    user_id
        .split_once(SERVER_SEPARATOR)
        .map_or(user_id, |(_, server)| server)
}

fn filler(chars: usize) -> String {
    FILLER.to_string().repeat(chars)
}

fn hashed_event_id() -> String {
    format!(
        "{EVENT_ID_SIGIL}{}",
        filler(unpadded_base64_len(SHA256_BYTES))
    )
}

fn domain_event_id() -> String {
    let domain = format!("{SERVER_SEPARATOR}{FILLER}");
    format!(
        "{EVENT_ID_SIGIL}{}{domain}",
        filler(ID_MAX_BYTES.saturating_sub(EVENT_ID_SIGIL.len_utf8() + domain.len()))
    )
}

fn longest_event_id(event_ids: EventIds) -> Option<OwnedEventId> {
    let id = match event_ids {
        EventIds::Hashed => hashed_event_id(),
        EventIds::WithDomain => domain_event_id(),
    };
    OwnedEventId::try_from(id).ok()
}

fn event_id_or_longest(event_id: &str, event_ids: EventIds) -> Option<OwnedEventId> {
    OwnedEventId::try_from(event_id)
        .ok()
        .or_else(|| longest_event_id(event_ids))
}

fn longest_user_id() -> Option<OwnedUserId> {
    let server = format!("{SERVER_SEPARATOR}{FILLER}");
    let id = format!(
        "{USER_ID_SIGIL}{}{server}",
        filler(ID_MAX_BYTES.saturating_sub(USER_ID_SIGIL.len_utf8() + server.len()))
    );
    OwnedUserId::try_from(id).ok()
}
