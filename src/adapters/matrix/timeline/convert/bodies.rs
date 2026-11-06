use std::collections::BTreeMap;

use matrix_sdk::ruma::UInt;
use matrix_sdk::ruma::events::Mentions;
use matrix_sdk::ruma::events::poll::start::PollKind;
use matrix_sdk::ruma::events::room::ImageInfo;
use matrix_sdk::ruma::events::room::message::{
    AudioMessageEventContent, FileMessageEventContent, FormattedBody, ImageMessageEventContent,
    MessageFormat, MessageType, UnstableAmplitude, VideoInfo, VideoMessageEventContent,
};
use matrix_sdk::ruma::events::sticker::StickerEventContent;
use matrix_sdk::ruma::html::{HtmlSanitizerMode, RemoveReplyFallback, sanitize_html};
use matrix_sdk_ui::timeline::{EventTimelineItem, PollState};
use serde::Deserialize;

use crate::adapters::matrix::media::{EventMedia, file_content, thumbnail_content};
use crate::adapters::matrix::timeline::TimelineContext;
use crate::adapters::{body_emoji, markdown};
use crate::domain::location::Location;
use crate::domain::media::{
    AudioKind, AudioMeta, FileMeta, ImageMeta, MediaKind, VideoMeta, Waveform,
};
use crate::domain::message::{CustomEmoji, MessageBody, RichText, ServiceEvent};
use crate::domain::poll::{Poll, PollAnswer, PollChoice, PollDisclosure, PollStatus, Voter};

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

pub(super) fn extract_sticker_body(sticker: &StickerEventContent) -> MessageBody {
    let mut meta = image_meta(&sticker.info);
    meta.thumbnail = thumbnail_content(MediaKind::Sticker, &meta, EventMedia::of_sticker(sticker));
    MessageBody::Sticker {
        alt: sticker.body.clone(),
        meta,
    }
}

pub(super) fn extract_poll_body(
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

pub(super) fn name_target(mut service: ServiceEvent, ctx: &TimelineContext<'_>) -> ServiceEvent {
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
        caption: file
            .caption()
            .map(|caption| rich_body(caption, file.formatted_caption())),
        meta: FileMeta {
            filename: file.filename().to_owned(),
            mimetype,
            size,
        },
    }
}

fn rich_body(plain: &str, formatted: Option<&FormattedBody>) -> RichText {
    let html = formatted
        .filter(|formatted| formatted.format == MessageFormat::Html)
        .map(|formatted| formatted.body.clone());
    RichText::with_html(plain.to_owned(), html)
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

pub(super) fn emoji_states(
    body: &MessageBody,
    ctx: &TimelineContext<'_>,
) -> BTreeMap<String, CustomEmoji> {
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

pub(super) fn with_emoticons_marked(
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

pub(super) fn names_room(mentions: Option<&Mentions>) -> bool {
    mentions.is_some_and(|mentions| mentions.room)
}

pub(super) fn is_own(event: &EventTimelineItem, ctx: &TimelineContext<'_>) -> bool {
    ctx.own_user_id
        .is_some_and(|own| own == event.sender().as_str())
}

pub(super) fn authored(body: MessageBody) -> MessageBody {
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
        MessageBody::File { caption, meta } => MessageBody::File {
            caption: caption.map(markdown::own_caption),
            meta,
        },
        other => other,
    }
}

pub(super) fn message_type_to_body(msgtype: &MessageType) -> MessageBody {
    match msgtype {
        MessageType::Text(t) => MessageBody::Text(rich_body(&t.body, t.formatted.as_ref())),
        MessageType::Notice(n) => MessageBody::Notice(rich_body(&n.body, n.formatted.as_ref())),
        MessageType::Emote(e) => MessageBody::Emote(rich_body(&e.body, e.formatted.as_ref())),
        MessageType::Image(i) => extract_image_body(i),
        MessageType::Video(v) => extract_video_body(v),
        MessageType::Audio(a) => extract_audio_body(a),
        MessageType::File(f) => extract_file_body(f),
        MessageType::Location(l) => MessageBody::Location(Location {
            description: l.body.clone(),
            geo_uri: l.geo_uri.clone(),
        }),
        MessageType::ServerNotice(n) => MessageBody::Notice(RichText::plain(n.body.clone())),
        other => MessageBody::Unsupported {
            kind: other.msgtype().to_string(),
            fallback: other.body().to_string(),
        },
    }
}
