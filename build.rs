#![allow(clippy::panic)]

use std::fs;
use std::path::Path;
use std::process::Command;

const LANG_DIR: &str = "lang";
const UI_DIR: &str = "ui";
const POT_FILE: &str = "lang/u2dm.pot";

fn main() {
    #[cfg(not(feature = "interpreted"))]
    if let Err(e) = slint_build::compile("ui/main.slint") {
        panic!("Failed to compile Slint UI: {e}");
    }

    update_translations();
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

    for po_path in collect_files_recursive(LANG_DIR, "po") {
        merge_translations(&po_path);
        compile_translations(&po_path);
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

fn compile_translations(po_path: &str) {
    let Some(lang) = Path::new(po_path).file_stem().map(|s| s.to_string_lossy()) else {
        return;
    };

    let mo_dir = format!("{LANG_DIR}/{lang}/LC_MESSAGES");
    drop(fs::create_dir_all(&mo_dir));

    drop(
        Command::new("msgfmt")
            .args([po_path, "-o", &format!("{mo_dir}/U2DM.mo")])
            .status(),
    );
}
