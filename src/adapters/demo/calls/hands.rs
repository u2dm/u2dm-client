use std::time::Duration;

use tokio::time::{Instant, sleep_until};

use super::call_book::{CallBook, echo};
use super::{JORDAN, NADIA, scenario};
use crate::adapters::demo::data;
use crate::domain::call::hands::HandSighting;
use crate::domain::call::ring::CallSignal;
use crate::domain::room::RoomId;
use crate::util::unix_now_ms;

const NADIA_RAISES_AFTER: Duration = Duration::from_secs(3);

const NADIA_LOWERS_AFTER: Duration = Duration::from_secs(9);

const JORDAN_RENEWS_AFTER: Duration = Duration::from_secs(12);

#[derive(Clone)]
pub(super) struct DemoHand {
    room_id: String,
    user: &'static str,
    membership_event: String,
    reaction: String,
    raised_ms: u64,
}

impl DemoHand {
    fn sighting(&self) -> HandSighting {
        HandSighting {
            by: self.user.to_owned(),
            membership_event: self.membership_event.clone(),
            reaction: self.reaction.clone(),
            raised_ms: self.raised_ms,
        }
    }
}

impl CallBook {
    fn put_hand(&self, room_id: &str, user: &'static str, membership_event: String) -> DemoHand {
        let hand = DemoHand {
            room_id: room_id.to_owned(),
            user,
            membership_event,
            reaction: self.next_reaction(),
            raised_ms: unix_now_ms(),
        };
        if let Ok(mut hands) = self.hands.lock() {
            hands.push(hand.clone());
        }
        hand
    }

    fn take_hand(&self, room_id: &str, reaction: &str) -> bool {
        let Ok(mut hands) = self.hands.lock() else {
            return false;
        };
        let before = hands.len();
        hands.retain(|hand| !(hand.room_id == room_id && hand.reaction == reaction));
        hands.len() != before
    }

    fn hand_of(&self, room_id: &str, user: &str) -> Option<String> {
        self.hands
            .lock()
            .ok()?
            .iter()
            .find(|hand| hand.room_id == room_id && hand.user == user)
            .map(|hand| hand.reaction.clone())
    }

    pub fn raise_own_hand(&self, room_id: &str, membership_event: &str) -> String {
        let hand = self.put_hand(room_id, data::own_user(), membership_event.to_owned());
        tracing::info!(
            room_id,
            reaction = hand.reaction,
            "demo: you raised your hand"
        );
        echo(CallSignal::Hand {
            room_id: RoomId::new(room_id),
            sighting: hand.sighting(),
        });
        hand.reaction
    }

    pub fn lower_own_hand(&self, room_id: &str, reaction: &str) -> bool {
        if !self.take_hand(room_id, reaction) {
            return false;
        }
        tracing::info!(room_id, reaction, "demo: you lowered your hand");
        echo(CallSignal::Redacted {
            room_id: RoomId::new(room_id),
            event: reaction.to_owned(),
        });
        true
    }

    pub fn raised_on(&self, room_id: &str, memberships: &[String]) -> Vec<HandSighting> {
        self.hands
            .lock()
            .map(|hands| {
                hands
                    .iter()
                    .filter(|hand| {
                        hand.room_id == room_id && memberships.contains(&hand.membership_event)
                    })
                    .map(DemoHand::sighting)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn hands_before_you(&self, room_id: &str) {
        if !scenario().raised_hands {
            return;
        }
        if self.hand_of(room_id, JORDAN).is_some() {
            return;
        }
        if let Some(membership_event) = self.membership_event_of(room_id, JORDAN) {
            let hand = self.put_hand(room_id, JORDAN, membership_event);
            tracing::info!(
                room_id,
                reaction = hand.reaction,
                "demo: Jordan's hand is raised before you join"
            );
        }
    }

    pub async fn play_hands(&self, room_id: &str, renewed: impl Fn() + Send) {
        if !scenario().raised_hands {
            return;
        }
        let start = Instant::now();
        sleep_until(start + NADIA_RAISES_AFTER).await;
        let Some(reaction) = self.nadia_raises(room_id).await else {
            return;
        };
        sleep_until(start + NADIA_LOWERS_AFTER).await;
        self.nadia_lowers(room_id, reaction).await;
        sleep_until(start + JORDAN_RENEWS_AFTER).await;
        self.jordan_renews(room_id);
        renewed();
    }

    async fn nadia_raises(&self, room_id: &str) -> Option<String> {
        let membership_event = self.membership_event_of(room_id, NADIA)?;
        let hand = self.put_hand(room_id, NADIA, membership_event);
        tracing::info!(
            room_id,
            reaction = hand.reaction,
            "demo: Nadia raises her hand"
        );
        let reaction = hand.reaction.clone();
        self.signal(CallSignal::Hand {
            room_id: RoomId::new(room_id),
            sighting: hand.sighting(),
        })
        .await;
        Some(reaction)
    }

    async fn nadia_lowers(&self, room_id: &str, reaction: String) {
        if !self.take_hand(room_id, &reaction) {
            return;
        }
        tracing::info!(room_id, "demo: Nadia lowers her hand");
        self.signal(CallSignal::Redacted {
            room_id: RoomId::new(room_id),
            event: reaction,
        })
        .await;
    }

    fn jordan_renews(&self, room_id: &str) {
        tracing::info!(room_id, "demo: Jordan's call membership is renewed");
        self.renew(room_id, JORDAN);
        if let Some(reaction) = self.hand_of(room_id, JORDAN) {
            self.take_hand(room_id, &reaction);
        }
    }
}
