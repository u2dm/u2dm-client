use std::collections::HashSet;
use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use super::{data, room_info};
use crate::domain::room_info::{ADMIN_LEVEL, MODERATOR_LEVEL, MemberRole};
use crate::domain::user_info::{
    DeviceTrust, IdentityTrust, Moderation, ModerationPowers, Pronouns, RoomMembership, UserDevice,
    UserProfile, localpart,
};

const ENV_VAR: &str = "U2DM_DEMO_USER_INFO";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes the user info sheet: how a person's profile, pronouns and avatar arrive",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "pronouns, the profile of someone outside the room and the avatar arrive ~1.5s after the sheet opens, and the avatar exists only once its fetch lands",
            note: "the sheet itself still opens at once, as the store read it waits for does",
        },
        Flag {
            value: "profile-fails",
            effect: "the first read of each person fails, so the Retry card is reachable",
            note: "",
        },
        Flag {
            value: "verified",
            effect: "everyone you share a direct chat with reads as verified",
            note: "",
        },
        Flag {
            value: "dm-fails",
            effect: "starting a chat fails, so the inline error is reachable",
            note: "",
        },
        Flag {
            value: "dm-no-echo",
            effect: "a started chat never reaches the room list, so the Message tile stays on Starting until the sheet closes",
            note: "",
        },
        Flag {
            value: "ignore-fails",
            effect: "ignoring and unignoring fail, so the inline errors are reachable",
            note: "",
        },
        Flag {
            value: "admin",
            effect: "you are an admin in every room, so Remove and Ban show for members and moderators but not for other admins",
            note: "without it you are a plain member and the sheet offers no moderation",
        },
        Flag {
            value: "kick-fails",
            effect: "removing someone fails, so the inline error is reachable",
            note: "",
        },
        Flag {
            value: "ban-fails",
            effect: "banning someone fails, so the inline error is reachable",
            note: "",
        },
        Flag {
            value: "unban-fails",
            effect: "unbanning someone fails, so the inline error is reachable",
            note: "",
        },
        Flag {
            value: "departed",
            effect: "Guest 1 has left and Guest 2 is banned in every room",
            note: "neither posts, so open them with the probe's open_user_info",
        },
        Flag {
            value: "verify-fails",
            effect: "asking someone to verify fails, so the inline error is reachable",
            note: "",
        },
        Flag {
            value: "verify-declined",
            effect: "the person you ask to verify declines 2s later, so the dialog stops",
            note: "",
        },
        Flag {
            value: "verify-unanswered",
            effect: "the person you ask to verify never answers; the request expires 4s later",
            note: "",
        },
    ],
    notes: &[
        "a member's name, avatar and role come from the room info roster; anyone else is outside the room and is named from the global profile",
        "a started chat returns first and reaches the room list ~0.6s later, as a real sync echo does; `slow` also delays starting one",
        "an ignore change replays the open room's timeline ~0.6s later without the ignored person, the way the event cache clears every room when the ignore list echoes back",
        "everyone has sessions: Nadia's old tablet and one of Jordan's are not verified by their owner, and Marcus has no cross-signing identity",
        "the person you ask to verify answers 2s later with the emoji; They match verifies them for the session",
    ],
};

