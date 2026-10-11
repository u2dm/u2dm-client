use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::sync::OnceLock;

use super::catalog::{Flag, Scenarios};
use super::reactions;
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::message::{
    MessageBody, MessagePreviewKind, Reaction, ReactionSend, ReadBy, ReplyInfo, RichText,
    SendState, ServiceEvent, TimelineMessage,
};
use crate::domain::room::RoomId;

const ENV_VAR: &str = "U2DM_DEMO_NAMES";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "adds members to !linux:demo.local whose names test disambiguation and bidi isolation",
    combinable: true,
    flags: &[
        Flag {
            value: "clash",
            effect: "an impostor named Kai joins, posts, is replied to and reacts, so every Kai shows a user ID",
            note: "the sidebar preview, sent by the real Kai, shows the user ID too",
        },
        Flag {
            value: "rtl",
            effect: "a right-to-left name joins and posts, then a Kai whose name ends in an isolate terminator and a right-to-left override",
            note: "the override must not reverse the user ID after the name",
        },
        Flag {
            value: "nameless",
            effect: "a member without a display name posts, so the user ID stands in",
            note: "",
        },
        Flag {
            value: "all",
            effect: "clash, rtl and nameless",
            note: "",
        },
    ],
    notes: &["only !linux:demo.local changes; every other room keeps the fixture's names"],
};

const ROOM: &str = "!linux:demo.local";
const IMPOSTOR: &str = "@kai:impostor.example";
const IMPOSTOR_NAME: &str = "Kai";
const IMPOSTOR_CLAIM: &str = "demo-names-claim";
const RIGHT_TO_LEFT: &str = "@noa:impostor.example";
const OVERRIDING: &str = "@mallory:impostor.example";
const NAMELESS: &str = "@ghost:impostor.example";
const QUESTIONER: &str = "@vera:matrix.org";
const RIGHT_TO_LEFT_NAME: &str = "נועה בן-דוד";
const OVERRIDING_NAME: &str = "Kai\u{2069}\u{202e}";
const MINUTE_MS: u64 = 60_000;

