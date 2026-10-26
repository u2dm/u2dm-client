use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use super::{data, room_info};
use crate::domain::call::identity::{MediaIdentity, spellings};
use crate::domain::call::keys::{KeyIndex, MediaKey, OWN_KEY_LEN, ReceivedKey};
use crate::domain::call::media::{ConnectionQuality, MediaEvent, MediaSource};
use crate::domain::call::ring::{CallSignal, Ring, RingId, RingKind};
use crate::domain::call::sfu::{SfuId, SfuRole};
use crate::domain::call::video::{FeedId, VideoKind};
use crate::domain::call::{
    CallIntent, CallMember, Capabilities, Capability, DeviceId, FocusSelection, MemberKey,
    MemberSession, RoomCall, Transport,
};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::room::{Room, RoomId};
use crate::domain::user_info::UserId;
use crate::util::unix_now_ms;

const ENV_VAR: &str = "U2DM_DEMO_CALLS";

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
            value: "reconnecting",
            effect: "5s after connecting the media server reconnects for 3s",
            note: "",
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

pub const RING_AFTER: Duration = Duration::from_secs(3);
const ELSEWHERE_AFTER: Duration = Duration::from_secs(5);
const RING_ENDS_AFTER: Duration = Duration::from_secs(8);
const EXPIRES_AFTER: Duration = Duration::from_secs(20);
const EXPIRY_LAG: Duration = Duration::from_secs(1);
const SLOW_ANSWER: Duration = Duration::from_millis(1500);
const RING_LIFETIME_MS: u64 = 90_000;
const MEMBERSHIP_LIFETIME_MS: u64 = 14_400_000;
const DESIGN: &str = "!design:demo.local";
const RUST: &str = "!rust:demo.local";
const SARAH_CHAT: &str = "!sarah:demo.local";
const MATRIX_HQ: &str = "!matrixhq:demo.local";
const SARAH: &str = "@sarah:matrix.org";
const NADIA: &str = "@nadia:matrix.org";
const JORDAN: &str = "@jordan:matrix.org";
const MARCUS: &str = "@marcus:matrix.org";
const KAI: &str = "@kai:matrix.org";
const LEO: &str = "@leo:matrix.org";
const NINA: &str = "@nina:matrix.org";
const PRIYA: &str = "@priya:matrix.org";
const VERA: &str = "@vera:matrix.org";
const MEMBER_DEVICE: &str = "DEMOCALLDEVICE";
const SECOND_DEVICE: &str = "DEMOPHONE";
const CAMERA_FEED_PREFIX: &str = "demo-camera-";
const SCREEN_FEED_PREFIX: &str = "demo-screen-";
const SCREEN_SHARER: &str = NADIA;
const SCREEN_SHARE_ENDS_AFTER: Duration = Duration::from_secs(6);
const SCREEN_SHARE_RESTARTS_AFTER: Duration = Duration::from_secs(4);
const SECOND_SHARER: &str = JORDAN;
const SECOND_SHARE_AFTER: Duration = Duration::from_secs(4);
const CAMERA_TOGGLER: &str = NADIA;
const CAMERA_TOGGLES_EVERY: Duration = Duration::from_secs(5);
const PORTRAIT_CAMERA_OF: &str = JORDAN;
const LEAVER: &str = JORDAN;
const MEMBER_LEAVES_AFTER: Duration = Duration::from_secs(15);
const BRIDGE: &str = PRIYA;
const INTENT_CHANGES_AFTER: Duration = Duration::from_secs(4);
pub const PICKER_TIME: Duration = Duration::from_secs(1);
pub const UNPLUGGED_AFTER: Duration = Duration::from_secs(3);
pub const PLUGGED_BACK_AFTER: Duration = Duration::from_secs(4);
pub const SHARE_ENDS_AFTER: Duration = Duration::from_secs(5);
pub const OWN_CAMERA_FEED: &str = "demo-own-camera";
pub const OWN_SCREEN_FEED: &str = "demo-own-screen";
pub const PREVIEW_FEED: &str = "demo-camera-preview";
const OTHER_OWN_DEVICE: &str = "DEMOOTHERDEVICE";
const DEMO_SFU: &str = "https://sfu.demo.local/livekit/jwt";
const REMOTE_SFU: &str = "https://sfu.remote.demo/livekit/jwt";
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
pub const CAMERA_LOST_AFTER: Duration = Duration::from_secs(3);
const TROUBLE_AFTER: Duration = Duration::from_secs(3);
const UNRECEIVED_FOR: Duration = Duration::from_secs(8);
const POOR_FOR: Duration = Duration::from_secs(8);
const LOST_AFTER: Duration = Duration::from_secs(7);
const LOST_FOR: Duration = Duration::from_secs(5);
const STALLED_FOR: Duration = Duration::from_secs(8);
pub const UNTRUSTED: &str = JORDAN;
const DEMO_KEY: [u8; OWN_KEY_LEN] = [7; OWN_KEY_LEN];

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
    pub reconnecting: bool,
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
    tracing::info!(flags = raw, "demo mode: shaping calls");
    scenario
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
        "reconnecting" => scenario.reconnecting = true,
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
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

