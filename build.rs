#![allow(clippy::panic)]

#[cfg(not(feature = "interpreted"))]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

const LANG_DIR: &str = "lang";
const BUNDLED_CATALOGS_ENV: &str = "U2DM_BUNDLED_CATALOGS";
const UI_DIR: &str = "ui";
const ENUMS_FILE: &str = "ui/enums.slint";
const POT_FILE: &str = "lang/u2dm.pot";
const LUCIDE_LSP_LIB: &str = ".lucide/lib.slint";
const TWEMOJI_FONT: &str = "ui/fonts/Twemoji.ttf";
const FONT_REPO: &str = "u2dm/twemoji";
#[cfg(feature = "demo")]
const DEMO_ASSETS_SCRIPT: &str = "scripts/gen-demo-assets.sh";
#[cfg(feature = "demo")]
const DEMO_DATA: &str = "assets/demo/data.json";

struct EnumCoverage {
    slint_enum: &'static str,
    branches_in: &'static str,
    falls_through: &'static [&'static str],
}

const ENUM_COVERAGE: &[EnumCoverage] = &[
    EnumCoverage {
        slint_enum: "UserMessageKind",
        branches_in: "ui/messages.slint",
        falls_through: &["none"],
    },
    EnumCoverage {
        slint_enum: "ServiceKind",
        branches_in: "ui/screens/chat/components/timeline/messages/service-message.slint",
        falls_through: &["none"],
    },
    EnumCoverage {
        slint_enum: "LoginActivity",
        branches_in: "ui/screens/login/common.slint",
        falls_through: &["idle"],
    },
    EnumCoverage {
        slint_enum: "PreviewKind",
        branches_in: "ui/screens/chat/components/message-preview.slint",
        falls_through: &["none", "text"],
    },
    EnumCoverage {
        slint_enum: "MessageKind",
        branches_in: "ui/screens/chat/components/timeline/message-bubble.slint",
        falls_through: &[],
    },
];

const SCHEMA_FILE: &str = "src/adapters/ui/schema.rs";

const ENUM_TABLES: &[(&str, &str)] = &[
    ("UserMessageKind", "user_message_kinds"),
    ("ServiceKind", "service_kinds"),
    ("PreviewKind", "preview_kinds"),
    ("MessageKind", "message_kinds"),
    ("MediaState", "media_states"),
];

fn main() {
    check_enum_branch_coverage();
    sync_lucide_lsp_lib();
    ensure_twemoji_font();

    #[cfg(feature = "demo")]
    fetch_demo_assets();

    #[cfg(not(feature = "interpreted"))]
    {
        let library = HashMap::from([("lucide".to_string(), PathBuf::from(lucide_slint::lib()))]);
        let config = slint_build::CompilerConfiguration::new().with_library_paths(library);
        if let Err(e) = slint_build::compile_with_config("ui/main.slint", config) {
            panic!("Failed to compile Slint UI: {e}");
        }
    }

    update_translations(&bundled_catalog_root());
}

fn check_enum_branch_coverage() {
    println!("cargo::rerun-if-changed={ENUMS_FILE}");

    let Ok(enums) = fs::read_to_string(ENUMS_FILE) else {
        panic!("failed to read {ENUMS_FILE}, so no Slint enum can be checked for missing branches");
    };

    for coverage in ENUM_COVERAGE {
        println!("cargo::rerun-if-changed={}", coverage.branches_in);
        coverage.check(&enums);
    }

    check_enum_table_names(&enums);
}

fn check_enum_table_names(enums: &str) {
    println!("cargo::rerun-if-changed={SCHEMA_FILE}");

    let Ok(schema) = fs::read_to_string(SCHEMA_FILE) else {
        panic!(
            "failed to read {SCHEMA_FILE}, so no enum table can be checked against {ENUMS_FILE}"
        );
    };

    for (slint_enum, table) in ENUM_TABLES {
        let declared = declared_variants(enums, slint_enum);
        let listed = table_names(&schema, table);

        assert!(
            declared == listed,
            "the `{table}!` table in {SCHEMA_FILE} names {listed:?} but `export enum {slint_enum}` \
             in {ENUMS_FILE} declares {declared:?}. Interpreted mode looks these up by name, so a \
             mismatch is not a compile error, it silently drops the value at runtime. Keep both \
             lists identical and in the same order."
        );
    }
}

fn quoted_literals(source: &str) -> impl Iterator<Item = &str> {
    source.split('"').skip(1).step_by(2)
}

fn table_names(schema: &str, table: &str) -> Vec<String> {
    let header = format!("macro_rules! {table} {{");
    let footer = format!("pub(crate) use {table};");
    let Some(body) = schema
        .split_once(header.as_str())
        .and_then(|(_, rest)| rest.split_once(footer.as_str()))
        .map(|(body, _)| body)
    else {
        panic!(
            "{SCHEMA_FILE} has no `macro_rules! {table}` followed by `{footer}`, so its variant \
             names cannot be checked. Update ENUM_TABLES in build.rs to the new name."
        );
    };

    let names: Vec<String> = quoted_literals(body)
        .map(|name| name.replace('_', "-"))
        .collect();

    assert!(
        !names.is_empty(),
        "the `{table}!` table in {SCHEMA_FILE} names no variants"
    );

    names
}

