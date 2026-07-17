use matrix_sdk::ruma::events::room::message::MessageType;

use crate::domain::models::{MessagePreviewKind, ServiceEvent};

pub(super) struct MessagePreview {
    pub kind: MessagePreviewKind,
    pub body: String,
    pub service: Option<ServiceEvent>,
    pub edited: bool,
}

impl MessagePreview {
    pub(super) fn labelled(kind: MessagePreviewKind) -> Self {
        Self {
            kind,
            body: String::new(),
            service: None,
            edited: false,
        }
    }

    pub(super) fn service(event: ServiceEvent) -> Self {
        Self {
            kind: MessagePreviewKind::Text,
            body: String::new(),
            service: Some(event),
            edited: false,
        }
    }
}

pub(super) fn from_msgtype(msgtype: &MessageType) -> MessagePreview {
    let (kind, body) = match msgtype {
        MessageType::Text(content) => (MessagePreviewKind::Text, content.body.as_str()),
        MessageType::Notice(content) => (MessagePreviewKind::Text, content.body.as_str()),
        MessageType::Emote(content) => (MessagePreviewKind::Text, content.body.as_str()),
        MessageType::Image(_) => return MessagePreview::labelled(MessagePreviewKind::Image),
        MessageType::Video(_) => return MessagePreview::labelled(MessagePreviewKind::Video),
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
        body: body.split_whitespace().collect::<Vec<_>>().join(" "),
        service: None,
        edited: false,
    }
}
