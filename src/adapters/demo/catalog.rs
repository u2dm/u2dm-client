use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

use super::{
    attachments, audio, create_room, encryption, identity, invite_people, invites, links, login,
    logout, message_menu, names, notifications, pinned, polls, reactions, receipts, richtext,
    room_info, shields, space_index, stickers, sync, timeline, typing, upgrades, user_info,
    verification, videos,
};

pub struct Flag {
    pub value: &'static str,
    pub effect: &'static str,
    pub note: &'static str,
}

pub struct Scenarios {
    pub env: &'static str,
    pub summary: &'static str,
    pub combinable: bool,
    pub flags: &'static [Flag],
    pub notes: &'static [&'static str],
}

pub fn all() -> &'static [&'static Scenarios] {
    &[
        &timeline::CATALOG,
        &pinned::CATALOG,
        &reactions::CATALOG,
        &polls::CATALOG,
        &receipts::CATALOG,
        &names::CATALOG,
        &shields::CATALOG,
        &richtext::CATALOG,
        &stickers::CATALOG,
        &attachments::CATALOG,
        &audio::CATALOG,
        &videos::CATALOG,
        &space_index::CATALOG,
        &invites::CATALOG,
        &upgrades::CATALOG,
        &links::CATALOG,
        &create_room::CATALOG,
        &invite_people::CATALOG,
        &typing::CATALOG,
        &notifications::CATALOG,
        &room_info::CATALOG,
        &user_info::CATALOG,
        &identity::CATALOG,
        &encryption::CATALOG,
        &message_menu::CATALOG,
        &login::CATALOG,
        &logout::CATALOG,
        &sync::CATALOG,
        &verification::CATALOG,
        &WINDOW,
        &DATA,
        &HANDOFF,
    ]
}

const WINDOW: Scenarios = Scenarios {
    env: "U2DM_DEMO_WINDOW",
    summary: "overrides the fixed 860x1000 screenshot window size",
    combinable: false,
    flags: &[Flag {
        value: "<width>x<height>",
        effect: "sizes the window, for example 700x900",
        note: "a tiling compositor ignores this entirely",
    }],
    notes: &["the only way to reach the compact layout without a narrow window"],
};

const DATA: Scenarios = Scenarios {
    env: "U2DM_DEMO_DATA",
    summary: "reads the fixture from another path instead of assets/demo/data.json",
    combinable: false,
    flags: &[Flag {
        value: "<path>",
        effect: "loads that JSON file as the demo fixture",
        note: "images still resolve against assets/demo",
    }],
    notes: &["a malformed fixture names the offending field and starts the app empty"],
};

const HANDOFF: Scenarios = Scenarios {
    env: "U2DM_DEMO_HANDOFF",
    summary: "listens on a socket at this path for links another demo launch hands over",
    combinable: false,
    flags: &[Flag {
        value: "<path>",
        effect: "a second demo launch with the same value hands its link, or nothing, to this one and exits",
        note: "the socket's directory must be private to you (0700)",
    }],
    notes: &[
        "without it a demo run neither listens nor hands over, so it never reaches a real U2DM's socket",
    ],
};

pub fn print() -> ExitCode {
    match write_catalog() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("the scenario catalog could not be written: {e}");
            ExitCode::FAILURE
        }
    }
}

fn write_catalog() -> io::Result<()> {
    let body = serde_json::to_vec_pretty(&json())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut out = io::stdout().lock();
    out.write_all(&body)?;
    out.write_all(b"\n")?;
    out.flush()
}

pub fn json() -> serde_json::Value {
    let scenarios: Vec<serde_json::Value> = all()
        .iter()
        .map(|scenario| {
            serde_json::json!({
                "env": scenario.env,
                "summary": scenario.summary,
                "combinable": scenario.combinable,
                "current": env::var(scenario.env).ok(),
                "notes": scenario.notes,
                "values": scenario.flags.iter().map(|flag| serde_json::json!({
                    "value": flag.value,
                    "effect": flag.effect,
                    "note": flag.note,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({ "scenarios": scenarios })
}
