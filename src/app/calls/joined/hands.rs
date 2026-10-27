use super::{Effect, Joined};
use crate::commands::view::{HandNews, HandView};
use crate::domain::call::MemberKey;
use crate::domain::call::hands::{HandSighting, RaisedHand, Sighted};
use crate::util::unix_now_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum HandRequest {
    Idle,
    Raising { membership_event: String },
    Lowering { reaction: String },
    RaiseFailed,
    LowerFailed,
}

impl Joined {
    fn own_membership_event(&self) -> Option<String> {
        self.members
            .iter()
            .find(|member| MemberKey::of(member) == self.me)
            .and_then(|member| member.membership_event.clone())
    }

    fn hand_up(&self) -> bool {
        self.hands.of(&self.me).is_some()
    }

    pub(super) fn hand_view(&self) -> HandView {
        match &self.hand_request {
            HandRequest::Raising { .. } => HandView::Raising,
            HandRequest::Lowering { .. } => HandView::Lowering,
            HandRequest::RaiseFailed => HandView::RaiseFailed,
            HandRequest::LowerFailed => HandView::LowerFailed,
            HandRequest::Idle if self.hand_up() => HandView::Up,
            HandRequest::Idle if self.own_membership_event().is_some() => HandView::Down,
            HandRequest::Idle => HandView::Unavailable,
        }
    }

    pub(super) fn hand_order(&self) -> Vec<MemberKey> {
        self.hands.order()
    }

    pub(super) fn hands_raised(&self) -> usize {
        self.hands.count()
    }

    pub(super) fn hand_news(&self) -> Option<HandNews> {
        self.hand_news.clone()
    }

    pub(in crate::app::calls) fn raise_hand(&mut self, on: bool) -> Vec<Effect> {
        if matches!(
            self.hand_request,
            HandRequest::Raising { .. } | HandRequest::Lowering { .. }
        ) {
            tracing::debug!(call = self.call, on, "your hand is already on its way");
            return Vec::new();
        }
        self.hand_request = HandRequest::Idle;
        match (on, self.hands.of(&self.me).cloned()) {
            (true, None) => self.ask_to_raise(),
            (false, Some(hand)) => self.ask_to_lower(hand),
            (true, Some(_)) | (false, None) => Vec::new(),
        }
    }

    fn ask_to_raise(&mut self) -> Vec<Effect> {
        let Some(membership_event) = self.own_membership_event() else {
            tracing::info!(
                call = self.call,
                "your call membership has not arrived yet, so no hand is raised"
            );
            return Vec::new();
        };
        tracing::info!(call = self.call, "raising your hand");
        self.hand_request = HandRequest::Raising {
            membership_event: membership_event.clone(),
        };
        vec![Effect::RaiseHand { membership_event }]
    }

    fn ask_to_lower(&mut self, hand: RaisedHand) -> Vec<Effect> {
        tracing::info!(call = self.call, "lowering your hand");
        self.hand_request = HandRequest::Lowering {
            reaction: hand.reaction.clone(),
        };
        vec![Effect::LowerHand {
            reaction: hand.reaction,
        }]
    }

    pub(in crate::app::calls) fn hand_sent(
        &mut self,
        membership_event: String,
        outcome: Result<String, String>,
    ) -> Vec<Effect> {
        let asked = matches!(&self.hand_request, HandRequest::Raising { membership_event: asked } if *asked == membership_event);
        let reaction = match outcome {
            Ok(reaction) => reaction,
            Err(why) => {
                tracing::warn!(call = self.call, "your hand was not raised: {why}");
                if asked {
                    self.hand_request = HandRequest::RaiseFailed;
                }
                return Vec::new();
            }
        };
        if asked {
            self.hand_request = HandRequest::Idle;
        }
        let hand = RaisedHand {
            membership_event,
            reaction,
            raised_ms: unix_now_ms(),
        };
        if self.own_membership_event().as_deref() == Some(hand.membership_event.as_str()) {
            tracing::info!(call = self.call, "your hand is raised");
            self.hands.raise_own(self.me.clone(), hand);
            return Vec::new();
        }
        self.raise_again(&hand)
    }

