#![allow(clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

const LANG_DIR: &str = "lang";
const SRC_DIR: &str = "src";
const GENERATED_UI_CRATE: &str = "u2dm_ui";
const GENERATED_UI_ADAPTER: &str = "src/adapters/ui/compiled.rs";
const BUNDLED_CATALOGS_ENV: &str = "U2DM_BUNDLED_CATALOGS";
const ENUMS_FILE: &str = "ui/enums.slint";
const LUCIDE_LSP_LIB: &str = ".lucide/lib.slint";
#[cfg(feature = "demo")]
const DEMO_ASSETS_SCRIPT: &str = "scripts/gen-demo-assets.sh";
#[cfg(feature = "demo")]
const DEMO_DATA: &str = "assets/demo/data.json";
const CALL_REACTIONS_DOMAIN: &str = "src/domain/call/reactions.rs";
const CALL_REACTIONS_MENU: &str = "ui/screens/chat/components/call/call-reactions-menu.slint";

struct EnumCoverage {
    slint_enum: &'static str,
    branches_in: &'static str,
    falls_through: &'static [&'static str],
}

const fn covers(
    slint_enum: &'static str,
    branches_in: &'static str,
    falls_through: &'static [&'static str],
) -> EnumCoverage {
    EnumCoverage {
        slint_enum,
        branches_in,
        falls_through,
    }
}

const MESSAGES_SLINT: &str = "ui/messages.slint";
const LOGIN_COMMON_SLINT: &str = "ui/screens/login/common.slint";
const ATTACHMENT_DIALOG_SLINT: &str = "ui/screens/chat/components/timeline/attachment-dialog.slint";
const AUDIO_MESSAGE_SLINT: &str =
    "ui/screens/chat/components/timeline/messages/audio-message.slint";
const SHIELD_MARK_SLINT: &str = "ui/screens/chat/components/timeline/shield-mark.slint";
const UTD_MESSAGE_SLINT: &str = "ui/screens/chat/components/timeline/messages/utd-message.slint";
const SPACE_INDEX_SLINT: &str = "ui/screens/chat/components/space-index.slint";
const MESSAGE_BODY_SLINT: &str = "ui/screens/chat/components/timeline/message-body.slint";
const NOTIFY_MODES_SLINT: &str = "ui/screens/chat/components/notify-modes.slint";
const MEMBER_ROLES_SLINT: &str = "ui/screens/chat/components/member-roles.slint";
const USER_INFO_SLINT: &str = "ui/screens/chat/components/user-info.slint";
const IDENTITY_BANNER_SLINT: &str = "ui/screens/chat/components/timeline/identity-banner.slint";
const ENCRYPTION_SLINT: &str = "ui/screens/chat/components/encryption.slint";
const ACCOUNT_SLINT: &str = "ui/screens/chat/components/account.slint";
const AUTH_PROMPT_SLINT: &str = "ui/screens/chat/components/auth-prompt.slint";
const POLL_MESSAGE_SLINT: &str = "ui/screens/chat/components/timeline/messages/poll-message.slint";
const ROOM_LOG_SLINT: &str = "ui/screens/chat/components/room-log.slint";
const SIDEBAR_SLINT: &str = "ui/screens/chat/components/sidebar.slint";
const INVITE_SLINT: &str = "ui/screens/chat/components/invite.slint";
const REMOVAL_SLINT: &str = "ui/screens/chat/components/removal.slint";
const CREATE_ROOM_SLINT: &str = "ui/screens/chat/components/create-room.slint";
const INVITE_PEOPLE_SLINT: &str = "ui/screens/chat/components/invite-people.slint";
const SETTINGS_SLINT: &str = "ui/screens/chat/components/settings/settings.slint";

