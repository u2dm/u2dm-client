mod call_book;
mod hands;
mod news;
mod reactions;
mod scripted;
mod trouble;

use std::sync::OnceLock;
use std::time::Duration;

pub(super) use self::call_book::{book, own_device, own_transport, shaped};
pub(super) use self::news::{is_portrait, pause};
pub(super) use self::scripted::{Step, answer_in, partner_of, script};
pub(super) use self::trouble::{
    Trouble, announcement_fails, cuts_off, fails_to_encrypt, loses_camera,
};
use super::catalog::{Flag, Scenarios, scenario_from_env};

const ENV_VAR: &str = "U2DM_DEMO_CALLS";

const SHAPING: &str = "demo mode: shaping calls";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "starts MatrixRTC calls in demo rooms and rings you",
    combinable: true,
    flags: &[
        Flag {
            value: "active",
            effect: "a video call runs in Design Team (Nadia, Jordan) and a voice call in Rust Programming (Marcus, Kai)",
            note: "",
        },
        Flag {
            value: "ring",
            effect: "Sarah joins a voice call in her chat 3s after sync and rings you",
            note: "her membership lands with the ring, so the card shows at once",
        },
        Flag {
            value: "ring-video",
            effect: "with ring: the call and the ring are video",
            note: "",
        },
        Flag {
            value: "ring-unconfirmed",
            effect: "with ring: Sarah's membership never shows, so nothing rings",
            note: "the ring is dropped after its 3s window",
        },
        Flag {
            value: "ring-ends",
            effect: "with ring: Sarah leaves the call 8s after ringing",
            note: "",
        },
        Flag {
            value: "declined-elsewhere",
            effect: "with ring: your other device declines 5s after the ring",
            note: "",
        },
        Flag {
            value: "answered-elsewhere",
            effect: "with ring: your other device joins the call 5s after the ring",
            note: "",
        },
        Flag {
            value: "group-call",
            effect: "Kai starts a call in Matrix HQ 3s after sync and notifies the room",
            note: "a notification, not a ring: nothing rings",
        },
        Flag {
            value: "expiring",
            effect: "with active: Kai's membership in Rust Programming expires 20s after sync, as a crashed client's does",
            note: "",
        },
        Flag {
            value: "no-permission",
            effect: "joining calls in Rust Programming needs a power level you lack",
            note: "",
        },
        Flag {
            value: "decline-fails",
            effect: "declining a call fails",
            note: "",
        },
        Flag {
            value: "slow",
            effect: "declining, joining and token requests take ~1.5s",
            note: "",
        },
        Flag {
            value: "no-sfu",
            effect: "the homeserver names no call server, so calls cannot be joined",
            note: "",
        },
        Flag {
            value: "delayed-unsupported",
            effect: "the homeserver has no delayed events: the join goes on without a delayed leave",
            note: "",
        },
        Flag {
            value: "grant-fails",
            effect: "the call server refuses the token request with 403",
            note: "",
        },
        Flag {
            value: "connect-fails",
            effect: "the media server refuses the connection",
            note: "",
        },
        Flag {
            value: "cut-off",
            effect: "the media server drops your first call 5s after connecting",
            note: "the call is kept as failed, and Try again joins it for good",
        },
        Flag {
            value: "not-encrypted",
            effect: "your media cannot be encrypted 3s after your first call in an encrypted room connects",
            note: "the call stops and is kept as failed; Try again joins it with what you were sending, and it encrypts",
        },
        Flag {
            value: "reconnecting",
            effect: "5s after connecting the media server reconnects for 3s",
            note: "",
        },
        Flag {
            value: "full-reconnect",
            effect: "with reconnecting: the reconnection is a full one: the others' media and presence on the media server go before it says it reconnects, and come back as they joined once it is connected",
            note: "the order LiveKit's full reconnect reports; a share you pinned stays pinned through it",
        },
        Flag {
            value: "keys-late",
            effect: "the other members' keys arrive 4s after you join",
            note: "their tiles wait for keys until then",
        },
        Flag {
            value: "undecryptable",
            effect: "Jordan's device is not cross-signed and never sends a key that works",
            note: "",
        },
        Flag {
            value: "answers",
            effect: "Sarah joins a call you start in her chat 3s after it rings",
            note: "",
        },
        Flag {
            value: "declines",
            effect: "Sarah declines a call you start in her chat 3s after it rings",
            note: "",
        },
        Flag {
            value: "announce-fails",
            effect: "the first call you start is not announced: its ring or room notification fails to send",
            note: "Try again sends it; with answers, Sarah then picks up",
        },
        Flag {
            value: "speaking",
            effect: "the people in your call take turns speaking every 2s",
            note: "",
        },
        Flag {
            value: "speeches",
            effect: "with active: Nadia speaks 4s, Jordan 0.5s, Nadia 3s, Jordan 4s, and again; you never speak",
            note: "Jordan's 0.5s interjection is shorter than the stage's speaker settle",
        },
        Flag {
            value: "capabilities",
            effect: "with active: Nadia's membership declares m.render_audio and m.render_video, Jordan's only m.render_audio",
            note: "without it no member declares capabilities, as Element Call does",
        },
        Flag {
            value: "bridge",
            effect: "with active: Priya joins Design Team's call as a bridge declaring m.render_audio and m.exposes_e2ee",
            note: "",
        },
        Flag {
            value: "intent-changes",
            effect: "with active: Kai's intent in Rust Programming's voice call turns to video 4s after sync",
            note: "the room's call then reads as a video call",
        },
        Flag {
            value: "transports-refused",
            effect: "the homeserver refuses to name its call transports (a 403), so no call can be joined",
            note: "",
        },
        Flag {
            value: "crowd",
            effect: "with active: eight people are in the Design Team call",
            note: "",
        },
        Flag {
            value: "second-device",
            effect: "with active: Nadia is in the Design Team call from a second device too",
            note: "its camera is a feed of its own; it never shares a screen",
        },
        Flag {
            value: "remote-sfu",
            effect: "with active: Jordan publishes to another homeserver's media server",
            note: "",
        },
        Flag {
            value: "cameras",
            effect: "the people in your call have their cameras on",
            note: "",
        },
        Flag {
            value: "no-camera",
            effect: "this computer has no camera, so turning yours on fails",
            note: "",
        },
        Flag {
            value: "camera-fails",
            effect: "your camera is busy, so turning it on fails",
            note: "",
        },
        Flag {
            value: "microphone-fails",
            effect: "your microphone is busy, so publishing it fails",
            note: "",
        },
        Flag {
            value: "camera-lost",
            effect: "your camera stops sending pictures 3s after it first starts",
            note: "Try again starts it for good",
        },
        Flag {
            value: "audio-tests-fail",
            effect: "the call sheet's microphone and speaker tests do not start",
            note: "",
        },
        Flag {
            value: "preview-lost",
            effect: "the camera the call sheet previews stops sending pictures 3s after it starts",
            note: "switching the sheet's camera off and on starts it again",
        },
        Flag {
            value: "screen-share",
            effect: "with active: Nadia shares her screen in the Design Team call",
            note: "her share takes the stage and the others move to the strip or rail beside it",
        },
        Flag {
            value: "screen-share-ends",
            effect: "with screen-share: Nadia stops sharing 6s after you join",
            note: "",
        },
        Flag {
            value: "screen-share-restarts",
            effect: "with screen-share-ends: Nadia shares her screen again 4s after she stopped",
            note: "",
        },
        Flag {
            value: "second-share",
            effect: "with screen-share: Jordan shares his screen too, 4s after you join",
            note: "two shares at once: the first keeps the stage and the call offers the second",
        },
        Flag {
            value: "camera-toggles",
            effect: "with cameras: Nadia's camera goes off 5s after you join and back on 5s later, over and over",
            note: "",
        },
        Flag {
            value: "portrait",
            effect: "with cameras: Jordan's camera sends portrait pictures",
            note: "",
        },
        Flag {
            value: "member-leaves",
            effect: "with active: Jordan leaves the Design Team call 15s after sync",
            note: "",
        },
        Flag {
            value: "no-screen-capture",
            effect: "this desktop has no screen-cast portal, so the share button is disabled",
            note: "",
        },
        Flag {
            value: "share-cancelled",
            effect: "the picker for your screen share is closed without a choice",
            note: "the share goes back to off without an error",
        },
        Flag {
            value: "change-cancelled",
            effect: "every picker after the first one of a call is closed without a choice",
            note: "Change shared screen… stops the share and then says that sharing stopped",
        },
        Flag {
            value: "share-ends",
            effect: "the desktop ends your screen share 5s after it starts",
            note: "",
        },
        Flag {
            value: "headset-unplugs",
            effect: "the USB Headset is unplugged 3s after you first choose it as your microphone or speaker, and plugged back in 4s later",
            note: "the call uses the system default while it is gone and the headset again once it is back",
        },
        Flag {
            value: "headset-fails",
            effect: "the USB Headset cannot be opened, so choosing it as your microphone or speaker fails",
            note: "the call keeps the system default and the device menu says the headset could not be used",
        },
        Flag {
            value: "webcam-unplugs",
            effect: "the USB Webcam is unplugged 3s after you first choose it, and plugged back in 4s later",
            note: "a camera capturing it stops, as an unplugged one does",
        },
        Flag {
            value: "unreceived-audio",
            effect: "Jordan's audio is not received from 3s after you connect, and arrives 8s later",
            note: "the dock names him and his tile and people row say what is missing",
        },
        Flag {
            value: "unreceived-media",
            effect: "Jordan's audio and camera, and Nadia's audio, are never received from 3s after you connect",
            note: "Jordan's camera picture is withheld; the dock names Nadia and one other person",
        },
        Flag {
            value: "poor-connection",
            effect: "your own connection is rated poor from 3s after you connect, for 8s",
            note: "the media server's rating as it reaches the app, already held",
        },
        Flag {
            value: "connection-lost",
            effect: "Jordan's connection is rated poor 3s after you connect, lost at 7s and good again at 12s",
            note: "his tile hides its frozen picture while the connection is lost",
        },
        Flag {
            value: "video-stalls",
            effect: "Jordan's camera stops arriving 3s after you connect, for 8s, then flows again",
            note: "with cameras; his tile shows his avatar and says the video stopped arriving",
        },
        Flag {
            value: "raised-hands",
            effect: "Jordan's hand is already raised when you join; Nadia raises hers 3s after you join and lowers it at 9s; Jordan's membership is renewed at 12s, which lowers his",
            note: "your own raise and lower echo back like the server's; a renewed membership of yours lowers your hand until it is raised again",
        },
        Flag {
            value: "reactions",
            effect: "Jordan's 👋 sent a minute before you joined arrives 2s after you join and is dropped; Nadia reacts 👍 at 3s; Jordan sends Element Call's party name with 🎊 at 4s (shown as 🎉) and 👏 at 5s, dropped while his first still shows; at 8s he sends ✨ under a name outside Element Call's set",
            note: "your own reactions echo back like the server's; each reaction shows for 3s",
        },
    ],
    notes: &[
        "each decline logs `demo: the call is declined`",
        "call facets show in /state at .view.directory.rooms[].call",
        "joining logs `demo: you joined the call`, leaving `demo: you left the call`",
        "the media is fake: it never opens a microphone, speaker or camera; video tiles show generated pictures",
        "sharing your screen stands in for the portal's picker with a 1s wait, then shares nothing real",
        "the device menu lists two microphones, three speakers and two cameras; none of them exists",
    ],
};

