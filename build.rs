#![allow(clippy::panic)]

#[cfg(not(feature = "interpreted"))]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

const LANG_DIR: &str = "lang";
const UI_DIR: &str = "ui";
const POT_FILE: &str = "lang/u2dm.pot";
const LUCIDE_LSP_LIB: &str = ".lucide/lib.slint";
const TWEMOJI_FONT: &str = "ui/fonts/Twemoji.ttf";
const FONT_REPO: &str = "u2dm/twemoji";

fn main() {
    sync_lucide_lsp_lib();
    ensure_twemoji_font();

    #[cfg(not(feature = "interpreted"))]
    {
        let library = HashMap::from([("lucide".to_string(), PathBuf::from(lucide_slint::lib()))]);
        let config = slint_build::CompilerConfiguration::new().with_library_paths(library);
        if let Err(e) = slint_build::compile_with_config("ui/main.slint", config) {
            panic!("Failed to compile Slint UI: {e}");
        }
    }

    update_translations();
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

    // hardcoded for now
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

fn sync_lucide_lsp_lib() {
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let src = PathBuf::from(lucide_slint::lib());
    let dest = Path::new(&manifest_dir).join(LUCIDE_LSP_LIB);

    let up_to_date = fs::metadata(&dest)
        .ok()
        .zip(fs::metadata(&src).ok())
        .is_some_and(|(d, s)| d.len() == s.len());
    if up_to_date {
        return;
    }

    if let Some(parent) = dest.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }
    drop(fs::copy(&src, &dest));
}

fn update_translations() {
    let slint_files = collect_files_recursive(UI_DIR, "slint");
    if slint_files.is_empty() {
        return;
    }

    if !extract_translatable_strings(&slint_files) {
        return;
    }

    strip_pot_creation_date(POT_FILE);

    let pkg_name = env::var("CARGO_PKG_NAME").unwrap_or_default();
    for po_path in collect_files_recursive(LANG_DIR, "po") {
        merge_translations(&po_path);
        compile_translations(&po_path, &pkg_name);
    }

    println!("cargo::rerun-if-changed={UI_DIR}/");
}

fn collect_files_recursive(dir: &str, extension: &str) -> Vec<String> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_files_recursive(&path.to_string_lossy(), extension));
        } else if path.extension().is_some_and(|ext| ext == extension) {
            files.push(path.to_string_lossy().to_string());
        }
    }
    files
}

fn extract_translatable_strings(slint_files: &[String]) -> bool {
    let Ok(status) = Command::new("slint-tr-extractor")
        .arg("-o")
        .arg(POT_FILE)
        .args(slint_files)
        .status()
    else {
        println!("cargo::warning=slint-tr-extractor not found, skipping translation extraction");
        return false;
    };

    if !status.success() {
        println!("cargo::warning=slint-tr-extractor failed");
        return false;
    }

    true
}

fn strip_pot_creation_date(path: &str) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let stripped: String = content
        .lines()
        .filter(|line| !line.contains("POT-Creation-Date"))
        .collect::<Vec<_>>()
        .join("\n");
    drop(fs::write(path, stripped));
}

fn merge_translations(po_path: &str) {
    drop(
        Command::new("msgmerge")
            .args([
                "--update",
                "--no-fuzzy-matching",
                "--backup=none",
                po_path,
                POT_FILE,
            ])
            .status(),
    );
}

fn compile_translations(po_path: &str, pkg_name: &str) {
    let Some(lang) = Path::new(po_path).file_stem().map(|s| s.to_string_lossy()) else {
        return;
    };

    let mo_dir = format!("{LANG_DIR}/{lang}/LC_MESSAGES");
    drop(fs::create_dir_all(&mo_dir));

    drop(
        Command::new("msgfmt")
            .args([po_path, "-o", &format!("{mo_dir}/{pkg_name}.mo")])
            .status(),
    );
}