#[derive(Clone, Copy)]
enum Declares {
    Nothing,
    AudioAndVideo,
    AudioOnly,
    Bridge,
}

impl Declares {
    fn capabilities(self) -> Capabilities {
        match self {
            Self::Nothing => Capabilities::Undetermined,
            Self::AudioAndVideo => {
                Capabilities::declared([Capability::RenderAudio, Capability::RenderVideo])
            }
            Self::AudioOnly => Capabilities::declared([Capability::RenderAudio]),
            Self::Bridge => {
                Capabilities::declared([Capability::RenderAudio, Capability::ExposesE2ee])
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Joiner {
    user: &'static str,
    device: &'static str,
    intent: CallIntent,
    declares: Declares,
    expires_after: Option<Duration>,
    sfu: &'static str,
}

impl Joiner {
    const fn member(user: &'static str, intent: CallIntent) -> Self {
        Self {
            user,
            device: MEMBER_DEVICE,
            intent,
            declares: Declares::Nothing,
            expires_after: None,
            sfu: DEMO_SFU,
        }
    }

    fn identity(self) -> MediaIdentity {
        let member_id = format!("{}:{}", self.user, self.device);
        let [legacy, _, _] = spellings(self.user, self.device, &member_id);
        legacy
    }

    fn on_main_device(self) -> bool {
        self.device == MEMBER_DEVICE
    }

    fn key(self) -> MemberKey {
        MemberKey::new(self.user, DeviceId::new(self.device))
    }

    fn to_member(self, started_ms: u64, offset: u64) -> CallMember {
        let created_ms = started_ms.saturating_add(offset);
        let expires_ms = self
            .expires_after
            .map_or(created_ms.saturating_add(MEMBERSHIP_LIFETIME_MS), |after| {
                started_ms.saturating_add(duration_ms(after))
            });
        CallMember {
            user: NamedUser::new(
                self.user,
                MemberName::new(data::person_name(self.user).as_deref(), false),
            ),
            avatar_mxc: Some(room_info::member_avatar(self.user)),
            device: DeviceId::new(self.device),
            member_id: format!("{}:{}", self.user, self.device),
            created_ms,
            expires_ms,
            intent: Some(self.intent),
            capabilities: self.declares.capabilities(),
            transport: Some(Transport {
                service_url: Some(self.sfu.to_owned()),
                sfu_url: None,
            }),
            selection: FocusSelection::MultiSfu,
        }
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Default)]
struct Calls {
    started_ms: u64,
    rooms: HashMap<String, Vec<Joiner>>,
}

struct Watcher {
    room_id: String,
    sfu: String,
    events_tx: mpsc::UnboundedSender<MediaEvent>,
}

#[derive(Default)]
pub struct CallBook {
    calls: Mutex<Calls>,
    signals: Mutex<Option<mpsc::Sender<CallSignal>>>,
    watchers: Mutex<Vec<Watcher>>,
}

pub fn own_transport() -> Transport {
    Transport {
        service_url: Some(DEMO_SFU.to_owned()),
        sfu_url: None,
    }
}

pub fn book() -> &'static CallBook {
    static BOOK: OnceLock<CallBook> = OnceLock::new();
    BOOK.get_or_init(CallBook::default)
}

pub fn shaped(mut room: Room) -> Room {
    book().shape(&mut room);
    room
}

impl CallBook {
    pub fn begin(&self) {
        let shaped = scenario();
        let Ok(mut calls) = self.calls.lock() else {
            return;
        };
        calls.started_ms = unix_now_ms();
        calls.rooms.clear();
        if shaped.active {
            let kai = Joiner {
                expires_after: shaped.expiring.then_some(EXPIRES_AFTER),
                ..Joiner::member(KAI, CallIntent::Voice)
            };
            let declares = |declares| {
                if shaped.capabilities {
                    declares
                } else {
                    Declares::Nothing
                }
            };
            let jordan = Joiner {
                sfu: if shaped.remote_sfu {
                    REMOTE_SFU
                } else {
                    DEMO_SFU
                },
                declares: declares(Declares::AudioOnly),
                ..Joiner::member(JORDAN, CallIntent::Video)
            };
            let nadia = Joiner {
                declares: declares(Declares::AudioAndVideo),
                ..Joiner::member(NADIA, CallIntent::Video)
            };
            let mut design = vec![nadia, jordan];
            if shaped.bridge {
                design.push(Joiner {
                    declares: Declares::Bridge,
                    ..Joiner::member(BRIDGE, CallIntent::Voice)
                });
            }
            if shaped.second_device {
                design.push(Joiner {
                    device: SECOND_DEVICE,
                    ..Joiner::member(NADIA, CallIntent::Video)
                });
            }
            if shaped.crowd {
                design.extend(
                    [LEO, NINA, PRIYA, VERA, MARCUS, KAI]
                        .map(|user| Joiner::member(user, CallIntent::Video)),
                );
            }
            calls.rooms.insert(DESIGN.to_owned(), design);
            calls.rooms.insert(
                RUST.to_owned(),
                vec![Joiner::member(MARCUS, CallIntent::Voice), kai],
            );
        }
    }

    fn shape(&self, room: &mut Room) {
        if scenario().no_permission && room.id.as_ref() == RUST {
            room.may_join_calls = false;
        }
        let Ok(calls) = self.calls.lock() else {
            return;
        };
        let now = unix_now_ms();
        let members: Vec<CallMember> = calls
            .rooms
            .get(room.id.as_ref())
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(offset, joiner)| {
                joiner.to_member(calls.started_ms, u64::try_from(offset).unwrap_or(0))
            })
            .filter(|member| member.expires_ms > now)
            .collect();
        room.call = RoomCall::gather(members).map(Arc::new);
    }

    pub fn follow(&self, signals_tx: mpsc::Sender<CallSignal>) {
        if let Ok(mut signals) = self.signals.lock() {
            *signals = Some(signals_tx);
        }
    }

    fn join(&self, room_id: &str, joiner: Joiner) {
        if let Ok(mut calls) = self.calls.lock() {
            calls
                .rooms
                .entry(room_id.to_owned())
                .or_default()
                .push(joiner);
        }
        self.tell_watchers(room_id, joiner, true);
    }

    fn session_of(&self, room_id: &str, user: &str) -> Option<MemberSession> {
        let calls = self.calls.lock().ok()?;
        let joiners = calls.rooms.get(room_id)?;
        let offset = joiners.iter().position(|joiner| joiner.user == user)?;
        let member = joiners
            .get(offset)?
            .to_member(calls.started_ms, u64::try_from(offset).unwrap_or(0));
        Some(MemberSession {
            member_id: member.member_id,
            created_ms: member.created_ms,
        })
    }

    fn leave(&self, room_id: &str, user: &str) {
        let mut left = Vec::new();
        if let Ok(mut calls) = self.calls.lock()
            && let Some(joiners) = calls.rooms.get_mut(room_id)
        {
            left = joiners
                .iter()
                .filter(|joiner| joiner.user == user)
                .copied()
                .collect();
            joiners.retain(|joiner| joiner.user != user);
        }
        for joiner in left {
            self.tell_watchers(room_id, joiner, false);
        }
    }

    pub fn own_join(&self, room_id: &str, intent: CallIntent) {
        if self.say_own_intent(room_id, intent) {
            return;
        }
        let own = Joiner {
            device: own_device(),
            ..Joiner::member(data::own_user(), intent)
        };
        self.join(room_id, own);
    }

    fn say_own_intent(&self, room_id: &str, intent: CallIntent) -> bool {
        let Ok(mut calls) = self.calls.lock() else {
            return false;
        };
        calls
            .rooms
            .get_mut(room_id)
            .and_then(|joiners| {
                joiners
                    .iter_mut()
                    .find(|joiner| joiner.user == data::own_user() && joiner.device == own_device())
            })
            .map(|own| own.intent = intent)
            .is_some()
    }

    fn say_intent(&self, room_id: &str, user: &str, intent: CallIntent) {
        if let Ok(mut calls) = self.calls.lock()
            && let Some(joiners) = calls.rooms.get_mut(room_id)
        {
            for joiner in joiners.iter_mut().filter(|joiner| joiner.user == user) {
                joiner.intent = intent;
            }
        }
    }

    pub fn leave_own(&self, room_id: &str) {
        if let Ok(mut calls) = self.calls.lock()
            && let Some(joiners) = calls.rooms.get_mut(room_id)
        {
            joiners.retain(|joiner| {
                !(joiner.user == data::own_user() && joiner.device == own_device())
            });
        }
    }

    pub fn watch(&self, room_id: &str, sfu: &str, events_tx: mpsc::UnboundedSender<MediaEvent>) {
        let present: Vec<Joiner> = self
            .calls
            .lock()
            .map(|calls| {
                calls
                    .rooms
                    .get(room_id)
                    .into_iter()
                    .flatten()
                    .filter(|joiner| joiner.sfu == sfu && joiner.user != data::own_user())
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        if let Some(id) = sfu_id(sfu) {
            for joiner in present {
                send_media(
                    &events_tx,
                    MediaEvent::Present {
                        sfu: id.clone(),
                        identity: joiner.identity(),
                        present: true,
                    },
                );
                if let Some(camera) = camera_news(&id, joiner, true) {
                    send_media(&events_tx, camera);
                }
                if let Some(screen) = screen_news(&id, joiner, true) {
                    send_media(&events_tx, screen);
                    end_screen_share_later(&events_tx, &id, joiner);
                }
                share_screen_later(&events_tx, &id, joiner);
                toggle_camera_later(&events_tx, &id, joiner);
            }
        }
        if let Ok(mut watchers) = self.watchers.lock() {
            watchers.retain(|watcher| !watcher.events_tx.is_closed());
            watchers.push(Watcher {
                room_id: room_id.to_owned(),
                sfu: sfu.to_owned(),
                events_tx,
            });
        }
    }

    fn tell_watchers(&self, room_id: &str, joiner: Joiner, present: bool) {
        if joiner.user == data::own_user() {
            return;
        }
        let Some(id) = sfu_id(joiner.sfu) else {
            return;
        };
        let Ok(watchers) = self.watchers.lock() else {
            return;
        };
        for watcher in watchers
            .iter()
            .filter(|watcher| watcher.room_id == room_id && watcher.sfu == joiner.sfu)
        {
            send_media(
                &watcher.events_tx,
                MediaEvent::Present {
                    sfu: id.clone(),
                    identity: joiner.identity(),
                    present,
                },
            );
            if let Some(camera) = camera_news(&id, joiner, present) {
                send_media(&watcher.events_tx, camera);
            }
            if let Some(screen) = screen_news(&id, joiner, present) {
                send_media(&watcher.events_tx, screen);
            }
        }
    }

    pub fn troubles(&self, room_id: &str, sfu: &str, role: SfuRole) -> Vec<Trouble> {
        let Some(id) = sfu_id(sfu) else {
            return Vec::new();
        };
        let here: Vec<Joiner> = self
            .calls
            .lock()
            .map(|calls| {
                calls
                    .rooms
                    .get(room_id)
                    .into_iter()
                    .flatten()
                    .filter(|joiner| joiner.sfu == sfu && joiner.on_main_device())
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        let mut troubles = Vec::new();
        for joiner in here {
            troubles.extend(missing_media(&id, joiner));
            troubles.extend(lost_connection(&id, joiner));
            troubles.extend(stalled_video(&id, joiner));
        }
        if scenario().poor_connection && role == SfuRole::Publish {
            troubles.push(Trouble::own(
                &id,
                TROUBLE_AFTER,
                ConnectionQuality::Poor,
                "demo: your connection is poor",
            ));
            troubles.push(Trouble::own(
                &id,
                TROUBLE_AFTER + POOR_FOR,
                ConnectionQuality::Good,
                "demo: your connection is good again",
            ));
        }
        troubles.sort_by_key(|trouble| trouble.after);
        troubles
    }

    pub fn cameras_on(&self, room_id: &str, sfu: &str) -> Vec<(MediaIdentity, FeedId)> {
        let Some(id) = sfu_id(sfu) else {
            return Vec::new();
        };
        let here: Vec<Joiner> = self
            .calls
            .lock()
            .map(|calls| {
                calls
                    .rooms
                    .get(room_id)
                    .into_iter()
                    .flatten()
                    .filter(|joiner| joiner.sfu == sfu && joiner.user != data::own_user())
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        here.into_iter()
            .filter_map(|joiner| match camera_news(&id, joiner, true)? {
                MediaEvent::Video {
                    identity,
                    feed: Some(feed),
                    ..
                } => Some((identity, feed)),
                _ => None,
            })
            .collect()
    }

    pub fn others_in(&self, room_id: &str) -> Vec<(MemberKey, MediaIdentity)> {
        self.calls
            .lock()
            .map(|calls| {
                calls
                    .rooms
                    .get(room_id)
                    .into_iter()
                    .flatten()
                    .filter(|joiner| joiner.user != data::own_user())
                    .map(|joiner| (joiner.key(), joiner.identity()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn keys_from_others(&self, room_id: &str) -> Vec<CallSignal> {
        let shaped = scenario();
        self.others_in(room_id)
            .into_iter()
            .filter(|(member, _)| !(shaped.undecryptable && member.user == UNTRUSTED))
            .map(|(member, _)| CallSignal::Key {
                room_id: RoomId::new(room_id),
                key: Box::new(ReceivedKey {
                    member,
                    index: KeyIndex::FIRST,
                    key: MediaKey::new(DEMO_KEY),
                    sent_ms: unix_now_ms(),
                }),
            })
            .collect()
    }

    pub async fn signal(&self, signal: CallSignal) {
        let signals_tx = self.signals.lock().ok().and_then(|signals| signals.clone());
        let Some(signals_tx) = signals_tx else {
            tracing::debug!("demo: nobody follows call signals yet");
            return;
        };
        if signals_tx.send(signal).await.is_err() {
            tracing::debug!("demo: the call signals follower is gone");
        }
    }
}

pub enum Step {
    Rooms,
    Signal(CallSignal),
}

pub type Change = Box<dyn FnOnce(&CallBook) -> Vec<Step> + Send>;

pub struct ScriptedChange {
    pub after: Duration,
    pub change: Change,
}

pub fn script() -> Vec<ScriptedChange> {
    let shaped = scenario();
    let mut changes = Vec::new();
    if shaped.ring {
        let intent = if shaped.ring_video {
            CallIntent::Video
        } else {
            CallIntent::Voice
        };
        changes.push(ScriptedChange {
            after: RING_AFTER,
            change: Box::new(move |book: &CallBook| {
                if !shaped.ring_unconfirmed {
                    book.join(SARAH_CHAT, Joiner::member(SARAH, intent));
                }
                vec![
                    Step::Rooms,
                    Step::Signal(ring(book, SARAH_CHAT, SARAH, intent, RingKind::Ring)),
                ]
            }),
        });
        script_ring_endings(shaped, &mut changes);
    }
    if shaped.active && shaped.expiring {
        changes.push(ScriptedChange {
            after: EXPIRES_AFTER + EXPIRY_LAG,
            change: Box::new(|_: &CallBook| vec![Step::Rooms]),
        });
    }
    if shaped.active && shaped.intent_changes {
        changes.push(ScriptedChange {
            after: INTENT_CHANGES_AFTER,
            change: Box::new(|book: &CallBook| {
                tracing::info!(user = KAI, "demo: a member's call intent turns to video");
                book.say_intent(RUST, KAI, CallIntent::Video);
                vec![Step::Rooms]
            }),
        });
    }
    if shaped.active && shaped.member_leaves {
        changes.push(ScriptedChange {
            after: MEMBER_LEAVES_AFTER,
            change: Box::new(|book: &CallBook| {
                tracing::info!(user = LEAVER, "demo: a member leaves the call");
                book.leave(DESIGN, LEAVER);
                vec![Step::Rooms]
            }),
        });
    }
    if shaped.group_call {
        changes.push(ScriptedChange {
            after: RING_AFTER,
            change: Box::new(|book: &CallBook| {
                book.join(MATRIX_HQ, Joiner::member(KAI, CallIntent::Voice));
                vec![
                    Step::Rooms,
                    Step::Signal(ring(
                        book,
                        MATRIX_HQ,
                        KAI,
                        CallIntent::Voice,
                        RingKind::Notify,
                    )),
                ]
            }),
        });
    }
    changes
}

fn script_ring_endings(shaped: Scenario, changes: &mut Vec<ScriptedChange>) {
    if shaped.declined_elsewhere {
        changes.push(ScriptedChange {
            after: RING_AFTER + ELSEWHERE_AFTER,
            change: Box::new(|_: &CallBook| {
                vec![Step::Signal(CallSignal::Declined {
                    room_id: RoomId::new(SARAH_CHAT),
                    ring: ring_id(SARAH_CHAT),
                    by: UserId::new(data::own_user()),
                })]
            }),
        });
    }
    if shaped.answered_elsewhere {
        changes.push(ScriptedChange {
            after: RING_AFTER + ELSEWHERE_AFTER,
            change: Box::new(|book: &CallBook| {
                book.join(
                    SARAH_CHAT,
                    Joiner {
                        device: OTHER_OWN_DEVICE,
                        ..Joiner::member(data::own_user(), CallIntent::Voice)
                    },
                );
                vec![Step::Rooms]
            }),
        });
    }
    if shaped.ring_ends {
        changes.push(ScriptedChange {
            after: RING_AFTER + RING_ENDS_AFTER,
            change: Box::new(|book: &CallBook| {
                book.leave(SARAH_CHAT, SARAH);
                vec![Step::Rooms]
            }),
        });
    }
}

fn ring_id(room_id: &str) -> RingId {
    RingId::new(format!("$demo-ring-{room_id}"))
}

fn ring(
    book: &CallBook,
    room_id: &str,
    caller: &str,
    intent: CallIntent,
    kind: RingKind,
) -> CallSignal {
    CallSignal::Rang(Box::new(Ring {
        id: ring_id(room_id),
        room_id: RoomId::new(room_id),
        caller: NamedUser::new(
            caller,
            MemberName::new(data::person_name(caller).as_deref(), false),
        ),
        session: book.session_of(room_id, caller),
        intent: Some(intent),
        kind,
        expires_ms: unix_now_ms().saturating_add(RING_LIFETIME_MS),
    }))
}

pub fn partner_of(room_id: &str) -> Option<&'static str> {
    (room_id == SARAH_CHAT).then_some(SARAH)
}

pub fn answer_in(room_id: &str) {
    if let Some(partner) = partner_of(room_id) {
        book().join(room_id, Joiner::member(partner, CallIntent::Voice));
    }
}

pub fn own_device() -> &'static str {
    static DEVICE: OnceLock<String> = OnceLock::new();
    DEVICE.get_or_init(|| data::session().device_id)
}

pub fn sfu_id(sfu: &str) -> Option<SfuId> {
    SfuId::of(&Transport {
        service_url: Some(sfu.to_owned()),
        sfu_url: None,
    })
}

fn camera_news(sfu: &SfuId, joiner: Joiner, present: bool) -> Option<MediaEvent> {
    let shaped = scenario();
    let withheld = shaped.unreceived_media && joiner.user == JORDAN;
    (shaped.cameras && !withheld).then(|| video_news(sfu, joiner, VideoKind::Camera, present))
}

fn screen_news(sfu: &SfuId, joiner: Joiner, present: bool) -> Option<MediaEvent> {
    let shaped = scenario();
    let shares_at_once = shaped.screen_share && joiner.user == SCREEN_SHARER;
    let shares_later = shaped.second_share && joiner.user == SECOND_SHARER;
    (joiner.on_main_device() && (shares_at_once || (shares_later && !present)))
        .then(|| video_news(sfu, joiner, VideoKind::Screen, present))
}

fn video_news(sfu: &SfuId, joiner: Joiner, kind: VideoKind, present: bool) -> MediaEvent {
    let prefix = match kind {
        VideoKind::Camera => CAMERA_FEED_PREFIX,
        VideoKind::Screen => SCREEN_FEED_PREFIX,
    };
    MediaEvent::Video {
        sfu: sfu.clone(),
        identity: joiner.identity(),
        kind,
        feed: present.then(|| FeedId::new(feed_name(prefix, joiner))),
    }
}

fn feed_name(prefix: &str, joiner: Joiner) -> String {
    if joiner.on_main_device() {
        format!("{prefix}{}", joiner.user)
    } else {
        format!("{prefix}{}/{}", joiner.user, joiner.device)
    }
}

pub fn is_portrait(feed: &FeedId) -> bool {
    scenario().portrait && feed.as_ref() == format!("{CAMERA_FEED_PREFIX}{PORTRAIT_CAMERA_OF}")
}

fn share_screen_later(events_tx: &mpsc::UnboundedSender<MediaEvent>, sfu: &SfuId, joiner: Joiner) {
    if !(scenario().second_share && joiner.user == SECOND_SHARER && joiner.on_main_device()) {
        return;
    }
    let events_tx = events_tx.clone();
    let started = video_news(sfu, joiner, VideoKind::Screen, true);
    tokio::spawn(async move {
        sleep(SECOND_SHARE_AFTER).await;
        tracing::info!(user = joiner.user, "demo: a second screen share starts");
        send_media(&events_tx, started);
    });
}

fn toggle_camera_later(events_tx: &mpsc::UnboundedSender<MediaEvent>, sfu: &SfuId, joiner: Joiner) {
    let shaped = scenario();
    if !(shaped.cameras
        && shaped.camera_toggles
        && joiner.user == CAMERA_TOGGLER
        && joiner.on_main_device())
    {
        return;
    }
    let events_tx = events_tx.clone();
    let sfu = sfu.clone();
    tokio::spawn(async move {
        let mut on = true;
        while !events_tx.is_closed() {
            sleep(CAMERA_TOGGLES_EVERY).await;
            on = !on;
            tracing::info!(user = joiner.user, on, "demo: a camera switches");
            send_media(&events_tx, video_news(&sfu, joiner, VideoKind::Camera, on));
        }
    });
}

fn end_screen_share_later(
    events_tx: &mpsc::UnboundedSender<MediaEvent>,
    sfu: &SfuId,
    joiner: Joiner,
) {
    if !scenario().screen_share_ends {
        return;
    }
    let events_tx = events_tx.clone();
    let ended = MediaEvent::Video {
        sfu: sfu.clone(),
        identity: joiner.identity(),
        kind: VideoKind::Screen,
        feed: None,
    };
    let restarted = scenario()
        .screen_share_restarts
        .then(|| video_news(sfu, joiner, VideoKind::Screen, true));
    tokio::spawn(async move {
        sleep(SCREEN_SHARE_ENDS_AFTER).await;
        tracing::info!(user = joiner.user, "demo: a screen share ends");
        send_media(&events_tx, ended);
        if let Some(restarted) = restarted {
            sleep(SCREEN_SHARE_RESTARTS_AFTER).await;
            tracing::info!(user = joiner.user, "demo: a screen share starts again");
            send_media(&events_tx, restarted);
        }
    });
}

pub struct Trouble {
    pub after: Duration,
    pub event: MediaEvent,
    pub says: &'static str,
}

impl Trouble {
    fn missing(
        sfu: &SfuId,
        joiner: Joiner,
        after: Duration,
        source: MediaSource,
        missing: bool,
        says: &'static str,
    ) -> Self {
        Self {
            after,
            event: MediaEvent::Missing {
                sfu: sfu.clone(),
                identity: joiner.identity(),
                source,
                missing,
            },
            says,
        }
    }

    fn rated(
        sfu: &SfuId,
        joiner: Joiner,
        after: Duration,
        quality: ConnectionQuality,
        says: &'static str,
    ) -> Self {
        Self {
            after,
            event: MediaEvent::Quality {
                sfu: sfu.clone(),
                identity: joiner.identity(),
                quality,
            },
            says,
        }
    }

    fn own(sfu: &SfuId, after: Duration, quality: ConnectionQuality, says: &'static str) -> Self {
        Self {
            after,
            event: MediaEvent::OwnQuality {
                sfu: sfu.clone(),
                quality,
            },
            says,
        }
    }
}

fn missing_media(sfu: &SfuId, joiner: Joiner) -> Vec<Trouble> {
    let shaped = scenario();
    let mut troubles = Vec::new();
    if joiner.user == JORDAN && shaped.unreceived_audio {
        troubles.push(Trouble::missing(
            sfu,
            joiner,
            TROUBLE_AFTER,
            MediaSource::Microphone,
            true,
            "demo: Jordan's audio is not received",
        ));
        troubles.push(Trouble::missing(
            sfu,
            joiner,
            TROUBLE_AFTER + UNRECEIVED_FOR,
            MediaSource::Microphone,
            false,
            "demo: Jordan's audio arrives",
        ));
    }
    if joiner.user == JORDAN && shaped.unreceived_media {
        troubles.push(Trouble::missing(
            sfu,
            joiner,
            TROUBLE_AFTER,
            MediaSource::Microphone,
            true,
            "demo: Jordan's audio is not received",
        ));
        troubles.push(Trouble::missing(
            sfu,
            joiner,
            TROUBLE_AFTER,
            MediaSource::Camera,
            true,
            "demo: Jordan's camera is not received",
        ));
    }
    if joiner.user == NADIA && shaped.unreceived_media {
        troubles.push(Trouble::missing(
            sfu,
            joiner,
            TROUBLE_AFTER,
            MediaSource::Microphone,
            true,
            "demo: Nadia's audio is not received",
        ));
    }
    troubles
}

fn stalled_video(sfu: &SfuId, joiner: Joiner) -> Vec<Trouble> {
    if !(scenario().video_stalls && joiner.user == JORDAN) {
        return Vec::new();
    }
    let stall = |after: Duration, stalled: bool, says: &'static str| Trouble {
        after,
        event: MediaEvent::Stalled {
            sfu: sfu.clone(),
            identity: joiner.identity(),
            stalled,
        },
        says,
    };
    vec![
        stall(TROUBLE_AFTER, true, "demo: Jordan's camera stops arriving"),
        stall(
            TROUBLE_AFTER + STALLED_FOR,
            false,
            "demo: Jordan's camera flows again",
        ),
    ]
}

fn lost_connection(sfu: &SfuId, joiner: Joiner) -> Vec<Trouble> {
    if !(scenario().connection_lost && joiner.user == JORDAN) {
        return Vec::new();
    }
    vec![
        Trouble::rated(
            sfu,
            joiner,
            TROUBLE_AFTER,
            ConnectionQuality::Poor,
            "demo: Jordan's connection is poor",
        ),
        Trouble::rated(
            sfu,
            joiner,
            LOST_AFTER,
            ConnectionQuality::Lost,
            "demo: Jordan's connection is lost",
        ),
        Trouble::rated(
            sfu,
            joiner,
            LOST_AFTER + LOST_FOR,
            ConnectionQuality::Good,
            "demo: Jordan's connection is good again",
        ),
    ]
}

fn send_media(events_tx: &mpsc::UnboundedSender<MediaEvent>, event: MediaEvent) {
    if events_tx.send(event).is_err() {
        tracing::debug!("demo: the call media session is gone");
    }
}

pub async fn pause() {
    if scenario().slow {
        sleep(SLOW_ANSWER).await;
    }
}

pub fn announcement_fails() -> bool {
    static FAILED: AtomicBool = AtomicBool::new(false);
    scenario().announce_fails && !FAILED.swap(true, Ordering::AcqRel)
}

pub fn cuts_off() -> bool {
    static CUT: AtomicBool = AtomicBool::new(false);
    scenario().cut_off && !CUT.swap(true, Ordering::AcqRel)
}

pub fn loses_camera() -> bool {
    static LOST: AtomicBool = AtomicBool::new(false);
    scenario().camera_lost && !LOST.swap(true, Ordering::AcqRel)
}
