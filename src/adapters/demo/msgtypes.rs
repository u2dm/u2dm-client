use std::env;
use std::sync::OnceLock;

use super::catalog::{Flag, Scenarios};
use super::{data, media};
use crate::domain::location::Location;
use crate::domain::media::FileMeta;
use crate::domain::message::{MessageBody, RichText, SendState, TimelineMessage};

const ENV_VAR: &str = "U2DM_DEMO_MSGTYPES";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "appends a message of each type the timeline draws apart from m.text",
    combinable: true,
    flags: &[
        Flag {
            value: "notice",
            effect: "bot notices: plain, formatted with a link, one tagging you, and one of yours",
            note: "",
        },
        Flag {
            value: "emote",
            effect: "emotes: plain, formatted, with a link, with a custom emoji, in paragraphs, \
                     and one of yours",
            note: "",
        },
        Flag {
            value: "file",
            effect: "files with a plain and a formatted caption, one without, and one of yours \
                     whose caption is editable",
            note: "",
        },
        Flag {
            value: "location",
            effect: "locations: a place, an Element-style description, a geo URI in another \
                     coordinate system (no map link), and one of yours",
            note: "",
        },
        Flag {
            value: "unknown",
            effect: "messages of msgtypes no client knows, with and without a fallback body",
            note: "",
        },
        Flag {
            value: "all",
            effect: "notice, emote, file, location and unknown",
            note: "",
        },
    ],
    notes: &[],
};

const NOTICES: &[(&str, Option<&str>)] = &[
    ("Build #4521 passed on main in 3 min 12 s", None),
    (
        "Deploy finished: release 1.4.2 is live",
        Some(
            "<b>Deploy finished</b>: <a href=\"https://example.invalid/deploys/88\">release 1.4.2</a> is live",
        ),
    ),
    (
        "Reminder for Alex: the standup starts in 5 minutes",
        Some(
            "Reminder for <a href=\"https://matrix.to/#/@alex:matrix.org\">Alex</a>: the standup starts in 5 minutes",
        ),
    ),
];
const OWN_NOTICE: &str = "Backup finished: 2.3 GB in 41 s";

const EMOTES: &[(&str, Option<&str>)] = &[
    ("waves at everyone", None),
    (
        "thinks this is an example emote",
        Some("thinks <b>this</b> is an example emote"),
    ),
    (
        "is reading https://spec.matrix.org/v1.19/client-server-api/#memote",
        None,
    ),
    (
        "celebrates :ship:",
        Some(
            "celebrates <img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-ship\" alt=\":ship:\">",
        ),
    ),
    (
        "shrugs\n\nand moves on",
        Some("<p>shrugs</p><p>and moves on</p>"),
    ),
];
const OWN_EMOTE: &str = "is heading out, back in an hour";

struct DemoFile {
    filename: &'static str,
    mimetype: &'static str,
    size: u64,
    caption: Option<(&'static str, Option<&'static str>)>,
}

const FILES: &[DemoFile] = &[
    DemoFile {
        filename: "q3-report.pdf",
        mimetype: "application/pdf",
        size: 482_133,
        caption: Some(("Numbers for the Thursday review", None)),
    },
    DemoFile {
        filename: "notes.md",
        mimetype: "text/markdown",
        size: 2_048,
        caption: Some((
            "Draft notes, see the doc",
            Some("<b>Draft</b> notes, see <a href=\"https://example.invalid/doc\">the doc</a>"),
        )),
    },
    DemoFile {
        filename: "archive.tar.zst",
        mimetype: "application/zstd",
        size: 73_400_320,
        caption: None,
    },
];
const OWN_FILE: DemoFile = DemoFile {
    filename: "slides.odp",
    mimetype: "application/vnd.oasis.opendocument.presentation",
    size: 1_310_720,
    caption: Some(("Final deck for Friday", None)),
};

const LOCATIONS: &[(&str, &str)] = &[
    ("Big Ben, London, UK", "geo:51.5007,-0.1246;u=35"),
    (
        "Location geo:45.4642,9.19;u=10 at 2026-10-18T09:12:00.000Z",
        "geo:45.4642,9.19;u=10",
    ),
    ("Gale crater, Mars", "geo:-5.4,137.8;crs=iau2000:49900"),
];
const OWN_LOCATION: (&str, &str) = ("Office", "geo:-33.8568,151.2153");

const UNKNOWN: &[(&str, &str)] = &[
    ("nic.custom.confetti", "sends confetti 🎉"),
    ("org.example.unknown", ""),
];

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub notice: bool,
    pub emote: bool,
    pub file: bool,
    pub location: bool,
    pub unknown: bool,
}

