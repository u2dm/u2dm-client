#![allow(clippy::panic)]

use std::fs;
use std::process::Command;

fn main() {
    #[cfg(not(feature = "interpreted"))]
    if let Err(e) = slint_build::compile("ui/main.slint") {
        panic!("Failed to compile Slint UI: {e}");
    }

    update_translations();
}

fn collect_slint_files(dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_slint_files(&path.to_string_lossy()));
        } else if path.extension().is_some_and(|ext| ext == "slint") {
            files.push(path.to_string_lossy().to_string());
        }
    }
    files
}

fn update_translations() {
    let lang_dir = "lang";
    let pot_file = format!("{lang_dir}/u2dm.pot");

    let slint_files = collect_slint_files("ui");
    if slint_files.is_empty() {
        return;
    }

    let Ok(status) = Command::new("slint-tr-extractor")
        .arg("-o")
        .arg(&pot_file)
        .args(&slint_files)
        .status()
    else {
        println!("cargo::warning=slint-tr-extractor not found, skipping translation extraction");
        return;
    };

    if !status.success() {
        println!("cargo::warning=slint-tr-extractor failed");
        return;
    }

    strip_pot_creation_date(&pot_file);

    let Ok(entries) = fs::read_dir(lang_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let is_po = path.extension().is_some_and(|ext| ext == "po");
        let Some(lang) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        if is_po {
            let po_path = path.to_string_lossy().to_string();
            let mo_dir = format!("{lang_dir}/{lang}/LC_MESSAGES");
            drop(fs::create_dir_all(&mo_dir));

            drop(
                Command::new("msgmerge")
                    .args([
                        "--update",
                        "--no-fuzzy-matching",
                        "--backup=none",
                        &po_path,
                        &pot_file,
                    ])
                    .status(),
            );

            drop(
                Command::new("msgfmt")
                    .args([&po_path, "-o", &format!("{mo_dir}/U2DM.mo")])
                    .status(),
            );
        }
    }

    println!("cargo::rerun-if-changed=ui/");
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