const SLOW_DELAY: Duration = Duration::from_millis(1500);
const LEFT_GUEST: usize = 1;
const BANNED_GUEST: usize = 2;

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub is_slow: bool,
    pub profile_fails_once: bool,
    pub verified: bool,
    pub departed: bool,
    pub dm_fails: bool,
    pub dm_echo_lost: bool,
    pub ignore_fails: bool,
    pub admin: bool,
    pub kick_fails: bool,
    pub ban_fails: bool,
    pub unban_fails: bool,
    pub verify_fails: bool,
    pub verify_answer: VerifyAnswer,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum VerifyAnswer {
    #[default]
    Emoji,
    Declined,
    Unanswered,
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
        is_slow = scenario.is_slow,
        profile_fails_once = scenario.profile_fails_once,
        verified = scenario.verified,
        departed = scenario.departed,
        dm_fails = scenario.dm_fails,
        dm_echo_lost = scenario.dm_echo_lost,
        ignore_fails = scenario.ignore_fails,
        admin = scenario.admin,
        kick_fails = scenario.kick_fails,
        ban_fails = scenario.ban_fails,
        unban_fails = scenario.unban_fails,
        verify_fails = scenario.verify_fails,
        "demo mode: shaping the user info sheet"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.is_slow = true,
        "profile-fails" => scenario.profile_fails_once = true,
        "verified" => scenario.verified = true,
        "departed" => scenario.departed = true,
        "dm-fails" => scenario.dm_fails = true,
        "dm-no-echo" => scenario.dm_echo_lost = true,
        "ignore-fails" => scenario.ignore_fails = true,
        "admin" => scenario.admin = true,
        "kick-fails" => scenario.kick_fails = true,
        "ban-fails" => scenario.ban_fails = true,
        "unban-fails" => scenario.unban_fails = true,
        "verify-fails" => scenario.verify_fails = true,
        "verify-declined" => scenario.verify_answer = VerifyAnswer::Declined,
        "verify-unanswered" => scenario.verify_answer = VerifyAnswer::Unanswered,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause() {
    if scenario().is_slow {
        sleep(SLOW_DELAY).await;
    }
}

fn failed_reads() -> &'static Mutex<HashSet<String>> {
    static FAILED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn profile_fails_now(user_id: &str) -> bool {
    scenario().profile_fails_once
        && failed_reads()
            .lock()
            .is_ok_and(|mut failed| failed.insert(user_id.to_owned()))
}

pub fn avatar(user_id: &str) -> String {
    if scenario().is_slow {
        room_info::withheld_avatar(user_id)
    } else {
        user_id.to_owned()
    }
}

pub fn shape(profile: &mut UserProfile) {
    let scenario = scenario();
    if scenario.departed {
        if *profile.user_id == data::guest_user(LEFT_GUEST) {
            profile.membership = RoomMembership::Left;
        } else if *profile.user_id == data::guest_user(BANNED_GUEST) {
            profile.membership = RoomMembership::Banned;
        }
    }
    if scenario.is_slow {
        profile.pronouns = Pronouns::Unknown;
    }
}

pub fn moderation_fails(action: Moderation) -> bool {
    let scenario = scenario();
    match action {
        Moderation::Kick => scenario.kick_fails,
        Moderation::Ban => scenario.ban_fails,
        Moderation::Unban => scenario.unban_fails,
    }
}

pub fn powers(own: MemberRole, profile: &UserProfile) -> ModerationPowers {
    let own = if scenario().admin {
        MemberRole::Admin
    } else {
        own
    };
    let allowed =
        !profile.is_self && level(own) >= MODERATOR_LEVEL && level(own) > level(profile.role);
    ModerationPowers {
        kick: allowed,
        ban: allowed,
        unban: allowed,
    }
}

fn level(role: MemberRole) -> i64 {
    match role {
        MemberRole::Owner => i64::MAX,
        MemberRole::Admin => ADMIN_LEVEL,
        MemberRole::Moderator => MODERATOR_LEVEL,
        MemberRole::Member => 0,
    }
}

const NO_IDENTITY: &str = "@marcus:matrix.org";
const DEVICE_ID_LEN: usize = 10;
const DEVICE_ID_FILL: &str = "QWERTYUIOPASDFGHJKLZ";

struct DemoDevice {
    suffix: &'static str,
    name: Option<&'static str>,
    signed: bool,
}

const fn signed(suffix: &'static str, name: &'static str) -> DemoDevice {
    DemoDevice {
        suffix,
        name: Some(name),
        signed: true,
    }
}

fn demo_devices(user_id: &str) -> Vec<DemoDevice> {
    match user_id {
        "@sarah:matrix.org" => vec![
            signed("A", "Element X on iPhone"),
            signed("B", "Element Web on Firefox"),
        ],
        "@nadia:matrix.org" => vec![
            signed("A", "Element X on Android"),
            DemoDevice {
                suffix: "B",
                name: Some("Old tablet"),
                signed: false,
            },
        ],
        "@jordan:matrix.org" => vec![
            signed("A", "Element Desktop"),
            signed("B", "Work laptop"),
            DemoDevice {
                suffix: "C",
                name: None,
                signed: false,
            },
        ],
        NO_IDENTITY => vec![DemoDevice {
            suffix: "A",
            name: Some("Fractal"),
            signed: false,
        }],
        _ => vec![signed("A", "Element")],
    }
}

fn device_id(user_id: &str, suffix: &str) -> String {
    let localpart = localpart(user_id).to_uppercase();
    localpart
        .chars()
        .filter(char::is_ascii_alphabetic)
        .chain(DEVICE_ID_FILL.chars())
        .take(DEVICE_ID_LEN - suffix.len())
        .chain(suffix.chars())
        .collect()
}

pub fn has_identity(user_id: &str) -> bool {
    user_id != NO_IDENTITY
}

pub fn verified_from_the_start(user_id: &str) -> bool {
    scenario().verified && data::direct_room_with(user_id).is_some()
}

pub fn devices(user_id: &str, identity: IdentityTrust) -> Vec<UserDevice> {
    demo_devices(user_id)
        .into_iter()
        .map(|device| UserDevice {
            device_id: device_id(user_id, device.suffix),
            name: device.name.map(ToOwned::to_owned),
            trust: match (device.signed, identity) {
                (true, IdentityTrust::Verified) => DeviceTrust::Verified,
                (true, _) => DeviceTrust::SignedByOwner,
                (false, _) => DeviceTrust::NotSignedByOwner,
            },
        })
        .collect()
}

pub struct VerifiedPeople {
    people: Mutex<HashSet<String>>,
    revision: watch::Sender<u64>,
}

impl Default for VerifiedPeople {
    fn default() -> Self {
        Self {
            people: Mutex::default(),
            revision: watch::Sender::new(0),
        }
    }
}

impl VerifiedPeople {
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    pub fn contains(&self, user_id: &str) -> bool {
        self.people
            .lock()
            .is_ok_and(|people| people.contains(user_id))
    }

    pub fn verify(&self, user_id: &str) {
        if let Ok(mut people) = self.people.lock() {
            people.insert(user_id.to_owned());
        }
        self.revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}
