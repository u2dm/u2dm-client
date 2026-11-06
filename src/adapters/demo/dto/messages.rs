use std::time::Duration;

use serde::Deserialize;

use super::rooms::{JoinRuleDto, KindDto};
use super::{ago_ms, rich_text};
use crate::adapters::demo::{data, media, reactions};
use crate::domain::media::{AudioKind, AudioMeta, ImageMeta, VideoMeta, Waveform};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::message::{
    MessageBody, Reaction, ReactionSend, ReadBy, ReplyInfo, RichText, SendState, ServiceEvent,
    Shield, TimelineMessage,
};
use crate::domain::poll::{Poll, PollAnswer, PollChoice, PollDisclosure, PollStatus, Voter};
use crate::domain::room_state::{GuestAccess, HistoryVisibility, PowerChange, RoleChange};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageDto {
    id: String,
    sender: String,
    name: String,
    #[serde(default)]
    body: String,
    html: Option<String>,
    #[serde(default)]
    minutes_ago: u64,
    #[serde(default)]
    days_ago: u64,
    #[serde(default)]
    edited: bool,
    #[serde(default)]
    mentions_room: bool,
    image: Option<ImageDto>,
    sticker: Option<StickerDto>,
    video: Option<VideoDto>,
    audio: Option<AudioDto>,
    poll: Option<PollDto>,
    reply: Option<ReplyDto>,
    service: Option<ServiceDto>,
    #[serde(default)]
    reactions: Vec<ReactionDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PollDto {
    #[serde(default)]
    kind: PollKindDto,
    #[serde(default)]
    max: usize,
    #[serde(default)]
    ended: bool,
    answers: Vec<PollAnswerDto>,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum PollKindDto {
    #[default]
    Disclosed,
    Undisclosed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PollAnswerDto {
    id: String,
    text: String,
    #[serde(default)]
    voters: Vec<String>,
}

impl PollDto {
    fn to_poll(&self, question: &str, own_user: &str, is_own: bool) -> Poll {
        let answered = self.answers.iter().any(|answer| !answer.voters.is_empty());
        Poll {
            question: question.to_owned(),
            disclosure: match self.kind {
                PollKindDto::Disclosed => PollDisclosure::Disclosed,
                PollKindDto::Undisclosed => PollDisclosure::Undisclosed,
            },
            choice: PollChoice::up_to(self.max),
            answers: self
                .answers
                .iter()
                .map(|answer| PollAnswer {
                    id: answer.id.clone(),
                    text: answer.text.clone(),
                    voters: answer
                        .voters
                        .iter()
                        .map(|voter| Voter::new(voter.clone(), Some(own_user)))
                        .collect(),
                })
                .collect(),
            status: if self.ended {
                PollStatus::Ended
            } else {
                PollStatus::Open
            },
            editable: is_own && !self.ended && !answered,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactionDto {
    key: String,
    #[serde(default)]
    senders: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageDto {
    width: u32,
    height: u32,
    mimetype: Option<String>,
    filename: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VideoDto {
    width: u32,
    height: u32,
    #[serde(default)]
    duration_secs: u64,
    mimetype: Option<String>,
    filename: Option<String>,
    size: Option<u64>,
}

impl VideoDto {
    fn to_meta(&self, id: &str) -> VideoMeta {
        VideoMeta {
            image: ImageMeta {
                width: Some(self.width),
                height: Some(self.height),
                mimetype: Some(
                    self.mimetype
                        .clone()
                        .unwrap_or_else(|| "video/mp4".to_owned()),
                ),
                filename: self.filename.clone(),
                thumbnail: Some(media::content_of(id)),
            },
            duration: (self.duration_secs > 0).then(|| Duration::from_secs(self.duration_secs)),
            size: self.size,
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum AudioKindDto {
    Voice,
    Track,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioDto {
    kind: AudioKindDto,
    #[serde(default)]
    duration_secs: u64,
    mimetype: Option<String>,
    filename: Option<String>,
    size: Option<u64>,
    #[serde(default)]
    waveform: Vec<u16>,
}

impl AudioDto {
    fn to_meta(&self, id: &str) -> AudioMeta {
        let (kind, filename, mimetype) = match self.kind {
            AudioKindDto::Voice => (AudioKind::Voice, "voice-message.ogg", "audio/ogg"),
            AudioKindDto::Track => (AudioKind::Track, "audio.m4a", "audio/mp4"),
        };
        AudioMeta {
            kind,
            file: media::content_of(id),
            filename: self.filename.clone().unwrap_or_else(|| filename.to_owned()),
            mimetype: Some(self.mimetype.clone().unwrap_or_else(|| mimetype.to_owned())),
            duration: (self.duration_secs > 0).then(|| Duration::from_secs(self.duration_secs)),
            size: self.size,
            waveform: Waveform::from_amplitudes(self.waveform.iter().copied()),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StickerDto {
    width: u32,
    height: u32,
    #[serde(default)]
    animated: bool,
}

impl StickerDto {
    fn to_meta(&self, id: &str) -> ImageMeta {
        let mimetype = if self.animated {
            "image/webp"
        } else {
            "image/png"
        };
        ImageMeta {
            width: Some(self.width),
            height: Some(self.height),
            mimetype: Some(mimetype.to_owned()),
            filename: None,
            thumbnail: Some(media::content_of(id)),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(super) enum ServiceDto {
    Joined,
    Left,
    Invited {
        #[serde(default)]
        target: String,
    },
    Kicked {
        #[serde(default)]
        target: String,
        #[serde(default)]
        reason: Option<String>,
    },
    Banned {
        #[serde(default)]
        target: String,
        #[serde(default)]
        reason: Option<String>,
    },
    Knocked,
    KnockRetracted,
    KnockDenied {
        #[serde(default)]
        target: String,
    },
    ThirdPartyInvite {
        #[serde(default)]
        target: String,
    },
    ThirdPartyRevoked {
        #[serde(default)]
        target: String,
    },
    NameChanged {
        #[serde(default)]
        target: String,
    },
    AvatarChanged,
    RoomName {
        #[serde(default)]
        target: String,
    },
    RoomNameRemoved,
    RoomAvatar,
    RoomCreated,
    UpgradedRoomCreated,
    RoomUpgraded,
    Encryption,
    JoinRules {
        rule: JoinRuleDto,
    },
    History {
        visibility: HistoryDto,
    },
    Guests {
        access: GuestsDto,
    },
    ServerAcl,
    MainAddress {
        #[serde(default)]
        target: String,
    },
    MainAddressRemoved,
    AltAddresses,
    Pinned,
    Unpinned,
    PinsChanged,
    Role {
        #[serde(default)]
        target: String,
        change: RoleChangeDto,
    },
    PowerLevel {
        #[serde(default)]
        target: String,
    },
    PowerLevels,
    CallStarted,
    CallNotification {
        #[serde(default)]
        video: bool,
        #[serde(default)]
        target: String,
    },
}

impl ServiceDto {
    pub(super) fn to_event(&self) -> ServiceEvent {
        match self {
            Self::Joined => ServiceEvent::Joined,
            Self::Left => ServiceEvent::Left,
            Self::Invited { target } => ServiceEvent::Invited {
                target: named_only(target),
            },
            Self::Kicked { target, reason } => ServiceEvent::Kicked {
                target: named_only(target),
                reason: reason.clone(),
            },
            Self::Banned { target, reason } => ServiceEvent::Banned {
                target: named_only(target),
                reason: reason.clone(),
            },
            Self::Knocked => ServiceEvent::Knocked,
            Self::KnockRetracted => ServiceEvent::KnockRetracted,
            Self::KnockDenied { target } => ServiceEvent::KnockDenied {
                target: named_only(target),
            },
            Self::ThirdPartyInvite { target } => ServiceEvent::ThirdPartyInvited {
                name: target.clone(),
            },
            Self::ThirdPartyRevoked { target } => ServiceEvent::ThirdPartyInviteRevoked {
                name: target.clone(),
            },
            Self::NameChanged { target } => ServiceEvent::DisplayNameChanged {
                name: target.clone(),
            },
            Self::AvatarChanged => ServiceEvent::AvatarChanged,
            Self::RoomName { target } => ServiceEvent::RoomNameChanged {
                name: target.clone(),
            },
            Self::RoomNameRemoved => ServiceEvent::RoomNameRemoved,
            Self::RoomAvatar => ServiceEvent::RoomAvatarChanged,
            Self::RoomCreated => ServiceEvent::RoomCreated,
            Self::UpgradedRoomCreated => ServiceEvent::UpgradedRoomCreated,
            Self::RoomUpgraded => ServiceEvent::RoomUpgraded,
            Self::Encryption => ServiceEvent::EncryptionEnabled,
            Self::JoinRules { rule } => ServiceEvent::JoinRuleChanged(rule.to_rule(Vec::new())),
            Self::History { visibility } => {
                ServiceEvent::HistoryVisibilityChanged(visibility.to_visibility())
            }
            Self::Guests { access } => ServiceEvent::GuestAccessChanged(access.to_access()),
            Self::ServerAcl => ServiceEvent::ServerAclChanged,
            Self::MainAddress { target } => ServiceEvent::MainAddressSet {
                alias: target.clone(),
            },
            Self::MainAddressRemoved => ServiceEvent::MainAddressRemoved,
            Self::AltAddresses => ServiceEvent::AlternativeAddressesChanged,
            Self::Pinned => ServiceEvent::MessagePinned,
            Self::Unpinned => ServiceEvent::MessageUnpinned,
            Self::PinsChanged => ServiceEvent::PinnedMessagesChanged,
            Self::Role { target, change } => ServiceEvent::PowerLevelsChanged(PowerChange::Role {
                target: named_only(target),
                change: change.to_change(),
            }),
            Self::PowerLevel { target } => ServiceEvent::PowerLevelsChanged(PowerChange::Level {
                target: named_only(target),
            }),
            Self::PowerLevels => ServiceEvent::PowerLevelsChanged(PowerChange::Levels),
            Self::CallStarted => ServiceEvent::CallStarted,
            Self::CallNotification { video, target } => ServiceEvent::CallNotification {
                video: *video,
                declined_by: (!target.is_empty()).then(|| named_only(target)),
            },
        }
    }
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(super) enum HistoryDto {
    WorldReadable,
    #[default]
    Shared,
    Invited,
    Joined,
    Custom,
}

impl HistoryDto {
    pub(super) fn to_visibility(self) -> HistoryVisibility {
        match self {
            Self::WorldReadable => HistoryVisibility::WorldReadable,
            Self::Shared => HistoryVisibility::Shared,
            Self::Invited => HistoryVisibility::Invited,
            Self::Joined => HistoryVisibility::Joined,
            Self::Custom => HistoryVisibility::Unknown,
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(super) enum GuestsDto {
    CanJoin,
    Forbidden,
    Custom,
}

impl GuestsDto {
    fn to_access(self) -> GuestAccess {
        match self {
            Self::CanJoin => GuestAccess::CanJoin,
            Self::Forbidden => GuestAccess::Forbidden,
            Self::Custom => GuestAccess::Unknown,
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(super) enum RoleChangeDto {
    MadeAdmin,
    MadeModerator,
    NoLongerAdmin,
    NoLongerModerator,
}

impl RoleChangeDto {
    fn to_change(self) -> RoleChange {
        match self {
            Self::MadeAdmin => RoleChange::MadeAdmin,
            Self::MadeModerator => RoleChange::MadeModerator,
            Self::NoLongerAdmin => RoleChange::NoLongerAdmin,
            Self::NoLongerModerator => RoleChange::NoLongerModerator,
        }
    }
}

fn named_only(name: &str) -> NamedUser {
    NamedUser::new(String::new(), MemberName::new(Some(name), false))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyDto {
    sender: String,
    #[serde(default)]
    event_id: String,
    #[serde(default)]
    kind: KindDto,
    #[serde(default)]
    body: String,
}

impl MessageDto {
    pub fn author(&self) -> (&str, &str) {
        (&self.sender, &self.name)
    }

    pub fn to_message(&self, own_user: &str, now_ms: u64) -> TimelineMessage {
        let body = self.to_body(own_user);
        TimelineMessage {
            unique_id: self.id.clone(),
            event_id: Some(self.id.clone()),
            local_id: None,
            sender_pronouns: data::pronouns(&self.sender),
            sender: self.sender.clone(),
            sender_name: MemberName::new(Some(&self.name), false),
            sender_avatar_url: Some(self.sender.clone()),
            body_emoji: media::emoji_states(&body),
            body,
            mentions_room: self.mentions_room,
            timestamp: ago_ms(now_ms, self.minutes_ago, self.days_ago),
            is_own: self.sender == own_user,
            reply: self.reply.as_ref().map(|reply| ReplyInfo {
                event_id: reply.event_id.clone(),
                sender: named_only(&reply.sender),
                kind: reply.kind.to_kind(),
                body: RichText::plain(reply.body.clone()),
            }),
            edited: self.edited,
            editable: self.sender == own_user,
            is_first_unread: false,
            send_state: SendState::default(),
            shield: Shield::None,
            reactions: self
                .reactions
                .iter()
                .map(|reaction| Reaction {
                    key: reaction.key.clone(),
                    image: media::custom_emoji(&reaction.key),
                    mine: reaction.senders.iter().any(|sender| sender == own_user),
                    senders: reaction
                        .senders
                        .iter()
                        .map(|sender| reactions::reactor(sender))
                        .collect(),
                    send: ReactionSend::default(),
                })
                .collect(),
            read_by: ReadBy::default(),
        }
    }

    fn to_body(&self, own_user: &str) -> MessageBody {
        if let Some(service) = &self.service {
            return MessageBody::Service(service.to_event());
        }
        if let Some(poll) = &self.poll {
            return MessageBody::Poll(poll.to_poll(&self.body, own_user, self.sender == own_user));
        }
        if let Some(sticker) = &self.sticker {
            return MessageBody::Sticker {
                alt: self.body.clone(),
                meta: sticker.to_meta(&self.id),
            };
        }
        if let Some(audio) = &self.audio {
            return MessageBody::Audio {
                caption: (!self.body.is_empty()).then(|| self.rich_body()),
                meta: audio.to_meta(&self.id),
            };
        }
        if let Some(video) = &self.video {
            return MessageBody::Video {
                caption: (!self.body.is_empty()).then(|| self.rich_body()),
                meta: video.to_meta(&self.id),
            };
        }
        match &self.image {
            Some(image) => MessageBody::Image {
                caption: (!self.body.is_empty()).then(|| self.rich_body()),
                meta: ImageMeta {
                    width: Some(image.width),
                    height: Some(image.height),
                    mimetype: Some(
                        image
                            .mimetype
                            .clone()
                            .unwrap_or_else(|| "image/png".to_owned()),
                    ),
                    filename: image.filename.clone(),
                    thumbnail: Some(media::content_of(&self.id)),
                },
            },
            None => MessageBody::Text(self.rich_body()),
        }
    }

    fn rich_body(&self) -> RichText {
        rich_text(&self.body, self.html.as_deref())
    }
}