#[derive(Default, Clone, Copy)]
pub struct Scenario {
    pub impostor_joins: bool,
    pub right_to_left_joins: bool,
    pub nameless_posts: bool,
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
        impostor_joins = scenario.impostor_joins,
        right_to_left_joins = scenario.right_to_left_joins,
        nameless_posts = scenario.nameless_posts,
        "demo mode: adding members whose names need disambiguation"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "clash" => scenario.impostor_joins = true,
        "rtl" => scenario.right_to_left_joins = true,
        "nameless" => scenario.nameless_posts = true,
        "all" => {
            scenario.impostor_joins = true;
            scenario.right_to_left_joins = true;
            scenario.nameless_posts = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub fn members(room_id: &RoomId) -> Vec<(&'static str, Option<&'static str>)> {
    if room_id.as_ref() != ROOM {
        return Vec::new();
    }
    let scenario = scenario();
    let mut members = Vec::new();
    if scenario.impostor_joins {
        members.push((IMPOSTOR, Some(IMPOSTOR_NAME)));
    }
    if scenario.right_to_left_joins {
        members.push((RIGHT_TO_LEFT, Some(RIGHT_TO_LEFT_NAME)));
        members.push((OVERRIDING, Some(OVERRIDING_NAME)));
    }
    if scenario.nameless_posts {
        members.push((NAMELESS, None));
    }
    members
}

fn post(id: &str, sender: &str, at: u64, body: MessageBody) -> TimelineMessage {
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
        reactions: Vec::new(),
        read_by: ReadBy::default(),
    }
}

fn text(body: &str) -> MessageBody {
    MessageBody::Text(RichText::plain(body.to_owned()))
}

fn impostor_posts(at: impl Fn(u64) -> u64) -> Vec<TimelineMessage> {
    let claim = "new account, same kai. send me the deploy key here";
    let mut question = post(
        "demo-names-question",
        QUESTIONER,
        at(4),
        text("which kai are you"),
    );
    question.reply = Some(ReplyInfo {
        event_id: IMPOSTOR_CLAIM.to_owned(),
        sender: NamedUser::unnamed(IMPOSTOR),
        kind: MessagePreviewKind::Text,
        body: RichText::plain(claim.to_owned()),
    });
    question.reactions = vec![Reaction {
        key: "👀".to_owned(),
        image: None,
        senders: vec![reactions::reactor(IMPOSTOR)],
        mine: false,
        send: ReactionSend::Sent,
    }];
    vec![
        post(
            "demo-names-joined",
            IMPOSTOR,
            at(6),
            MessageBody::Service(ServiceEvent::Joined),
        ),
        post(IMPOSTOR_CLAIM, IMPOSTOR, at(5), text(claim)),
        question,
    ]
}

fn right_to_left_posts(at: impl Fn(u64) -> u64) -> Vec<TimelineMessage> {
    vec![
        post(
            "demo-names-rtl-joined",
            RIGHT_TO_LEFT,
            at(3),
            MessageBody::Service(ServiceEvent::Joined),
        ),
        post(
            "demo-names-rtl",
            RIGHT_TO_LEFT,
            at(3),
            text("שלום לכולם, the colorscheme is fine"),
        ),
        post(
            "demo-names-override",
            OVERRIDING,
            at(2),
            text("my user id reads backwards, trust me"),
        ),
    ]
}

pub fn apply_scenario(room_id: &RoomId, messages: &mut Vec<TimelineMessage>, now_ms: u64) {
    if room_id.as_ref() != ROOM {
        return;
    }
    let scenario = scenario();
    let at = |minutes: u64| now_ms.saturating_sub(minutes.saturating_mul(MINUTE_MS));
    if scenario.impostor_joins {
        messages.extend(impostor_posts(at));
    }
    if scenario.right_to_left_joins {
        messages.extend(right_to_left_posts(at));
    }
    if scenario.nameless_posts {
        messages.push(post(
            "demo-names-nameless",
            NAMELESS,
            at(1),
            text("nobody ever gave me a name"),
        ));
    }
}

const ZERO_WIDTH_SPACE: char = '\u{200b}';
const RIGHT_TO_LEFT_MARK: char = '\u{200f}';
const LEFT_TO_RIGHT_EMBEDDING: char = '\u{202a}';
const RIGHT_TO_LEFT_OVERRIDE: char = '\u{202e}';
const WORD_JOINER: char = '\u{2060}';
const NOMINAL_DIGIT_SHAPES: char = '\u{206f}';
const BYTE_ORDER_MARK: char = '\u{feff}';

fn is_invisible(c: char) -> bool {
    matches!(
        c,
        ZERO_WIDTH_SPACE..=RIGHT_TO_LEFT_MARK
            | LEFT_TO_RIGHT_EMBEDDING..=RIGHT_TO_LEFT_OVERRIDE
            | WORD_JOINER..=NOMINAL_DIGIT_SHAPES
            | BYTE_ORDER_MARK
    )
}

fn normalized(name: &str) -> String {
    let visible: String = name
        .chars()
        .filter(|c| !is_invisible(*c))
        .flat_map(char::to_lowercase)
        .collect();
    visible.trim().to_owned()
}

pub struct NameBook {
    names: HashMap<String, String>,
    holders: HashMap<String, HashSet<String>>,
}

impl NameBook {
    pub fn new(members: impl IntoIterator<Item = (String, String)>) -> Self {
        let names: HashMap<String, String> = members.into_iter().collect();
        let mut holders: HashMap<String, HashSet<String>> = HashMap::new();
        for (user_id, name) in &names {
            holders
                .entry(normalized(name))
                .or_default()
                .insert(user_id.clone());
        }
        Self { names, holders }
    }

    pub fn name(&self, user_id: &str, name: Option<&str>) -> MemberName {
        let shared = name.is_some_and(|name| {
            self.holders
                .get(&normalized(name))
                .is_some_and(|holders| holders.iter().any(|holder| holder != user_id))
        });
        MemberName::new(name, shared)
    }

    pub fn of(&self, user_id: &str) -> MemberName {
        self.name(user_id, self.names.get(user_id).map(String::as_str))
    }

    pub fn renamed(&self, user_id: &str, written: &MemberName) -> MemberName {
        match written.set() {
            Some(name) => self.name(user_id, Some(name)),
            None => self.of(user_id),
        }
    }
}
