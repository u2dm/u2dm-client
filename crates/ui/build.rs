#![allow(clippy::panic)]

#[cfg(feature = "codegen")]
use std::collections::HashMap;
#[cfg(feature = "codegen")]
use std::env;
use std::fs;
use std::path::Path;
#[cfg(feature = "codegen")]
use std::path::PathBuf;
use std::process::Command;

const TWEMOJI_FONT: &str = "../../ui/fonts/Twemoji.ttf";
const FONT_REPO: &str = "u2dm/twemoji";

#[cfg(feature = "codegen")]
const UI_MAIN: &str = "../../ui/main.slint";
#[cfg(feature = "codegen")]
const APP_TRANSLATION_DOMAIN: &str = "u2dm";
#[cfg(feature = "codegen")]
const TRANSLATION_DOMAIN_SOURCE: &str = "CARGO_PKG_NAME";

fn main() {
    ensure_twemoji_font();

    #[cfg(feature = "codegen")]
    compile_ui();
}

#[cfg(feature = "codegen")]
fn compile_ui() {
    bind_generated_ui_to_app_translation_domain();

    let library = HashMap::from([("lucide".to_string(), PathBuf::from(lucide_slint::lib()))]);
    let config = slint_build::CompilerConfiguration::new().with_library_paths(library);
    if let Err(e) = slint_build::compile_with_config(UI_MAIN, config) {
        panic!("Failed to compile Slint UI: {e}");
    }

    let Ok(out_dir) = env::var("OUT_DIR") else {
        panic!("OUT_DIR is unset, so the generated UI cannot be checked");
    };
    reject_foreign_translation_domain(Path::new(&out_dir));
}

#[cfg(feature = "codegen")]
fn bind_generated_ui_to_app_translation_domain() {
    unsafe { env::set_var(TRANSLATION_DOMAIN_SOURCE, APP_TRANSLATION_DOMAIN) };
}

#[cfg(feature = "codegen")]
fn reject_foreign_translation_domain(out_dir: &Path) {
    let generated = generated_sources(out_dir);
    assert!(
        !generated.is_empty(),
        "the Slint compiler wrote no Rust source into {}, so the UI cannot be checked or included",
        out_dir.display()
    );

    let compact: String = generated.chars().filter(|c| !c.is_whitespace()).collect();
    if !compact.contains("private_unstable_api::translate(") {
        return;
    }

    assert!(
        compact.contains(&format!("from(\"{APP_TRANSLATION_DOMAIN}\")")),
        "the generated UI does not look up translations under the `{APP_TRANSLATION_DOMAIN}` \
         gettext domain. slint-build takes that domain from CARGO_PKG_NAME, which is \
         `u2dm-ui` here, while the binary binds the catalogs under `{APP_TRANSLATION_DOMAIN}`. \
         A mismatch is not a compile error: every @tr string silently falls back to English. \
         Keep bind_generated_ui_to_app_translation_domain() ahead of the Slint compile."
    );
}

#[cfg(feature = "codegen")]
fn generated_sources(out_dir: &Path) -> String {
    let Ok(entries) = fs::read_dir(out_dir) else {
        return String::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter_map(|path| fs::read_to_string(path).ok())
        .collect()
}

fn ensure_twemoji_font() {
    println!("cargo::rerun-if-changed={TWEMOJI_FONT}");

    if Path::new(TWEMOJI_FONT).exists() {
        return;
    }

    if let Some(parent) = Path::new(TWEMOJI_FONT).parent()
        && fs::create_dir_all(parent).is_err()
    {
        panic!(
            "failed to create the {} directory for the emoji font",
            parent.display()
        );
    }

    let url = format!("https://github.com/{FONT_REPO}/releases/latest/download/Twemoji.ttf");
    println!("cargo::warning={TWEMOJI_FONT} is missing; downloading it from {url}");

    let tmp = format!("{TWEMOJI_FONT}.download");
    match Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--output",
            &tmp,
            &url,
        ])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(_) => panic!(
            "failed to download {url}. Confirm a release exists at \
             https://github.com/{FONT_REPO}/releases."
        ),
        Err(e) => panic!("failed to run curl to download {url}: {e}. Install curl."),
    }

    if let Err(e) = fs::rename(&tmp, TWEMOJI_FONT) {
        drop(fs::remove_file(&tmp));
        panic!("failed to move downloaded emoji font into place: {e}");
    }
}
