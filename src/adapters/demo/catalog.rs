use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

use super::{
    attachments, audio, login, reactions, richtext, stickers, timeline, verification, videos,
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
        &reactions::CATALOG,
        &richtext::CATALOG,
        &stickers::CATALOG,
        &attachments::CATALOG,
        &audio::CATALOG,
        &videos::CATALOG,
        &login::CATALOG,
        &verification::CATALOG,
        &WINDOW,
        &DATA,
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
