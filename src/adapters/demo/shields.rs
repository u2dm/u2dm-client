use std::collections::BTreeMap;
use std::env;
use std::sync::OnceLock;

use super::catalog::{Flag, Scenarios};
use super::{data, media};
use crate::domain::media::ImageMeta;
use crate::domain::member_name::MemberName;
use crate::domain::message::{
    DecryptionFailure, MessageBody, ReadBy, RichText, SendState, Shield, ShieldCode,
    TimelineMessage,
};
use crate::domain::room::RoomId;

const ENV_VAR: &str = "U2DM_DEMO_SHIELDS";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "adds messages to !design:demo.local whose senders the encryption cannot vouch for",
    combinable: true,
    flags: &[
        Flag {
            value: "marks",
            effect: "one message per shield a decrypted row can carry: five warnings and the grey notice",
            note: "a photo and a sticker carry one too, on the overlay chip and under the sticker",
        },
        Flag {
            value: "hidden",
            effect: "messages the trust requirement refused, one per reason, beside an unexplained one",
            note: "",
        },
        Flag {
            value: "own",
            effect: "your message from a device you never verified, warned, and one refused",
            note: "the refused one is what an unverified U2DM shows for its own sends",
        },
        Flag {
            value: "all",
            effect: "marks, hidden and own",
            note: "",
        },
    ],
    notes: &["only !design:demo.local changes, an encrypted room; open it with select_room"],
};

const ROOM: &str = "!design:demo.local";
const NADIA: &str = "@nadia:matrix.org";
const JORDAN: &str = "@jordan:matrix.org";
const PRIYA: &str = "@priya:matrix.org";
const PHOTO_CONTENT: &str = "demo-rust-chart";
const PHOTO_WIDTH: u32 = 1040;
const PHOTO_HEIGHT: u32 = 660;
const STICKER_CONTENT: &str = "demo-sticker-ship";
const STICKER_EXTENT: u32 = 512;
const MINUTE_MS: u64 = 60_000;

#[derive(Default, Clone, Copy)]
pub struct Scenario {
    pub marks: bool,
    pub hidden: bool,
    pub own: bool,
}

pub fn scenario() -> Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    *SCENARIO.get_or_init(from_env)
}