const ENUM_COVERAGE: &[EnumCoverage] = &[
    covers("SettingsPage", SETTINGS_SLINT, &[]),
    covers("ThemeChoice", MESSAGES_SLINT, &["system"]),
    covers("MotionChoice", MESSAGES_SLINT, &["system"]),
    covers("SendKey", MESSAGES_SLINT, &["enter"]),
    covers("ReceiptVisibility", MESSAGES_SLINT, &["public"]),
    covers("CallPhase", MESSAGES_SLINT, &["live"]),
    covers("CallTileState", MESSAGES_SLINT, &["live"]),
    covers("CallNotice", MESSAGES_SLINT, &["none"]),
    covers("CallRefusal", MESSAGES_SLINT, &["session-unverified"]),
    covers("CallScreen", MESSAGES_SLINT, &["off"]),
    covers("CallScreenRefusal", MESSAGES_SLINT, &["no-portal"]),
    covers("CallMic", MESSAGES_SLINT, &["off"]),
    covers("CallReaction", MESSAGES_SLINT, &["ready"]),
    covers("CallCamera", MESSAGES_SLINT, &["off"]),
    covers("CallDeviceKind", MESSAGES_SLINT, &[]),
    covers("CallDeviceStatus", MESSAGES_SLINT, &["connected"]),
    covers("CallDeviceTrouble", MESSAGES_SLINT, &["failed"]),
    covers("UserMessageKind", MESSAGES_SLINT, &["none"]),
    covers("ServiceKind", MESSAGES_SLINT, &["none"]),
    covers("LoginActivity", LOGIN_COMMON_SLINT, &["idle"]),
    covers("PreviewKind", MESSAGES_SLINT, &["none", "text"]),
    covers("MediaFailure", MESSAGES_SLINT, &["none"]),
    covers("AttachmentKind", ATTACHMENT_DIALOG_SLINT, &[]),
    covers("AudioKind", AUDIO_MESSAGE_SLINT, &[]),
    covers("Delivery", MESSAGES_SLINT, &["none"]),
    covers("ShieldReason", MESSAGES_SLINT, &["none"]),
    covers("Shield", SHIELD_MARK_SLINT, &["none"]),
    covers("DecryptionFailure", UTD_MESSAGE_SLINT, &["unexplained"]),
    covers("ChildAccess", SPACE_INDEX_SLINT, &[]),
    covers("SpaceIndexStatus", SPACE_INDEX_SLINT, &["closed"]),
    covers("MessageKind", MESSAGE_BODY_SLINT, &["service"]),
    covers("NotifyMode", NOTIFY_MODES_SLINT, &[]),
    covers("MemberRole", MEMBER_ROLES_SLINT, &["member"]),
    covers("RoomMembership", USER_INFO_SLINT, &["joined", "unknown"]),
    covers("DirectChat", USER_INFO_SLINT, &["hidden"]),
    covers("UpgradeLink", MESSAGES_SLINT, &["hidden"]),
    covers("IdentityTrust", USER_INFO_SLINT, &["unverified"]),
    covers("VerifyOffer", USER_INFO_SLINT, &["hidden"]),
    covers("DeviceTrust", USER_INFO_SLINT, &[]),
    covers("PresenceLine", MESSAGES_SLINT, &["hidden"]),
    covers("IdentityAlertKind", IDENTITY_BANNER_SLINT, &["none"]),
    covers("EncryptionStanding", ENCRYPTION_SLINT, &[]),
    covers("RecoveryStatus", ENCRYPTION_SLINT, &[]),
    covers("EncryptionStep", ENCRYPTION_SLINT, &[]),
    covers("EncryptionActivity", ENCRYPTION_SLINT, &["none"]),
    covers("AccountStep", ACCOUNT_SLINT, &[]),
    covers("AccountActivity", ACCOUNT_SLINT, &["none"]),
    covers("AuthStage", AUTH_PROMPT_SLINT, &[]),
    covers("PollPhase", POLL_MESSAGE_SLINT, &[]),
    covers("LogLevel", ROOM_LOG_SLINT, &[]),
    covers("RoomRowKind", SIDEBAR_SLINT, &[]),
    covers("InviteKind", INVITE_SLINT, &["room"]),
    covers("RemovalKind", REMOVAL_SLINT, &["kicked"]),
    covers("InviteAnswer", INVITE_SLINT, &["none"]),
    covers("RoomAccess", CREATE_ROOM_SLINT, &[]),
    covers("PeopleSearchStatus", INVITE_PEOPLE_SLINT, &[]),
    covers("InviteeState", INVITE_PEOPLE_SLINT, &["available"]),
];

