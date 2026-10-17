use crate::commands::view::{JoinOffer, JoinRefusal};
use crate::domain::call::Transport;
use crate::domain::room::Room;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Media {
    NotBuilt,
    Broken,
    Ready,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) enum Transports {
    #[default]
    Unknown,
    Found(Vec<Transport>),
    Missing,
}

impl Transports {
    pub(super) fn first(&self) -> Option<&Transport> {
        match self {
            Self::Found(found) => found.first(),
            Self::Unknown | Self::Missing => None,
        }
    }
}

pub(super) struct OfferFacts<'a> {
    pub(super) media: Media,
    pub(super) transports: &'a Transports,
    pub(super) room: &'a Room,
    pub(super) verified: bool,
    pub(super) in_this_call: bool,
}

pub(super) fn offer(facts: &OfferFacts<'_>) -> JoinOffer {
    if facts.in_this_call {
        return JoinOffer::InCall;
    }
    match refusal(facts) {
        Some(refusal) => JoinOffer::Refused(refusal),
        None if facts.room.call.is_some() => JoinOffer::Join,
        None => JoinOffer::Start,
    }
}

fn refusal(facts: &OfferFacts<'_>) -> Option<JoinRefusal> {
    match facts.media {
        Media::NotBuilt => return Some(JoinRefusal::NotBuilt),
        Media::Broken => return Some(JoinRefusal::MediaBroken),
        Media::Ready => {}
    }
    if *facts.transports == Transports::Missing {
        return Some(JoinRefusal::NoServer);
    }
    if !facts.room.may_join_calls {
        return Some(JoinRefusal::NotAllowed);
    }
    (facts.room.is_encrypted && !facts.verified).then_some(JoinRefusal::SessionUnverified)
}