fn declared_variants(enums: &str, slint_enum: &str) -> Vec<String> {
    let header = format!("export enum {slint_enum} {{");
    let Some(body) = enums
        .split_once(header.as_str())
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(body, _)| body)
    else {
        panic!(
            "{ENUMS_FILE} declares no `export enum {slint_enum}`, so it cannot be checked. Update \
             ENUM_COVERAGE or ENUM_TABLES in build.rs to the new name."
        );
    };

    let variants: Vec<String> = body
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .map(|line| line.trim().trim_end_matches(',').trim().replace('_', "-"))
        .filter(|variant| !variant.is_empty())
        .collect();

    assert!(
        !variants.is_empty(),
        "`export enum {slint_enum}` in {ENUMS_FILE} declares no variants"
    );

    variants
}

impl EnumCoverage {
    fn check(&self, enums: &str) {
        let variants = declared_variants(enums, self.slint_enum);
        self.reject_stale_exemptions(&variants);

        let Ok(branches) = fs::read_to_string(self.branches_in) else {
            panic!(
                "failed to read {}, which is where {} is turned into text",
                self.branches_in, self.slint_enum
            );
        };
        let branches = branches.replace('_', "-");

        let missing: Vec<&str> = variants
            .iter()
            .map(String::as_str)
            .filter(|variant| !self.falls_through.contains(variant))
            .filter(|variant| !branches_on(&branches, self.slint_enum, variant))
            .collect();

        assert!(
            missing.is_empty(),
            "{} has no branch for {} {missing:?}. Slint has no match and no exhaustiveness check, \
             so this reaches the user as the fallthrough rather than as text. Add a branch, or \
             list the variant under falls_through in ENUM_COVERAGE in build.rs if the fallthrough \
             is deliberate.",
            self.branches_in,
            self.slint_enum
        );
    }

    fn reject_stale_exemptions(&self, variants: &[String]) {
        let stale: Vec<&str> = self
            .falls_through
            .iter()
            .copied()
            .filter(|exempt| !variants.iter().any(|variant| variant == exempt))
            .collect();

        assert!(
            stale.is_empty(),
            "ENUM_COVERAGE in build.rs lets {stale:?} fall through {}, but {ENUMS_FILE} no longer \
             declares them. A stale exemption silently covers for a missing branch, so fix the \
             list.",
            self.slint_enum
        );
    }
}

fn branches_on(branches: &str, slint_enum: &str, variant: &str) -> bool {
    let reference = format!("{slint_enum}.{variant}");
    branches
        .match_indices(reference.as_str())
        .any(|(at, _)| is_whole_reference(branches, at, reference.len()))
}

fn is_whole_reference(branches: &str, at: usize, len: usize) -> bool {
    let before = branches.get(..at).and_then(|head| head.chars().next_back());
    let after = branches
        .get(at + len..)
        .and_then(|tail| tail.chars().next());
    !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn bundled_catalog_root() -> PathBuf {
    let Ok(out_dir) = env::var("OUT_DIR") else {
        panic!("OUT_DIR is unset, so there is nowhere to put the compiled translation catalogs");
    };
    let root = Path::new(&out_dir).join(LANG_DIR);
    println!("cargo::rustc-env={BUNDLED_CATALOGS_ENV}={}", root.display());
    root
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

#[cfg(feature = "demo")]
fn fetch_demo_assets() {
    println!("cargo::rerun-if-changed={DEMO_ASSETS_SCRIPT}");
    println!("cargo::rerun-if-changed={DEMO_DATA}");

    let output = match Command::new("bash").arg(DEMO_ASSETS_SCRIPT).output() {
        Ok(output) => output,
        Err(e) => {
            println!("cargo::warning=could not run {DEMO_ASSETS_SCRIPT}: {e}");
            return;
        }
    };

    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        let reason = reason.trim().replace('\n', "; ");
        println!(
            "cargo::warning={DEMO_ASSETS_SCRIPT} failed ({reason}). The demo runs without images, \
             falling back to initials."
        );
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

fn update_translations(catalog_root: &Path) {
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
        println!("cargo::rerun-if-changed={po_path}");
        merge_translations(&po_path);
        compile_translations(&po_path, &pkg_name, catalog_root);
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

fn compile_translations(po_path: &str, pkg_name: &str, catalog_root: &Path) {
    let Some(lang) = Path::new(po_path).file_stem().map(|s| s.to_string_lossy()) else {
        return;
    };

    let mo_dir = catalog_root.join(lang.as_ref()).join("LC_MESSAGES");
    if let Err(e) = fs::create_dir_all(&mo_dir) {
        panic!("failed to create {}: {e}", mo_dir.display());
    }

    let mo_path = mo_dir.join(format!("{pkg_name}.mo"));
    match Command::new("msgfmt")
        .arg(po_path)
        .arg("-o")
        .arg(&mo_path)
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => panic!("msgfmt rejected {po_path} ({status}). The catalog is malformed."),
        Err(e) => println!(
            "cargo::warning=could not run msgfmt ({e}), {po_path} will not be compiled and its \
             strings stay untranslated. Install gettext."
        ),
    }
}
