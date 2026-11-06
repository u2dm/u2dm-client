use crate::domain::emote::Emote;
use crate::domain::message::MessageEdit;

#[derive(Clone, PartialEq, Eq)]
pub struct MessageDraft {
    pub body: String,
    pub reply: Option<ReplyDraft>,
    pub emotes: Vec<Emote>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReplyDraft {
    pub event_id: String,
    pub sender: String,
    pub preview: String,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Draft {
    Message(MessageDraft),
    Edit(MessageEdit),
}

impl Draft {
    pub fn body(&self) -> &str {
        match self {
            Self::Message(message) => &message.body,
            Self::Edit(edit) => &edit.body,
        }
    }

    pub fn emotes(&self) -> &[Emote] {
        match self {
            Self::Message(message) => &message.emotes,
            Self::Edit(edit) => &edit.emotes,
        }
    }
}
