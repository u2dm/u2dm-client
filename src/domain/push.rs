use crate::domain::member_name::NamedUser;
use crate::domain::message::{MessagePreviewKind, RichText, ServiceEvent};
use crate::domain::room::RoomId;
use crate::domain::settings::Alerts;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PushTweaks {
    pub highlight: bool,
    pub sound: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeContent {
    Message {
        sender: Option<NamedUser>,
        kind: MessagePreviewKind,
        body: RichText,
    },
    HiddenMessage,
    Service {
        actor: NamedUser,
        event: ServiceEvent,
    },
    Invite {
        inviter: Option<NamedUser>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushNotice {
    pub room_id: RoomId,
    pub room_name: String,
    pub sent_at: u64,
    pub content: NoticeContent,
    pub tweaks: PushTweaks,
}

impl PushNotice {
    pub fn is_invite(&self) -> bool {
        matches!(self.content, NoticeContent::Invite { .. })
    }

    pub fn is_call(&self) -> bool {
        matches!(
            self.content,
            NoticeContent::Service {
                event: ServiceEvent::CallNotification { .. },
                ..
            }
        )
    }

    pub fn is_older_than(&self, other: &Self) -> bool {
        self.sent_at < other.sent_at
    }

    pub fn shaped_by(mut self, alerts: Alerts) -> Self {
        self.tweaks.sound &= alerts.notification_sound;
        if alerts.hide_message_text && matches!(self.content, NoticeContent::Message { .. }) {
            self.content = NoticeContent::HiddenMessage;
        }
        self
    }
}