const EXPIRES_AFTER: Duration = Duration::from_secs(20);

const SLOW_ANSWER: Duration = Duration::from_millis(1500);

const DESIGN: &str = "!design:demo.local";

const RUST: &str = "!rust:demo.local";

const SARAH: &str = "@sarah:matrix.org";

const NADIA: &str = "@nadia:matrix.org";

const JORDAN: &str = "@jordan:matrix.org";

const KAI: &str = "@kai:matrix.org";

const NINA: &str = "@nina:matrix.org";

const PRIYA: &str = "@priya:matrix.org";

const SECOND_SHARER: &str = JORDAN;

pub const PICKER_TIME: Duration = Duration::from_secs(1);

pub const UNPLUGGED_AFTER: Duration = Duration::from_secs(3);

pub const PLUGGED_BACK_AFTER: Duration = Duration::from_secs(4);

pub const SHARE_ENDS_AFTER: Duration = Duration::from_secs(5);

pub const OWN_CAMERA_FEED: &str = "demo-own-camera";

pub const OWN_SCREEN_FEED: &str = "demo-own-screen";

pub const PREVIEW_FEED: &str = "demo-camera-preview";

pub const DEMO_LIVEKIT: &str = "wss://livekit.demo.local";

pub const ANSWER_AFTER: Duration = Duration::from_secs(3);