fn from_env() -> Scenario {
    let Ok(raw) = env::var(ENV_VAR) else {
        return Scenario::default();
    };
    let mut scenario = Scenario::default();
    for flag in raw.split(',').map(str::trim) {
        apply(&mut scenario, flag);
    }
    tracing::info!(
        marks = scenario.marks,
        hidden = scenario.hidden,
        own = scenario.own,
        "demo mode: adding messages the encryption cannot vouch for"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "marks" => scenario.marks = true,
        "hidden" => scenario.hidden = true,
        "own" => scenario.own = true,
        "all" => {
            scenario.marks = true;
            scenario.hidden = true;
            scenario.own = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

struct Post<'a> {
    id: &'a str,
    sender: &'a str,
    minutes_ago: u64,
    body: MessageBody,
    shield: Shield,
}

fn text(body: &str) -> MessageBody {
    MessageBody::Text(RichText::plain(body.to_owned()))
}

fn hidden(failure: DecryptionFailure) -> MessageBody {
    MessageBody::UnableToDecrypt(failure)
}

fn photo() -> MessageBody {
    MessageBody::Image {
        caption: None,
        meta: ImageMeta {
            width: Some(PHOTO_WIDTH),
            height: Some(PHOTO_HEIGHT),
            mimetype: Some("image/png".to_owned()),
            filename: Some("onboarding-v3.png".to_owned()),
            thumbnail: Some(media::content_of(PHOTO_CONTENT)),
        },
    }
}

fn sticker() -> MessageBody {
    MessageBody::Sticker {
        alt: "ship it".to_owned(),
        meta: ImageMeta {
            width: Some(STICKER_EXTENT),
            height: Some(STICKER_EXTENT),
            mimetype: Some("image/png".to_owned()),
            filename: None,
            thumbnail: Some(media::content_of(STICKER_CONTENT)),
        },
    }
}

fn marked_posts() -> Vec<Post<'static>> {
    vec![
        Post {
            id: "demo-shields-unsigned",
            sender: NADIA,
            minutes_ago: 15,
            body: text("posting from the old tablet, the one I never set up properly"),
            shield: Shield::Warning(ShieldCode::UnsignedDevice),
        },
        Post {
            id: "demo-shields-unknown",
            sender: JORDAN,
            minutes_ago: 14,
            body: text("is my work laptop still logged in? I wiped it last week"),
            shield: Shield::Warning(ShieldCode::UnknownDevice),
        },
        Post {
            id: "demo-shields-changed",
            sender: PRIYA,
            minutes_ago: 13,
            body: text("had to reset everything after my phone died, verify me again pls"),
            shield: Shield::Warning(ShieldCode::VerificationViolation),
        },
        Post {
            id: "demo-shields-mismatched",
            sender: JORDAN,
            minutes_ago: 12,
            body: text("standup moves to 10:30, relayed by the bridge"),
            shield: Shield::Warning(ShieldCode::MismatchedSender),
        },
        Post {
            id: "demo-shields-clear",
            sender: NADIA,
            minutes_ago: 11,
            body: text("this one went out unencrypted, sorry"),
            shield: Shield::Warning(ShieldCode::SentInClear),
        },
        Post {
            id: "demo-shields-backup",
            sender: PRIYA,
            minutes_ago: 10,
            body: text("found this in the history the key backup gave me back"),
            shield: Shield::Notice(ShieldCode::AuthenticityNotGuaranteed),
        },
        Post {
            id: "demo-shields-photo",
            sender: NADIA,
            minutes_ago: 9,
            body: photo(),
            shield: Shield::Warning(ShieldCode::UnsignedDevice),
        },
        Post {
            id: "demo-shields-sticker",
            sender: JORDAN,
            minutes_ago: 8,
            body: sticker(),
            shield: Shield::Notice(ShieldCode::AuthenticityNotGuaranteed),
        },
    ]
}

fn hidden_posts() -> Vec<Post<'static>> {
    vec![
        Post {
            id: "demo-shields-hidden-unsigned",
            sender: JORDAN,
            minutes_ago: 7,
            body: hidden(DecryptionFailure::UnsignedDevice),
            shield: Shield::None,
        },
        Post {
            id: "demo-shields-hidden-unknown",
            sender: NADIA,
            minutes_ago: 6,
            body: hidden(DecryptionFailure::UnknownDevice),
            shield: Shield::None,
        },
        Post {
            id: "demo-shields-hidden-changed",
            sender: PRIYA,
            minutes_ago: 5,
            body: hidden(DecryptionFailure::VerificationViolation),
            shield: Shield::None,
        },
        Post {
            id: "demo-shields-hidden-unexplained",
            sender: NADIA,
            minutes_ago: 4,
            body: hidden(DecryptionFailure::Unexplained),
            shield: Shield::None,
        },
    ]
}

fn own_posts(own_user: &str) -> Vec<Post<'_>> {
    vec![
        Post {
            id: "demo-shields-own-warned",
            sender: own_user,
            minutes_ago: 3,
            body: text("sent from this laptop before I verified it"),
            shield: Shield::Warning(ShieldCode::UnsignedDevice),
        },
        Post {
            id: "demo-shields-own-hidden",
            sender: own_user,
            minutes_ago: 2,
            body: hidden(DecryptionFailure::UnsignedDevice),
            shield: Shield::None,
        },
    ]
}

fn message(post: Post<'_>, own_user: &str, now_ms: u64) -> TimelineMessage {
    let is_own = post.sender == own_user;
    TimelineMessage {
        unique_id: post.id.to_owned(),
        event_id: Some(post.id.to_owned()),
        local_id: None,
        sender: post.sender.to_owned(),
        sender_name: if is_own {
            data::own_sender_name()
        } else {
            MemberName::Unset
        },
        sender_avatar_url: Some(post.sender.to_owned()),
        sender_pronouns: Vec::new(),
        body_emoji: BTreeMap::new(),
        body: post.body,
        mentions_room: false,
        timestamp: now_ms.saturating_sub(post.minutes_ago.saturating_mul(MINUTE_MS)),
        is_own,
        reply: None,
        edited: false,
        editable: false,
        is_first_unread: false,
        send_state: SendState::Sent,
        shield: post.shield,
        reactions: Vec::new(),
        read_by: ReadBy::default(),
    }
}

pub fn apply_scenario(room_id: &RoomId, messages: &mut Vec<TimelineMessage>, now_ms: u64) {
    if room_id.as_ref() != ROOM {
        return;
    }
    let scenario = scenario();
    let own_user = data::own_user();
    let mut posts = Vec::new();
    if scenario.marks {
        posts.extend(marked_posts());
    }
    if scenario.hidden {
        posts.extend(hidden_posts());
    }
    if scenario.own {
        posts.extend(own_posts(own_user));
    }
    messages.extend(
        posts
            .into_iter()
            .map(|post| message(post, own_user, now_ms)),
    );
}
