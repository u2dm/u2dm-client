use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::sleep;

use super::hands::DemoHand;
use super::news::camera_news;
use super::{DESIGN, EXPIRES_AFTER, JORDAN, KAI, NADIA, NINA, PRIYA, RUST, UNTRUSTED, scenario};
use crate::adapters::demo::{data, room_info};
use crate::domain::call::identity::{MediaIdentity, spellings};
use crate::domain::call::keys::{KeyIndex, MediaKey, OWN_KEY_LEN, ReceivedKey};
use crate::domain::call::media::MediaEvent;
use crate::domain::call::ring::CallSignal;
use crate::domain::call::sfu::SfuId;
use crate::domain::call::video::FeedId;
use crate::domain::call::{
    CallIntent, CallMember, Capabilities, Capability, DeviceId, FocusSelection, MemberKey,
    MemberSession, RoomCall, Transport,
};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::room::{Room, RoomId};
use crate::util::unix_now_ms;

const MEMBERSHIP_LIFETIME_MS: u64 = 14_400_000;

const MARCUS: &str = "@marcus:matrix.org";

const LEO: &str = "@leo:matrix.org";

const VERA: &str = "@vera:matrix.org";

const MEMBER_DEVICE: &str = "DEMOCALLDEVICE";

const SECOND_DEVICE: &str = "DEMOPHONE";

const BRIDGE: &str = PRIYA;

const DEMO_SFU: &str = "https://sfu.demo.local/livekit/jwt";

const REMOTE_SFU: &str = "https://sfu.remote.demo/livekit/jwt";

const HAND_ECHO_LAG: Duration = Duration::from_millis(150);

const DEMO_KEY: [u8; OWN_KEY_LEN] = [7; OWN_KEY_LEN];

#[derive(Clone, Copy)]
pub(super) enum Declares {
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
pub(super) struct Joiner {
    pub(super) user: &'static str,
    pub(super) device: &'static str,
    pub(super) intent: CallIntent,
    pub(super) declares: Declares,
    pub(super) expires_after: Option<Duration>,
    pub(super) sfu: &'static str,
    pub(super) revision: u32,
}

impl Joiner {
    pub(super) const fn member(user: &'static str, intent: CallIntent) -> Self {
        Self {
            user,
            device: MEMBER_DEVICE,
            intent,
            declares: Declares::Nothing,
            expires_after: None,
            sfu: DEMO_SFU,
            revision: 0,
        }
    }

    pub(super) fn is_own(self) -> bool {
        self.user == data::own_user()
    }

    fn is_this_device(self) -> bool {
        self.is_own() && self.device == own_device()
    }

    fn membership_event(self, room_id: &str) -> String {
        format!(
            "$demo-member-{room_id}-{}-{}-{}",
            self.user, self.device, self.revision
        )
    }

    pub(super) fn identity(self) -> MediaIdentity {
        let member_id = format!("{}:{}", self.user, self.device);
        let [legacy, _, _] = spellings(self.user, self.device, &member_id);
        legacy
    }

    pub(super) fn on_main_device(self) -> bool {
        self.device == MEMBER_DEVICE
    }

    fn key(self) -> MemberKey {
        MemberKey::new(self.user, DeviceId::new(self.device))
    }

    fn to_member(self, room_id: &str, started_ms: u64, offset: u64) -> CallMember {
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
            membership_event: Some(self.membership_event(room_id)),
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

pub(super) struct Watcher {
    pub(super) room_id: String,
    pub(super) sfu: String,
    pub(super) events_tx: mpsc::UnboundedSender<MediaEvent>,
}

#[derive(Default)]
pub struct CallBook {
    calls: Mutex<Calls>,
    signals: Mutex<Option<mpsc::Sender<CallSignal>>>,
    pub(super) watchers: Mutex<Vec<Watcher>>,
    pub(super) hands: Mutex<Vec<DemoHand>>,
    pub(super) reactions: AtomicU64,
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
        self.seat();
        if let Ok(mut hands) = self.hands.lock() {
            hands.clear();
        }
        self.hands_before_you(DESIGN);
    }

    fn seat(&self) {
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
                joiner.to_member(
                    room.id.as_ref(),
                    calls.started_ms,
                    u64::try_from(offset).unwrap_or(0),
                )
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

    pub(super) fn join(&self, room_id: &str, joiner: Joiner) {
        if let Ok(mut calls) = self.calls.lock() {
            calls
                .rooms
                .entry(room_id.to_owned())
                .or_default()
                .push(joiner);
        }
        self.tell_watchers(room_id, joiner, true);
    }

    pub(super) fn session_of(&self, room_id: &str, user: &str) -> Option<MemberSession> {
        let calls = self.calls.lock().ok()?;
        let joiners = calls.rooms.get(room_id)?;
        let offset = joiners.iter().position(|joiner| joiner.user == user)?;
        let member = joiners.get(offset)?.to_member(
            room_id,
            calls.started_ms,
            u64::try_from(offset).unwrap_or(0),
        );
        Some(MemberSession {
            member_id: member.member_id,
            created_ms: member.created_ms,
        })
    }

    pub(super) fn leave(&self, room_id: &str, user: &str) {
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
            .and_then(|joiners| joiners.iter_mut().find(|joiner| joiner.is_this_device()))
            .map(|own| {
                own.intent = intent;
                own.revision = own.revision.wrapping_add(1);
            })
            .is_some()
    }

    pub fn own_membership_event(&self, room_id: &str) -> Option<String> {
        let calls = self.calls.lock().ok()?;
        calls
            .rooms
            .get(room_id)?
            .iter()
            .find(|joiner| joiner.is_this_device())
            .map(|own| own.membership_event(room_id))
    }

    pub(super) fn membership_event_of(&self, room_id: &str, user: &str) -> Option<String> {
        let calls = self.calls.lock().ok()?;
        calls
            .rooms
            .get(room_id)?
            .iter()
            .find(|joiner| joiner.user == user && joiner.on_main_device())
            .map(|joiner| joiner.membership_event(room_id))
    }

    pub(super) fn renew(&self, room_id: &str, user: &str) {
        if let Ok(mut calls) = self.calls.lock()
            && let Some(joiners) = calls.rooms.get_mut(room_id)
        {
            for joiner in joiners.iter_mut().filter(|joiner| joiner.user == user) {
                joiner.revision = joiner.revision.wrapping_add(1);
            }
        }
    }

    pub(super) fn say_intent(&self, room_id: &str, user: &str, intent: CallIntent) {
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
            joiners.retain(|joiner| !joiner.is_this_device());
        }
    }

    pub fn cameras_on(&self, room_id: &str, sfu: &str) -> Vec<(MediaIdentity, FeedId)> {
        let Some(id) = sfu_id(sfu) else {
            return Vec::new();
        };
        let here = self.joiners(room_id, |joiner| joiner.sfu == sfu && !joiner.is_own());
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
        self.joiners(room_id, |joiner| !joiner.is_own())
            .into_iter()
            .map(|joiner| (joiner.key(), joiner.identity()))
            .collect()
    }

    pub(super) fn joiners(&self, room_id: &str, keep: impl Fn(&Joiner) -> bool) -> Vec<Joiner> {
        self.calls
            .lock()
            .map(|calls| {
                calls
                    .rooms
                    .get(room_id)
                    .into_iter()
                    .flatten()
                    .filter(|joiner| keep(joiner))
                    .copied()
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

pub(super) fn echo(signal: CallSignal) {
    tokio::spawn(async move {
        sleep(HAND_ECHO_LAG).await;
        book().signal(signal).await;
    });
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