    pub(in crate::app::calls) fn hand_lowered(&mut self, reaction: &str, ok: bool) {
        let asked = matches!(&self.hand_request, HandRequest::Lowering { reaction: asked } if asked == reaction);
        if !ok {
            tracing::warn!(call = self.call, "your hand was not lowered");
            if asked {
                self.hand_request = HandRequest::LowerFailed;
            }
            return;
        }
        if asked {
            self.hand_request = HandRequest::Idle;
        }
        if self.hands.redacted(reaction).is_some() {
            tracing::info!(call = self.call, "your hand is lowered");
        }
    }

    pub(in crate::app::calls) fn hand_sighted(&mut self, sighting: HandSighting, live: bool) {
        let by = sighting.by.clone();
        match self.hands.sighted(sighting, &self.members) {
            Sighted::NotAMembership => self.stray_hand(&by),
            Sighted::Again(member) | Sighted::Raised(member) if member == self.me => {
                self.own_hand_arrived();
            }
            Sighted::Again(_) => {}
            Sighted::Raised(member) => self.hand_raised(&member, live),
        }
    }

    fn stray_hand(&self, by: &str) {
        tracing::debug!(
            call = self.call,
            sender = by,
            "a raised hand names no current call membership of its sender"
        );
    }

    fn own_hand_arrived(&mut self) {
        tracing::debug!(call = self.call, "your raised hand arrived");
        if self.hand_request == HandRequest::RaiseFailed {
            self.hand_request = HandRequest::Idle;
        }
    }

    fn hand_raised(&mut self, member: &MemberKey, live: bool) {
        tracing::info!(call = self.call, %member, live, "a call participant raised their hand");
        if live {
            self.tell_hand_raised(member);
        }
    }

    fn tell_hand_raised(&mut self, member: &MemberKey) {
        let Some(who) = self
            .members
            .iter()
            .find(|candidate| MemberKey::of(candidate) == *member)
            .map(|candidate| candidate.user.clone())
        else {
            return;
        };
        let serial = self
            .hand_news
            .as_ref()
            .map_or(0, |news| news.serial)
            .wrapping_add(1);
        self.hand_news = Some(HandNews { serial, who });
    }

    pub(in crate::app::calls) fn hand_redacted(&mut self, event: &str) {
        let Some(member) = self.hands.redacted(event) else {
            return;
        };
        if member == self.me {
            tracing::info!(call = self.call, "your hand is lowered");
            if self.hand_request == HandRequest::LowerFailed {
                self.hand_request = HandRequest::Idle;
            }
        } else {
            tracing::info!(call = self.call, %member, "a call participant lowered their hand");
        }
    }

    pub(in crate::app::calls) fn hands_fetched(&mut self, sightings: Vec<HandSighting>) {
        for sighting in sightings {
            self.hand_sighted(sighting, false);
        }
    }

    pub(super) fn hands_follow_members(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        for lowered in self.hands.members_changed(&self.members) {
            if lowered.member == self.me {
                effects.extend(self.raise_again(&lowered.hand));
            } else {
                tracing::debug!(call = self.call, member = %lowered.member, "a raised hand went down with its call membership");
            }
        }
        let unasked = self.hands.unasked(&self.members);
        if !unasked.is_empty() {
            effects.push(Effect::FetchHands(unasked));
        }
        effects
    }

    fn raise_again(&mut self, stale: &RaisedHand) -> Vec<Effect> {
        let mut effects = vec![Effect::LowerHand {
            reaction: stale.reaction.clone(),
        }];
        if self.hand_request != HandRequest::Idle {
            return effects;
        }
        if let Some(membership_event) = self.own_membership_event() {
            tracing::info!(
                call = self.call,
                "your call membership changed, so your hand is raised on the new one"
            );
            self.hand_request = HandRequest::Raising {
                membership_event: membership_event.clone(),
            };
            effects.push(Effect::RaiseHand { membership_event });
        }
        effects
    }
}
