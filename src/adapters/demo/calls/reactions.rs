use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::time::{Instant, sleep_until};

use super::call_book::{CallBook, echo};
use super::{JORDAN, NADIA, scenario};
use crate::adapters::demo::data;
use crate::domain::call::reactions::{ReactionOption, ReactionSighting};
use crate::domain::call::ring::CallSignal;
use crate::domain::room::RoomId;

const SERVER_AGE: Duration = Duration::from_millis(40);

const BEFORE_YOU_JOINED: Duration = Duration::from_mins(1);

const SCRIPTED_REACTIONS: [ScriptedReaction; 5] = [
    ScriptedReaction {
        user: JORDAN,
        after: Duration::from_secs(2),
        emoji: "\u{1f44b}",
        name: "wave",
        age: BEFORE_YOU_JOINED,
    },
    ScriptedReaction {
        user: NADIA,
        after: Duration::from_secs(3),
        emoji: "\u{1f44d}",
        name: "thumbsup",
        age: SERVER_AGE,
    },
    ScriptedReaction {
        user: JORDAN,
        after: Duration::from_secs(4),
        emoji: "\u{1f38a}",
        name: "party",
        age: SERVER_AGE,
    },
    ScriptedReaction {
        user: JORDAN,
        after: Duration::from_secs(5),
        emoji: "\u{1f44f}",
        name: "clapping",
        age: SERVER_AGE,
    },
    ScriptedReaction {
        user: JORDAN,
        after: Duration::from_secs(8),
        emoji: "\u{2728} sparkles",
        name: "sparkles",
        age: SERVER_AGE,
    },
];

#[derive(Clone, Copy)]
struct ScriptedReaction {
    user: &'static str,
    after: Duration,
    emoji: &'static str,
    name: &'static str,
    age: Duration,
}

impl CallBook {
    pub(super) fn next_reaction(&self) -> String {
        let reaction = self.reactions.fetch_add(1, Ordering::Relaxed);
        format!("$demo-hand-{reaction}")
    }

    fn next_call_reaction(&self) -> String {
        let reaction = self.reactions.fetch_add(1, Ordering::Relaxed);
        format!("$demo-reaction-{reaction}")
    }

    pub fn send_own_reaction(
        &self,
        room_id: &str,
        membership_event: &str,
        reaction: ReactionOption,
    ) -> String {
        let event = self.next_call_reaction();
        tracing::info!(
            room_id,
            reaction = event,
            name = reaction.name,
            "demo: you reacted"
        );
        echo(CallSignal::Reacted {
            room_id: RoomId::new(room_id),
            sighting: ReactionSighting {
                by: data::own_user().to_owned(),
                membership_event: membership_event.to_owned(),
                reaction: event.clone(),
                emoji: reaction.emoji.to_owned(),
                name: reaction.name.to_owned(),
                age: Some(SERVER_AGE),
            },
        });
        event
    }

    pub async fn play_reactions(&self, room_id: &str) {
        if !scenario().reactions {
            return;
        }
        let start = Instant::now();
        for scripted in SCRIPTED_REACTIONS {
            sleep_until(start + scripted.after).await;
            self.react_as(room_id, scripted).await;
        }
    }

    async fn react_as(&self, room_id: &str, scripted: ScriptedReaction) {
        let Some(membership_event) = self.membership_event_of(room_id, scripted.user) else {
            return;
        };
        let event = self.next_call_reaction();
        tracing::info!(
            room_id,
            user = scripted.user,
            reaction = event,
            name = scripted.name,
            "demo: a call participant reacts"
        );
        self.signal(CallSignal::Reacted {
            room_id: RoomId::new(room_id),
            sighting: ReactionSighting {
                by: scripted.user.to_owned(),
                membership_event,
                reaction: event,
                emoji: scripted.emoji.to_owned(),
                name: scripted.name.to_owned(),
                age: Some(scripted.age),
            },
        })
        .await;
    }
}
