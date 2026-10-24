use std::collections::BTreeMap;
use std::sync::Arc;

use matrix_sdk::Room;
use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState;
use matrix_sdk::ruma::events::AnySyncStateEvent;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::events::room::member::MembershipState;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{MilliSecondsSinceUnixEpoch, OwnedUserId};
use serde::Deserialize;
use serde_json::{Map, Value};

use super::http::host_of;
use super::{CALL_MEMBER_TYPE, capability_named};
use crate::adapters::matrix::member_name::named_member_with_avatar;
use crate::domain::call::{
    CallIntent, CallMember, Capabilities, DeviceId, FocusSelection, RoomCall, Transport,
};

const MEMBERSHIP_LIFETIME_MS: u64 = 14_400_000;
const CALL_APPLICATION: &str = "m.call";
const ROOM_SCOPE: &str = "m.room";
const ROOM_CALL_ID: &str = "";
const LIVEKIT_TYPES: [&str; 2] = ["livekit", "m.livekit"];
const MULTI_SFU: &str = "multi_sfu";
const OLDEST_MEMBERSHIP: &str = "oldest_membership";
const AUDIO_INTENT: &str = "audio";
const VIDEO_INTENT: &str = "video";
const NO_TRANSPORT: &str = "no transport";

#[derive(Deserialize)]
struct MembershipEvent {
    sender: OwnedUserId,
    origin_server_ts: MilliSecondsSinceUnixEpoch,
    #[serde(default)]
    content: Map<String, Value>,
}