const SCHEMA_FILE: &str = "src/adapters/ui/schema.rs";

const ENUM_TABLES: &[(&str, &str)] = &[
    ("UserMessageKind", "user_message_kinds"),
    ("ServiceKind", "service_kinds"),
    ("PreviewKind", "preview_kinds"),
    ("MessageKind", "message_kinds"),
    ("PollPhase", "poll_phases"),
    ("AttachmentKind", "attachment_kinds"),
    ("RoomScope", "room_scopes"),
    ("SpaceIndexStatus", "space_index_statuses"),
    ("RoomPreviewStatus", "room_preview_statuses"),
    ("SourceStatus", "source_statuses"),
    ("ReadersStatus", "readers_statuses"),
    ("SourceEncryption", "source_encryptions"),
    ("ChildAccess", "child_accesses"),
    ("NotifyMode", "notify_modes"),
    ("MemberRole", "member_roles"),
    ("RoomMembership", "room_memberships"),
    ("DirectChat", "direct_chats"),
    ("UpgradeLink", "upgrade_links"),
    ("InviteKind", "invite_kinds"),
    ("RemovalKind", "removal_kinds"),
    ("InviteAnswer", "invite_answers"),
    ("Moderation", "pending_moderations"),
    ("IdentityTrust", "identity_trusts"),
    ("VerifyOffer", "verify_offers"),
    ("DeviceTrust", "device_trusts"),
    ("PresenceLine", "presence_lines"),
    ("IdentityAlertKind", "identity_alert_kinds"),
    ("EncryptionStanding", "encryption_standings"),
    ("RecoveryStatus", "recovery_statuses"),
    ("EncryptionStep", "encryption_steps"),
    ("EncryptionActivity", "encryption_activities"),
    ("AuthStage", "auth_stages"),
    ("AccountStep", "account_steps"),
    ("AccountActivity", "account_activities"),
    ("SettingsPage", "settings_pages"),
    ("ThemeChoice", "theme_choices"),
    ("MotionChoice", "motion_choices"),
    ("SendKey", "send_keys"),
    ("ReceiptVisibility", "receipt_visibilities"),
    ("SettingFlag", "setting_flags"),
    ("MemberRowKind", "member_row_kinds"),
    ("RoomRowKind", "room_row_kinds"),
    ("RoomCallMark", "room_call_marks"),
    ("CallOffer", "call_offers"),
    ("CallRefusal", "call_refusals"),
    ("CallPhase", "call_phases"),
    ("CallMic", "call_mics"),
    ("CallCamera", "call_cameras"),
    ("CallScreen", "call_screens"),
    ("CallReaction", "call_reactions"),
    ("CallScreenRefusal", "call_screen_refusals"),
    ("CallTileState", "call_tile_marks"),
    ("CallDeviceKind", "call_device_kinds"),
    ("CallDeviceStatus", "call_device_statuses"),
    ("CallDeviceTrouble", "call_device_troubles"),
    ("CallNotice", "call_notices"),
    ("RosterStatus", "roster_statuses"),
    ("RoomInfoPlacement", "room_info_placements"),
    ("LogLevel", "log_levels"),
    ("AudioKind", "audio_kinds"),
    ("MediaState", "media_states"),
    ("SendState", "send_states"),
    ("Delivery", "deliveries"),
    ("Shield", "shields"),
    ("ShieldReason", "shield_reasons"),
    ("DecryptionFailure", "decryption_failures"),
    ("ReactionSend", "reaction_sends"),
    ("EmojiArt", "emoji_arts"),
    ("SpoilerState", "spoiler_states"),
    ("MediaFailure", "media_failures"),
    ("TimelineState", "timeline_states"),
    ("PackSurface", "pack_surfaces"),
    ("PackOrigin", "pack_origins"),
    ("RoomKind", "room_kinds"),
    ("RoomAccess", "room_accesses"),
    ("CreateRoomStatus", "create_room_statuses"),
    ("PeopleSearchStatus", "people_search_statuses"),
    ("InviteeState", "invitee_states"),
];

