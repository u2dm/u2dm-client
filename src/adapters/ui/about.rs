use std::env::consts::{ARCH, OS};

use super::props::{BoolProp, StringProp, UiProps};

const APP_NAME: &str = "U2DM";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const LICENCE: &str = env!("CARGO_PKG_LICENSE");
const SOURCE: &str = env!("CARGO_PKG_REPOSITORY");
const PART_SEPARATOR: &str = ", ";
const LINE_SEPARATOR: &str = " · ";

#[derive(Clone, Copy)]
struct BuildPart {
    name: &'static str,
    built: bool,
}

const PARTS: [BuildPart; 3] = [
    BuildPart {
        name: "calls",
        built: cfg!(feature = "calls"),
    },
    BuildPart {
        name: "video",
        built: cfg!(feature = "video"),
    },
    BuildPart {
        name: "demo",
        built: cfg!(feature = "demo"),
    },
];

pub(super) fn seed_about<W: UiProps>(w: &W) {
    w.set_text(StringProp::AboutVersion, VERSION);
    w.set_text(StringProp::AboutLicence, LICENCE);
    w.set_text(StringProp::AboutSource, SOURCE);
    w.set_text(StringProp::AboutReport, report_line());
    w.set_bool(BoolProp::AboutCalls, cfg!(feature = "calls"));
    w.set_bool(BoolProp::AboutVideo, cfg!(feature = "video"));
}

fn report_line() -> String {
    let built: Vec<&str> = PARTS
        .iter()
        .filter(|part| part.built)
        .map(|part| part.name)
        .collect();
    let platform = format!("{OS} {ARCH}");
    let named = format!("{APP_NAME} {VERSION}");
    if built.is_empty() {
        return [named, platform].join(LINE_SEPARATOR);
    }
    [named, built.join(PART_SEPARATOR), platform].join(LINE_SEPARATOR)
}
