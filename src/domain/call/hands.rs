use std::collections::{HashMap, HashSet};

use super::{CallMember, MemberKey};

pub const RAISED_HAND_KEY: &str = "\u{1f590}\u{fe0f}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandSighting {
    pub by: String,
    pub membership_event: String,
    pub reaction: String,
    pub raised_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaisedHand {
    pub membership_event: String,
    pub reaction: String,
    pub raised_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lowered {
    pub member: MemberKey,
    pub hand: RaisedHand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sighted {
    NotAMembership,
    Raised(MemberKey),
    Again(MemberKey),
}

#[derive(Debug, Default)]
pub struct Hands {
    raised: HashMap<MemberKey, RaisedHand>,
    asked: HashSet<String>,
}

impl Hands {
    pub fn sighted(&mut self, sighting: HandSighting, members: &[CallMember]) -> Sighted {
        let Some(member) = members.iter().find(|member| {
            member.user.user_id == sighting.by
                && member.membership_event.as_deref() == Some(sighting.membership_event.as_str())
        }) else {
            return Sighted::NotAMembership;
        };
        let key = MemberKey::of(member);
        let hand = RaisedHand {
            membership_event: sighting.membership_event,
            reaction: sighting.reaction,
            raised_ms: sighting.raised_ms,
        };
        match self.raised.insert(key.clone(), hand) {
            None => Sighted::Raised(key),
            Some(_) => Sighted::Again(key),
        }
    }

    pub fn redacted(&mut self, event: &str) -> Option<MemberKey> {
        let key = self
            .raised
            .iter()
            .find(|(_, hand)| hand.reaction == event)
            .map(|(key, _)| key.clone())?;
        self.raised.remove(&key);
        Some(key)
    }

    pub fn members_changed(&mut self, members: &[CallMember]) -> Vec<Lowered> {
        let current: HashMap<MemberKey, Option<&str>> = members
            .iter()
            .map(|member| (MemberKey::of(member), member.membership_event.as_deref()))
            .collect();
        let mut lowered = Vec::new();
        self.raised.retain(|member, hand| {
            let current_membership =
                current.get(member).copied().flatten() == Some(hand.membership_event.as_str());
            if !current_membership {
                lowered.push(Lowered {
                    member: member.clone(),
                    hand: hand.clone(),
                });
            }
            current_membership
        });
        lowered
    }

    pub fn unasked(&mut self, members: &[CallMember]) -> Vec<String> {
        members
            .iter()
            .filter_map(|member| member.membership_event.clone())
            .filter(|event| self.asked.insert(event.clone()))
            .collect()
    }

    pub fn of(&self, member: &MemberKey) -> Option<&RaisedHand> {
        self.raised.get(member)
    }

    pub fn order(&self) -> Vec<MemberKey> {
        let mut raised: Vec<(&MemberKey, &RaisedHand)> = self.raised.iter().collect();
        raised.sort_by(|(_, a), (_, b)| {
            a.raised_ms
                .cmp(&b.raised_ms)
                .then_with(|| a.reaction.cmp(&b.reaction))
        });
        raised.into_iter().map(|(key, _)| key.clone()).collect()
    }

    pub fn raise_own(&mut self, me: MemberKey, hand: RaisedHand) {
        self.raised.entry(me).or_insert(hand);
    }

    pub fn count(&self) -> usize {
        self.raised.len()
    }
}
