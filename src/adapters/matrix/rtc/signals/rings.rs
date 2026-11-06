use std::result;

use matrix_sdk::Room;
use matrix_sdk::ruma::{OwnedEventId, OwnedUserId};
use serde_json::{Map, Value};

use super::received::Facts;
use super::{AUDIO_INTENT, RING_TYPE, ROOM_SLOT, STICKY_KEY, VIDEO_INTENT};
use crate::adapters::matrix::member_name::named_member;
use crate::adapters::matrix::rtc::REFERENCE;
use crate::adapters::matrix::rtc::members::{Referenced, referenced_membership};
use crate::domain::call::ring::{CallSignal, Ring, RingId, RingKind, expires_ms};
use crate::domain::call::{CallIntent, MemberSession};
use crate::domain::room::RoomId;
use crate::domain::user_info::UserId;
use crate::util::unix_now_ms;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Named,
    Unnamed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Audience {
    You,
    Room,
}

struct Invitation {
    kind: RingKind,
    intent: Option<CallIntent>,
    expires_ms: u64,
    slot: Slot,
    audience: Audience,
    membership: Option<OwnedEventId>,
}

pub(super) async fn ring(own: &OwnedUserId, room: &Room, facts: Facts) -> Option<CallSignal> {
    let room_id = room.room_id();
    let event_id = &facts.event_id;
    let checked = match invitation(own, &facts, unix_now_ms()) {
        Ok(invitation) => confirm(room, &facts, &invitation)
            .await
            .map(|session| (invitation, session)),
        Err(reason) => Err(reason),
    };
    let (invitation, session) = match checked {
        Ok(checked) => checked,
        Err(reason) => {
            tracing::debug!(%room_id, %event_id, reason, "a call notification rings nothing");
            return None;
        }
    };
    tracing::debug!(%room_id, %event_id, kind = ?invitation.kind, "a call notification arrived");
    Some(CallSignal::Rang(Box::new(Ring {
        id: RingId::new(event_id.as_str()),
        room_id: RoomId::new(room_id.as_str()),
        caller: named_member(room, &facts.sender).await,
        session,
        intent: invitation.intent,
        kind: invitation.kind,
        expires_ms: invitation.expires_ms,
    })))
}

fn invitation(
    own: &OwnedUserId,
    facts: &Facts,
    now_ms: u64,
) -> result::Result<Invitation, &'static str> {
    if facts.sender == *own {
        return Err("you sent it");
    }
    let content = &facts.content;
    if content.keys().all(|key| key == STICKY_KEY) {
        return Err("it was withdrawn");
    }
    let slot = slot_of(content)?;
    if content
        .get(STICKY_KEY)
        .is_some_and(|key| key.as_str() != Some(ROOM_SLOT))
    {
        return Err("its sticky key is not its slot");
    }
    let sender_ts = content
        .get("sender_ts")
        .and_then(Value::as_u64)
        .ok_or("its sender_ts is not a whole number")?;
    let lifetime = content
        .get("lifetime")
        .and_then(Value::as_u64)
        .ok_or("its lifetime is not a whole number")?;
    let expires = expires_ms(sender_ts, facts.origin_server_ts.0.into(), lifetime);
    if expires <= now_ms {
        return Err("it has expired");
    }
    let audience = audience_of(own, content)?;
    let membership = membership_of(content)?;
    if slot == Slot::Unnamed && membership.is_none() {
        return Err("it names no slot and references no membership");
    }
    Ok(Invitation {
        kind: kind_of(content),
        intent: intent(content),
        expires_ms: expires,
        slot,
        audience,
        membership,
    })
}

fn slot_of(content: &Map<String, Value>) -> result::Result<Slot, &'static str> {
    match content.get("slot_id") {
        None => Ok(Slot::Unnamed),
        Some(Value::String(slot)) if slot == ROOM_SLOT => Ok(Slot::Named),
        Some(Value::String(slot)) if slot.split('#').count() == 2 => {
            Err("it invites to another slot")
        }
        Some(_) => Err("its slot_id is malformed"),
    }
}