pub const KEYS_LATE_BY: Duration = Duration::from_secs(4);

pub const RECONNECT_AFTER: Duration = Duration::from_secs(5);

pub const RECONNECT_FOR: Duration = Duration::from_secs(3);

pub const SPEAKING_TURN: Duration = Duration::from_secs(2);

pub const SPEECHES: [(&str, Duration); 4] = [
    (NADIA, Duration::from_secs(4)),
    (JORDAN, Duration::from_millis(500)),
    (NADIA, Duration::from_secs(3)),
    (JORDAN, Duration::from_secs(4)),
];

pub const CUT_OFF_AFTER: Duration = Duration::from_secs(5);

pub const NOT_ENCRYPTED_AFTER: Duration = Duration::from_secs(3);

pub const CAMERA_LOST_AFTER: Duration = Duration::from_secs(3);

pub const UNTRUSTED: &str = JORDAN;

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    active: bool,
    ring: bool,
    ring_video: bool,
    ring_unconfirmed: bool,
    ring_ends: bool,
    declined_elsewhere: bool,
    answered_elsewhere: bool,
    group_call: bool,
    expiring: bool,
    no_permission: bool,
    pub decline_fails: bool,
    pub slow: bool,
    pub no_sfu: bool,
    pub delayed_unsupported: bool,
    pub grant_fails: bool,
    pub connect_fails: bool,
    cut_off: bool,
    not_encrypted: bool,
    pub reconnecting: bool,
    full_reconnect: bool,
    pub keys_late: bool,
    pub undecryptable: bool,
    pub answers: bool,
    pub declines: bool,
    announce_fails: bool,
    pub speaking: bool,
    pub speeches: bool,
    capabilities: bool,
    bridge: bool,
    intent_changes: bool,
    pub transports_refused: bool,
    crowd: bool,
    second_device: bool,
    remote_sfu: bool,
    cameras: bool,
    pub no_camera: bool,
    pub camera_fails: bool,
    pub microphone_fails: bool,
    camera_lost: bool,
    pub preview_lost: bool,
    pub audio_tests_fail: bool,
    screen_share: bool,
    screen_share_ends: bool,
    screen_share_restarts: bool,
    second_share: bool,
    camera_toggles: bool,
    portrait: bool,
    member_leaves: bool,
    pub no_screen_capture: bool,
    pub share_cancelled: bool,
    pub change_cancelled: bool,
    pub share_ends: bool,
    pub headset_unplugs: bool,
    pub headset_fails: bool,
    pub webcam_unplugs: bool,
    unreceived_audio: bool,
    unreceived_media: bool,
    poor_connection: bool,
    connection_lost: bool,
    video_stalls: bool,
    pub raised_hands: bool,
    reactions: bool,
}