fn main() {
    check_enum_branch_coverage();
    check_scenario_catalogs();
    check_call_reaction_options();
    check_generated_ui_stays_in_its_adapter();
    sync_lucide_lsp_lib();

    #[cfg(feature = "demo")]
    fetch_demo_assets();

    compile_bundled_catalogs(&bundled_catalog_root());
}

const APPLY_ARMS: (&str, &str) = ("fn apply(scenario: &mut Scenario, flag: &str) {", "\n}");

const SCENARIO_SOURCES: &[(&str, &[&str])] = &[
    ("src/adapters/demo/timeline.rs", &[]),
    ("src/adapters/demo/pinned.rs", &[]),
    ("src/adapters/demo/reactions.rs", &[]),
    ("src/adapters/demo/polls.rs", &[]),
    ("src/adapters/demo/receipts.rs", &[]),
    ("src/adapters/demo/names.rs", &[]),
    ("src/adapters/demo/shields.rs", &[]),
    ("src/adapters/demo/richtext.rs", &[]),
    ("src/adapters/demo/msgtypes.rs", &[]),
    ("src/adapters/demo/stickers.rs", &[]),
    ("src/adapters/demo/audio.rs", &[]),
    ("src/adapters/demo/videos.rs", &[]),
    ("src/adapters/demo/space_index.rs", &[]),
    ("src/adapters/demo/invites.rs", &[]),
    ("src/adapters/demo/departures.rs", &[]),
    ("src/adapters/demo/upgrades.rs", &[]),
    ("src/adapters/demo/links.rs", &[]),
    ("src/adapters/demo/create_room.rs", &[]),
    ("src/adapters/demo/invite_people.rs", &[]),
    ("src/adapters/demo/typing.rs", &[]),
    ("src/adapters/demo/presence.rs", &[]),
    ("src/adapters/demo/calls.rs", &[]),
    ("src/adapters/demo/notifications.rs", &[]),
    ("src/adapters/demo/room_info.rs", &[]),
    ("src/adapters/demo/user_info.rs", &[]),
    ("src/adapters/demo/identity.rs", &[]),
    ("src/adapters/demo/encryption.rs", &[]),
    ("src/adapters/demo/account.rs", &[]),
    ("src/adapters/demo/message_menu.rs", &[]),
    ("src/adapters/demo/powers.rs", &[]),
    ("src/adapters/demo/logout.rs", &[]),
    ("src/adapters/demo/sync.rs", &[]),
    ("src/adapters/demo/motion.rs", &[]),
    ("src/adapters/demo/settings.rs", &["file=<path>"]),
    (
        "src/adapters/demo/attachments.rs",
        &["pick=<path>", "save-to=<dir>"],
    ),
];