fn audience_of(
    own: &OwnedUserId,
    content: &Map<String, Value>,
) -> result::Result<Audience, &'static str> {
    const MALFORMED: &str = "its m.mentions is malformed";
    let Some(mentions) = content.get("m.mentions") else {
        return Ok(Audience::Room);
    };
    let mentions = mentions.as_object().ok_or(MALFORMED)?;
    let room = match mentions.get("room") {
        None => false,
        Some(room) => room.as_bool().ok_or(MALFORMED)?,
    };
    let users: Vec<&str> = match mentions.get("user_ids") {
        None => Vec::new(),
        Some(users) => users
            .as_array()
            .and_then(|users| users.iter().map(Value::as_str).collect())
            .ok_or(MALFORMED)?,
    };
    if users.contains(&own.as_str()) {
        Ok(Audience::You)
    } else if room {
        Ok(Audience::Room)
    } else if users.is_empty() {
        Err("it mentions nobody")
    } else {
        Err("it mentions someone else")
    }
}

fn membership_of(
    content: &Map<String, Value>,
) -> result::Result<Option<OwnedEventId>, &'static str> {
    let Some(relation) = content.get("m.relates_to") else {
        return Ok(None);
    };
    relation
        .as_object()
        .filter(|relation| relation.get("rel_type").and_then(Value::as_str) == Some(REFERENCE))
        .and_then(|relation| relation.get("event_id"))
        .and_then(Value::as_str)
        .and_then(|event_id| OwnedEventId::try_from(event_id).ok())
        .map(Some)
        .ok_or("its m.relates_to is no reference to an event")
}

fn kind_of(content: &Map<String, Value>) -> RingKind {
    if content.get("notification_type").and_then(Value::as_str) == Some(RING_TYPE) {
        RingKind::Ring
    } else {
        RingKind::Notify
    }
}

async fn confirm(
    room: &Room,
    facts: &Facts,
    invitation: &Invitation,
) -> result::Result<Option<MemberSession>, &'static str> {
    if invitation.audience == Audience::Room
        && !room
            .power_levels_or_default()
            .await
            .user_can_trigger_room_notification(&facts.sender)
    {
        return Err("its sender may not notify the whole room");
    }
    let Some(membership) = &invitation.membership else {
        return Ok(None);
    };
    let referenced = match referenced_membership(room, membership, &facts.sender, unix_now_ms())
        .await
    {
        Ok(referenced) => referenced,
        Err(reason) => {
            tracing::debug!(%membership, reason, "the membership a call notification references does not count");
            return Err("its membership does not count");
        }
    };
    match (referenced, invitation.slot) {
        (Referenced::Current(session), _) | (Referenced::Replaced(session), Slot::Named) => {
            Ok(Some(session))
        }
        (Referenced::Replaced(_), Slot::Unnamed) => {
            Err("it names no slot and its membership is no longer current")
        }
        (Referenced::Unknown, Slot::Named) => Ok(None),
        (Referenced::Unknown, Slot::Unnamed) => {
            Err("it names no slot and its membership could not be read")
        }
    }
}

pub(super) fn decline(room: &Room, facts: &Facts) -> Option<CallSignal> {
    let ring = facts
        .content
        .get("m.relates_to")
        .and_then(Value::as_object)
        .and_then(|relation| relation.get("event_id"))
        .and_then(Value::as_str)?;
    tracing::debug!(room_id = %room.room_id(), sender = %facts.sender, "a call was declined");
    Some(CallSignal::Declined {
        room_id: RoomId::new(room.room_id().as_str()),
        ring: RingId::new(ring),
        by: UserId::new(facts.sender.as_str()),
    })
}

pub(super) fn intent(content: &Map<String, Value>) -> Option<CallIntent> {
    match content.get("m.call.intent").and_then(Value::as_str)? {
        AUDIO_INTENT => Some(CallIntent::Voice),
        VIDEO_INTENT => Some(CallIntent::Video),
        _ => None,
    }
}