pub fn scenario() -> Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    *SCENARIO.get_or_init(|| scenario_from_env(ENV_VAR, apply, SHAPING))
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "active" => scenario.active = true,
        "ring" => scenario.ring = true,
        "ring-video" => scenario.ring_video = true,
        "ring-unconfirmed" => scenario.ring_unconfirmed = true,
        "ring-ends" => scenario.ring_ends = true,
        "declined-elsewhere" => scenario.declined_elsewhere = true,
        "answered-elsewhere" => scenario.answered_elsewhere = true,
        "group-call" => scenario.group_call = true,
        "expiring" => scenario.expiring = true,
        "no-permission" => scenario.no_permission = true,
        "decline-fails" => scenario.decline_fails = true,
        "slow" => scenario.slow = true,
        "no-sfu" => scenario.no_sfu = true,
        "delayed-unsupported" => scenario.delayed_unsupported = true,
        "grant-fails" => scenario.grant_fails = true,
        "connect-fails" => scenario.connect_fails = true,
        "cut-off" => scenario.cut_off = true,
        "not-encrypted" => scenario.not_encrypted = true,
        "reconnecting" => scenario.reconnecting = true,
        "full-reconnect" => scenario.full_reconnect = true,
        "keys-late" => scenario.keys_late = true,
        "undecryptable" => scenario.undecryptable = true,
        "answers" => scenario.answers = true,
        "declines" => scenario.declines = true,
        "announce-fails" => scenario.announce_fails = true,
        "speaking" => scenario.speaking = true,
        "speeches" => scenario.speeches = true,
        "capabilities" => scenario.capabilities = true,
        "bridge" => scenario.bridge = true,
        "intent-changes" => scenario.intent_changes = true,
        "transports-refused" => scenario.transports_refused = true,
        "crowd" => scenario.crowd = true,
        "second-device" => scenario.second_device = true,
        "remote-sfu" => scenario.remote_sfu = true,
        "cameras" => scenario.cameras = true,
        "no-camera" => scenario.no_camera = true,
        "camera-fails" => scenario.camera_fails = true,
        "microphone-fails" => scenario.microphone_fails = true,
        "camera-lost" => scenario.camera_lost = true,
        "preview-lost" => scenario.preview_lost = true,
        "audio-tests-fail" => scenario.audio_tests_fail = true,
        "screen-share" => scenario.screen_share = true,
        "screen-share-ends" => scenario.screen_share_ends = true,
        "screen-share-restarts" => scenario.screen_share_restarts = true,
        "second-share" => scenario.second_share = true,
        "camera-toggles" => scenario.camera_toggles = true,
        "portrait" => scenario.portrait = true,
        "member-leaves" => scenario.member_leaves = true,
        "no-screen-capture" => scenario.no_screen_capture = true,
        "share-cancelled" => scenario.share_cancelled = true,
        "change-cancelled" => scenario.change_cancelled = true,
        "share-ends" => scenario.share_ends = true,
        "headset-unplugs" => scenario.headset_unplugs = true,
        "headset-fails" => scenario.headset_fails = true,
        "webcam-unplugs" => scenario.webcam_unplugs = true,
        "unreceived-audio" => scenario.unreceived_audio = true,
        "unreceived-media" => scenario.unreceived_media = true,
        "poor-connection" => scenario.poor_connection = true,
        "connection-lost" => scenario.connection_lost = true,
        "video-stalls" => scenario.video_stalls = true,
        "raised-hands" => scenario.raised_hands = true,
        "reactions" => scenario.reactions = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}