fn check_scenario_catalogs() {
    for &(file, ignored) in SCENARIO_SOURCES {
        println!("cargo::rerun-if-changed={file}");
        let Ok(text) = fs::read_to_string(file) else {
            panic!("failed to read {file}, so its scenario catalog cannot be checked");
        };
        let arms = section(&text, file, APPLY_ARMS, "the flag match");
        let catalog = section(
            &text,
            file,
            ("pub const CATALOG: Scenarios = Scenarios {", "\n};"),
            "the CATALOG const",
        );

        let is_flag = |value: &&str| !value.is_empty() && !value.contains(' ');
        let mut flags: Vec<String> = quoted_literals(arms)
            .filter(is_flag)
            .map(str::to_owned)
            .collect();
        let mut listed: Vec<String> = quoted_literals(catalog)
            .filter(is_flag)
            .map(str::to_owned)
            .collect();
        listed.retain(|value| !ignored.contains(&value.as_str()));
        flags.sort_unstable();
        flags.dedup();
        listed.sort_unstable();
        listed.dedup();

        let missing: Vec<&String> = flags.iter().filter(|f| !listed.contains(f)).collect();
        let extra: Vec<&String> = listed.iter().filter(|l| !flags.contains(l)).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{file} and its CATALOG disagree: the match accepts {missing:?} that the catalog does \
             not list, and the catalog lists {extra:?} that the match does not accept. The catalog \
             is what `--demo-scenarios` reports."
        );
    }
}

fn check_call_reaction_options() {
    println!("cargo::rerun-if-changed={CALL_REACTIONS_DOMAIN}");
    println!("cargo::rerun-if-changed={CALL_REACTIONS_MENU}");
    let (Ok(domain), Ok(menu)) = (
        fs::read_to_string(CALL_REACTIONS_DOMAIN),
        fs::read_to_string(CALL_REACTIONS_MENU),
    ) else {
        panic!(
            "failed to read {CALL_REACTIONS_DOMAIN} or {CALL_REACTIONS_MENU}, so the call \
             reactions cannot be compared"
        );
    };
    let sent: Vec<&str> = quoted_literals(section(
        &domain,
        CALL_REACTIONS_DOMAIN,
        ("pub const REACTION_OPTIONS", "\n];"),
        "the reactions U2DM sends",
    ))
    .collect();
    let offered: Vec<&str> = quoted_literals(section(
        &menu,
        CALL_REACTIONS_MENU,
        ("out property <[CallReactionOption]> options: [", "\n    ];"),
        "the reactions the menu offers",
    ))
    .collect();
    assert!(
        sent == offered,
        "{CALL_REACTIONS_MENU} offers {offered:?}, but {CALL_REACTIONS_DOMAIN} sends {sent:?}: the \
         menu must list REACTION_OPTIONS' names and emoji, in its order"
    );
}

fn between<'a>(text: &'a str, header: &str, footer: &str) -> Option<&'a str> {
    text.split_once(header)
        .and_then(|(_, rest)| rest.split_once(footer))
        .map(|(body, _)| body)
}

fn section<'a>(text: &'a str, file: &str, bounds: (&str, &str), what: &str) -> &'a str {
    let (header, footer) = bounds;
    let Some(body) = between(text, header, footer) else {
        panic!("{file} has no `{header}` followed by `{footer}`, so {what} cannot be checked");
    };
    body
}

fn check_generated_ui_stays_in_its_adapter() {
    let trespassers: Vec<String> = collect_files_recursive(SRC_DIR, "rs")
        .into_iter()
        .filter(|path| path != GENERATED_UI_ADAPTER)
        .filter(|path| names_generated_ui(path))
        .collect();

    assert!(
        trespassers.is_empty(),
        "{trespassers:?} name the `{GENERATED_UI_CRATE}` crate, which only {GENERATED_UI_ADAPTER} \
         may. The generated Slint types used to be a private module inside that adapter, so \
         nothing else could reach them."
    );
}

fn names_generated_ui(path: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|text| text.contains(GENERATED_UI_CRATE))
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
    let Some(body) = between(schema, &header, &footer) else {
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
    let Some(body) = between(enums, &header, "}") else {
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

fn compile_bundled_catalogs(catalog_root: &Path) {
    let pkg_name = env::var("CARGO_PKG_NAME").unwrap_or_default();
    for po_path in collect_files_recursive(LANG_DIR, "po") {
        println!("cargo::rerun-if-changed={po_path}");
        compile_translations(&po_path, &pkg_name, catalog_root);
    }
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