enum Reading {
    Member(Box<CallMember>),
    Gone(&'static str),
    Rejected(&'static str),
}

pub(in crate::adapters::matrix) async fn room_call(
    room: &Room,
    now_ms: u64,
) -> Option<Arc<RoomCall>> {
    let events = match room
        .get_state_events(StateEventType::from(CALL_MEMBER_TYPE))
        .await
    {
        Ok(events) => events,
        Err(e) => {
            tracing::debug!("could not read the room's call memberships: {e}");
            return None;
        }
    };
    let mut gathered = Gathered::default();
    for event in events {
        if let RawAnySyncOrStrippedState::Sync(raw) = event {
            gathered.take(room, &raw, now_ms).await;
        }
    }
    gathered.log();
    RoomCall::gather(gathered.members).map(Arc::new)
}

#[derive(Default)]
struct Gathered {
    members: Vec<CallMember>,
    gone: BTreeMap<&'static str, usize>,
    left_the_room: usize,
    rejected: usize,
}

impl Gathered {
    async fn take(&mut self, room: &Room, raw: &Raw<AnySyncStateEvent>, now_ms: u64) {
        let Some(event) = parsed(raw) else {
            self.rejected += 1;
            return;
        };
        if left_the_room(room, &event.sender).await {
            self.left_the_room += 1;
            return;
        }
        match read(room, &event, now_ms).await {
            Reading::Member(member) => self.members.push(*member),
            Reading::Gone(reason) => *self.gone.entry(reason).or_default() += 1,
            Reading::Rejected(reason) => {
                tracing::debug!(sender = %event.sender, reason, "a call membership was rejected");
                self.rejected += 1;
            }
        }
    }

    fn log(&self) {
        if self.members.is_empty() && self.rejected == 0 {
            return;
        }
        let described: Vec<String> = self.members.iter().map(described).collect();
        tracing::debug!(
            members = ?described,
            gone = ?self.gone,
            left_the_room = self.left_the_room,
            rejected = self.rejected,
            "read the room's call memberships"
        );
    }
}

fn parsed(raw: &Raw<AnySyncStateEvent>) -> Option<MembershipEvent> {
    raw.deserialize_as_unchecked::<MembershipEvent>()
        .inspect_err(|e| tracing::debug!("a call membership does not parse: {e}"))
        .ok()
}

fn described(member: &CallMember) -> String {
    let via = member
        .transport
        .as_ref()
        .and_then(|transport| {
            transport
                .service_url
                .as_deref()
                .or(transport.sfu_url.as_deref())
        })
        .map_or(NO_TRANSPORT, host_of);
    format!(
        "{} {} via {via} ({:?})",
        member.user.user_id, member.device, member.selection
    )
}

async fn left_the_room(room: &Room, sender: &OwnedUserId) -> bool {
    match room.get_member_no_sync(sender).await {
        Ok(Some(member)) => matches!(
            member.membership(),
            MembershipState::Leave | MembershipState::Ban
        ),
        Ok(None) => false,
        Err(e) => {
            tracing::debug!(%sender, "could not read a call member's membership: {e}");
            false
        }
    }
}

async fn read(room: &Room, event: &MembershipEvent, now_ms: u64) -> Reading {
    let content = &event.content;
    if content.is_empty() {
        return Reading::Gone("it is empty, so its member left");
    }
    if content.contains_key("memberships") {
        return Reading::Rejected("it uses the legacy list format");
    }
    if text(content, "application") != Some(CALL_APPLICATION) {
        return Reading::Gone("it belongs to another application");
    }
    if text(content, "scope").is_some_and(|scope| scope != ROOM_SCOPE)
        || text(content, "call_id") != Some(ROOM_CALL_ID)
    {
        return Reading::Gone("it is not the room's call");
    }
    let Some(device) = text(content, "device_id").filter(|device| !device.is_empty()) else {
        return Reading::Rejected("it names no device");
    };
    let created_ms =
        number(content, "created_ts").unwrap_or_else(|| event.origin_server_ts.0.into());
    let expires_ms =
        created_ms.saturating_add(number(content, "expires").unwrap_or(MEMBERSHIP_LIFETIME_MS));
    if expires_ms <= now_ms {
        return Reading::Gone("it expired");
    }
    let member_id = text(content, "membershipID")
        .map_or_else(|| format!("{}:{device}", event.sender), str::to_owned);
    let (user, avatar_mxc) = named_member_with_avatar(room, &event.sender).await;
    Reading::Member(Box::new(CallMember {
        user,
        avatar_mxc,
        device: DeviceId::new(device),
        member_id,
        created_ms,
        expires_ms,
        intent: intent(content),
        capabilities: capabilities(content),
        transport: preferred_transport(content),
        selection: selection(content),
    }))
}

fn text<'a>(content: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    content.get(key).and_then(Value::as_str)
}

fn number(content: &Map<String, Value>, key: &str) -> Option<u64> {
    content.get(key).and_then(Value::as_u64)
}

fn intent(content: &Map<String, Value>) -> Option<CallIntent> {
    match text(content, "m.call.intent")? {
        AUDIO_INTENT => Some(CallIntent::Voice),
        VIDEO_INTENT => Some(CallIntent::Video),
        _ => None,
    }
}

fn capabilities(content: &Map<String, Value>) -> Capabilities {
    let Some(declared) = content.get("capabilities") else {
        return Capabilities::Undetermined;
    };
    let words: Option<Vec<&str>> = declared
        .as_array()
        .and_then(|list| list.iter().map(Value::as_str).collect());
    let Some(words) = words else {
        tracing::debug!(%declared, "a call membership's capabilities do not parse, so they stay undetermined");
        return Capabilities::Undetermined;
    };
    Capabilities::declared(words.into_iter().filter_map(capability_named))
}

fn preferred_transport(content: &Map<String, Value>) -> Option<Transport> {
    let first = content
        .get("foci_preferred")?
        .as_array()?
        .first()?
        .as_object()?;
    if !text(first, "type").is_some_and(|kind| LIVEKIT_TYPES.contains(&kind)) {
        return None;
    }
    let transport = Transport {
        service_url: text(first, "livekit_service_url").map(str::to_owned),
        sfu_url: text(first, "url").map(str::to_owned),
    };
    (transport.service_url.is_some() || transport.sfu_url.is_some()).then_some(transport)
}

fn selection(content: &Map<String, Value>) -> FocusSelection {
    let chosen = content
        .get("focus_active")
        .and_then(Value::as_object)
        .and_then(|focus| text(focus, "focus_selection"));
    match chosen {
        Some(OLDEST_MEMBERSHIP) => FocusSelection::OldestMembership,
        Some(MULTI_SFU) | None => FocusSelection::MultiSfu,
        Some(other) => {
            tracing::debug!(
                selection = other,
                "a call membership names an unknown focus selection, reading it as multi-SFU"
            );
            FocusSelection::MultiSfu
        }
    }
}
