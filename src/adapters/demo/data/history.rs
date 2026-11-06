use std::collections::HashMap;

use super::people::{names_in, person_name};
use super::sent::{body_preview, last_message_only};
use super::spaces::COPY_SUFFIX;
use super::{OWN_NAME_IN_TIMELINES, data, own_user};
use crate::adapters::demo::timeline::{self, scenario};
use crate::adapters::demo::{
    departures, msgtypes, names, polls, reactions, receipts, richtext, shields,
};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::message::{MessageBody, PinnedMessage, Reaction, TimelineMessage};
use crate::domain::room::RoomId;
use crate::util::unix_now_ms;

pub fn messages(room_id: &RoomId) -> Vec<TimelineMessage> {
    let now = unix_now_ms();
    let mut messages = match data().timelines.get(room_id.as_ref()) {
        Some(timeline) => timeline
            .iter()
            .map(|message| message.to_message(own_user(), now))
            .collect(),
        None => last_message_only(room_id),
    };
    if scenario().history_is_long {
        messages = repeated_history(&messages);
    }
    richtext::apply_scenario(&mut messages);
    msgtypes::apply_scenario(&mut messages);
    reactions::apply_scenario(&mut messages);
    polls::apply_scenario(&mut messages);
    departures::apply_scenario(room_id, &mut messages);
    names::apply_scenario(room_id, &mut messages, now);
    shields::apply_scenario(room_id, &mut messages, now);
    name_messages(room_id, &mut messages);
    receipts::apply_scenario(&mut messages);
    mark_first_unread(room_id, &mut messages);
    messages
}

fn referenced_users(messages: &[TimelineMessage]) -> Vec<String> {
    let mut users = Vec::new();
    for message in messages {
        users.push(message.sender.clone());
        users.extend(
            message
                .reactions
                .iter()
                .flat_map(Reaction::named)
                .map(|reactor| reactor.user_id.clone()),
        );
        if let Some(poll) = message.body.poll() {
            users.extend(poll.named_voters().map(|voter| voter.user_id.clone()));
        }
    }
    users
}

fn name_messages(room_id: &RoomId, messages: &mut [TimelineMessage]) {
    let book = names_in(room_id, referenced_users(messages));
    let senders: HashMap<String, String> = messages
        .iter()
        .filter_map(|message| Some((message.event_id.clone()?, message.sender.clone())))
        .collect();
    for message in messages.iter_mut() {
        message.sender_name = book.renamed(&message.sender, &message.sender_name);
        if let Some(reply) = &mut message.reply {
            let quoted = senders
                .get(&reply.event_id)
                .cloned()
                .unwrap_or_else(|| reply.sender.user_id.clone());
            if !quoted.is_empty() {
                reply.sender =
                    NamedUser::new(quoted.clone(), book.renamed(&quoted, &reply.sender.name));
            }
        }
        for reactor in message.reactions.iter_mut().flat_map(Reaction::named_mut) {
            reactor.name = book.of(&reactor.user_id);
        }
        if let MessageBody::Poll(poll) = &mut message.body {
            for voter in poll.named_voters_mut() {
                voter.name = book.of(&voter.user_id);
            }
        }
    }
}

pub fn own_sender_name() -> MemberName {
    MemberName::Unique(OWN_NAME_IN_TIMELINES.to_owned())
}

pub fn own_member_name() -> MemberName {
    MemberName::new(person_name(own_user()).as_deref(), false)
}

fn repeated_history(messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    let mut repeated = Vec::with_capacity(messages.len() * timeline::HISTORY_COPIES);
    for copy in 0..timeline::HISTORY_COPIES {
        for message in messages {
            let mut message = message.clone();
            message.unique_id = format!("{}{COPY_SUFFIX}{copy}", message.unique_id);
            message.event_id = message
                .event_id
                .as_ref()
                .map(|id| format!("{id}{COPY_SUFFIX}{copy}"));
            repeated.push(message);
        }
    }
    repeated
}

pub fn pinned_messages(room_id: &RoomId) -> Vec<PinnedMessage> {
    let pins = room_pins(room_id);
    if pins.is_empty() {
        return Vec::new();
    }
    let messages = messages(room_id);
    let mut rows: Vec<usize> = pins
        .iter()
        .filter_map(|pin| {
            messages
                .iter()
                .rposition(|message| fixture_id(message).is_some_and(|id| id == pin))
        })
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows.into_iter()
        .filter_map(|row| messages.get(row))
        .filter_map(pinned_message)
        .collect()
}

pub fn latest_pinnable(room_id: &RoomId) -> Option<PinnedMessage> {
    messages(room_id).iter().rev().find_map(pinned_message)
}

fn room_pins(room_id: &RoomId) -> &'static [String] {
    data()
        .rooms
        .iter()
        .find(|room| room.id == room_id.as_ref())
        .map_or(&[], |room| room.pinned.as_slice())
}

fn fixture_id(message: &TimelineMessage) -> Option<&str> {
    let event_id = message.event_id.as_deref()?;
    let copied = event_id
        .rsplit_once(COPY_SUFFIX)
        .filter(|(_, copy)| copy.parse::<usize>().is_ok());
    Some(copied.map_or(event_id, |(id, _)| id))
}

pub fn pinned_message(message: &TimelineMessage) -> Option<PinnedMessage> {
    if message.body.service().is_some() {
        return None;
    }
    Some(PinnedMessage {
        event_id: message.event_id.clone()?,
        kind: message.body.preview_kind(),
        body: body_preview(&message.body),
    })
}

fn unread_count(room_id: &RoomId, loaded: usize) -> usize {
    if scenario().unread_boundary_is_unresolved {
        return 0;
    }
    if scenario().read_position_precedes_history {
        return loaded;
    }
    if scenario().history_is_long {
        return loaded * timeline::UNREAD_PORTION_NUMERATOR / timeline::UNREAD_PORTION_DENOMINATOR;
    }
    data()
        .rooms
        .iter()
        .find(|room| room.id == room_id.as_ref())
        .map_or(0, |room| usize::try_from(room.unread).unwrap_or(usize::MAX))
}

fn mark_first_unread(room_id: &RoomId, messages: &mut [TimelineMessage]) {
    let unread = unread_count(room_id, messages.len());
    if unread == 0 {
        return;
    }
    let read_up_to = messages.len().saturating_sub(unread);
    if let Some(message) = messages.iter_mut().skip(read_up_to).find(|m| !m.is_own) {
        message.is_first_unread = true;
    }
}