impl Scenario {
    fn any(self) -> bool {
        self.notice || self.emote || self.file || self.location || self.unknown
    }
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
        notice = scenario.notice,
        emote = scenario.emote,
        file = scenario.file,
        location = scenario.location,
        unknown = scenario.unknown,
        "demo mode: appending message types"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "notice" => scenario.notice = true,
        "emote" => scenario.emote = true,
        "file" => scenario.file = true,
        "location" => scenario.location = true,
        "unknown" => scenario.unknown = true,
        "all" => {
            scenario.notice = true;
            scenario.emote = true;
            scenario.file = true;
            scenario.location = true;
            scenario.unknown = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub fn apply_scenario(messages: &mut Vec<TimelineMessage>) {
    let scenario = scenario();
    if !scenario.any() {
        return;
    }
    let Some(template) = messages.iter().rev().find(|m| !m.is_own).cloned() else {
        return;
    };
    let mut appended = Appended {
        template,
        messages: Vec::new(),
    };
    if scenario.notice {
        appended.notices();
    }
    if scenario.emote {
        appended.emotes();
    }
    if scenario.file {
        appended.files();
    }
    if scenario.location {
        appended.locations();
    }
    if scenario.unknown {
        appended.unknown();
    }
    messages.extend(appended.messages);
}

struct Appended {
    template: TimelineMessage,
    messages: Vec<TimelineMessage>,
}

impl Appended {
    fn notices(&mut self) {
        for (index, (plain, html)) in NOTICES.iter().enumerate() {
            self.theirs("notice", index, MessageBody::Notice(rich(plain, *html)));
        }
        self.mine("notice", MessageBody::Notice(rich(OWN_NOTICE, None)));
    }

    fn emotes(&mut self) {
        for (index, (plain, html)) in EMOTES.iter().enumerate() {
            self.theirs("emote", index, MessageBody::Emote(rich(plain, *html)));
        }
        self.mine("emote", MessageBody::Emote(rich(OWN_EMOTE, None)));
    }

    fn files(&mut self) {
        for (index, file) in FILES.iter().enumerate() {
            self.theirs("file", index, file.body());
        }
        self.mine("file", OWN_FILE.body());
    }

    fn locations(&mut self) {
        for (index, (description, geo_uri)) in LOCATIONS.iter().enumerate() {
            self.theirs("location", index, location(description, geo_uri));
        }
        let (description, geo_uri) = OWN_LOCATION;
        self.mine("location", location(description, geo_uri));
    }

    fn unknown(&mut self) {
        for (index, (kind, fallback)) in UNKNOWN.iter().enumerate() {
            let body = MessageBody::Unsupported {
                kind: (*kind).to_owned(),
                fallback: (*fallback).to_owned(),
            };
            self.theirs("unknown", index, body);
        }
    }

    fn theirs(&mut self, group: &str, index: usize, body: MessageBody) {
        let message = appended(&self.template, &format!("{group}-{index}"), body);
        self.messages.push(message);
    }

    fn mine(&mut self, group: &str, body: MessageBody) {
        let message = appended(&self.template, &format!("{group}-own"), body);
        self.messages.push(TimelineMessage {
            sender: data::own_user().to_owned(),
            sender_name: data::own_sender_name(),
            sender_avatar_url: Some(data::own_user().to_owned()),
            sender_pronouns: Vec::new(),
            is_own: true,
            editable: true,
            ..message
        });
    }
}

impl DemoFile {
    fn body(&self) -> MessageBody {
        MessageBody::File {
            caption: self.caption.map(|(plain, html)| rich(plain, html)),
            meta: FileMeta {
                filename: self.filename.to_owned(),
                mimetype: Some(self.mimetype.to_owned()),
                size: Some(self.size),
            },
        }
    }
}

fn rich(plain: &str, html: Option<&str>) -> RichText {
    match html {
        Some(html) => RichText::formatted(plain.to_owned(), html.to_owned()),
        None => RichText::plain(plain.to_owned()),
    }
}

fn location(description: &str, geo_uri: &str) -> MessageBody {
    MessageBody::Location(Location {
        description: description.to_owned(),
        geo_uri: geo_uri.to_owned(),
    })
}

fn appended(template: &TimelineMessage, suffix: &str, body: MessageBody) -> TimelineMessage {
    let id = format!("demo-msgtypes-{suffix}");
    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        local_id: None,
        body_emoji: media::emoji_states(&body),
        body,
        mentions_room: false,
        reply: None,
        edited: false,
        editable: false,
        is_first_unread: false,
        send_state: SendState::default(),
        reactions: Vec::new(),
        ..template.clone()
    }
}
