use matrix_sdk::ruma::events::poll::unstable_start::UnstablePollStartEventContent;
use matrix_sdk::ruma::events::room::message::{MessageFormat, MessageType};

use crate::domain::message::{MessagePreviewKind, RichText, ServiceEvent};

pub(super) struct MessagePreview {
    pub kind: MessagePreviewKind,
    pub body: RichText,
    pub service: Option<ServiceEvent>,
    pub edited: bool,
}

impl MessagePreview {
    pub(super) fn labelled(kind: MessagePreviewKind) -> Self {
        Self {
            kind,
            body: RichText::default(),
            service: None,
            edited: false,
        }
    }

    pub(super) fn service(event: ServiceEvent) -> Self {
        Self {
            kind: MessagePreviewKind::Text,
            body: RichText::default(),
            service: Some(event),
            edited: false,
        }
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn formatted_html(msgtype: &MessageType) -> Option<String> {
    let formatted = match msgtype {
        MessageType::Text(content) => content.formatted.as_ref(),
        MessageType::Notice(content) => content.formatted.as_ref(),
        MessageType::Emote(content) => content.formatted.as_ref(),
        _ => None,
    }?;
    (formatted.format == MessageFormat::Html).then(|| formatted.body.clone())
}

pub(super) fn from_msgtype(msgtype: &MessageType) -> MessagePreview {
    let (kind, body) = match msgtype {
        MessageType::Text(content) => (MessagePreviewKind::Text, content.body.as_str()),
        MessageType::Notice(content) => (MessagePreviewKind::Text, content.body.as_str()),
        MessageType::Emote(content) => (MessagePreviewKind::Text, content.body.as_str()),
        MessageType::Image(_) => return MessagePreview::labelled(MessagePreviewKind::Image),
        MessageType::Video(_) => return MessagePreview::labelled(MessagePreviewKind::Video),
        MessageType::Audio(content) if content.voice.is_some() => {
            return MessagePreview::labelled(MessagePreviewKind::Voice);
        }
        MessageType::Audio(_) => return MessagePreview::labelled(MessagePreviewKind::Audio),
        MessageType::File(content) => (
            MessagePreviewKind::File,
            content.filename.as_deref().unwrap_or(&content.body),
        ),
        MessageType::Location(_) => return MessagePreview::labelled(MessagePreviewKind::Location),
        other => (MessagePreviewKind::Text, other.body()),
    };
    MessagePreview {
        kind,
        body: RichText {
            plain: one_line(body),
            html: formatted_html(msgtype),
        },
        service: None,
        edited: false,
    }
}

pub(super) fn poll_start(content: &UnstablePollStartEventContent) -> MessagePreview {
    MessagePreview {
        kind: MessagePreviewKind::Poll,
        body: RichText::plain(one_line(&content.poll_start().question.text)),
        service: None,
        edited: matches!(content, UnstablePollStartEventContent::Replacement(_)),
    }
}
