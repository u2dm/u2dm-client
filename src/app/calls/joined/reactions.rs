use std::time::Duration;

use super::{Effect, Joined};
use crate::commands::view::{ReactionNews, ReactionView};
use crate::domain::call::MemberKey;
use crate::domain::call::reactions::{
    Reacted, Reaction, ReactionOption, ReactionSighting, reaction_named,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReactionRequest {
    Idle,
    Sending { membership_event: String },
    Failed,
}

impl Joined {
    fn own_reaction_shows(&self) -> bool {
        self.reactions.of(&self.me, self.now()).is_some()
    }

    pub(super) fn reaction_view(&self) -> ReactionView {
        match &self.reaction_request {
            ReactionRequest::Sending { .. } => ReactionView::Sending,
            ReactionRequest::Failed => ReactionView::Failed,
            ReactionRequest::Idle if self.own_membership_event().is_none() => {
                ReactionView::Unavailable
            }
            ReactionRequest::Idle if self.own_reaction_shows() => ReactionView::Showing,
            ReactionRequest::Idle => ReactionView::Ready,
        }
    }

    pub(super) fn reaction_news(&self) -> Option<ReactionNews> {
        self.reaction_news.clone()
    }

    pub(super) fn reaction_on(&self, member: &MemberKey) -> Option<Reaction> {
        self.reactions.of(member, self.now()).cloned()
    }

    pub(in crate::app::calls) fn settle_reactions(&self) -> Option<Duration> {
        self.reactions.next_end(self.now())
    }

    fn reaction_to_send(&self, name: &str) -> Result<(String, ReactionOption), &'static str> {
        if matches!(self.reaction_request, ReactionRequest::Sending { .. }) {
            return Err("your last one is on its way");
        }
        if self.own_reaction_shows() {
            return Err("your last one still shows");
        }
        let reaction = reaction_named(name).ok_or("no call reaction has that name")?;
        let membership_event = self
            .own_membership_event()
            .ok_or("your call membership has not arrived yet")?;
        Ok((membership_event, reaction))
    }

    pub(in crate::app::calls) fn send_reaction(&mut self, name: &str) -> Vec<Effect> {
        let (membership_event, reaction) = match self.reaction_to_send(name) {
            Ok(sendable) => sendable,
            Err(reason) => {
                tracing::debug!(call = self.call, name, reason, "no reaction is sent");
                return Vec::new();
            }
        };
        tracing::info!(
            call = self.call,
            name = reaction.name,
            "sending your reaction"
        );
        self.reaction_request = ReactionRequest::Sending {
            membership_event: membership_event.clone(),
        };
        vec![Effect::SendReaction {
            membership_event,
            reaction,
        }]
    }

    pub(in crate::app::calls) fn reaction_sent(
        &mut self,
        membership_event: &str,
        reaction: ReactionOption,
        outcome: Result<String, String>,
    ) {
        let asked = matches!(&self.reaction_request, ReactionRequest::Sending { membership_event: asked } if asked == membership_event);
        if asked {
            self.reaction_request = if outcome.is_ok() {
                ReactionRequest::Idle
            } else {
                ReactionRequest::Failed
            };
        }
        match outcome {
            Ok(event) => self.show_sent(membership_event, reaction, event),
            Err(why) => tracing::warn!(call = self.call, "your reaction was not sent: {why}"),
        }
    }

    fn show_sent(&mut self, membership_event: &str, reaction: ReactionOption, event: String) {
        if self.own_membership_event().as_deref() != Some(membership_event) {
            tracing::info!(
                call = self.call,
                "your call membership changed while your reaction was sent, so it shows nowhere"
            );
            return;
        }
        let now = self.now();
        if let Reacted::Shown(_) =
            self.reactions
                .show(self.me.clone(), Reaction::of(reaction), event, now)
        {
            tracing::info!(call = self.call, "your reaction shows");
        }
    }

    pub(in crate::app::calls) fn reaction_sighted(&mut self, sighting: ReactionSighting) {
        let by = sighting.by.clone();
        let now = self.now();
        match self.reactions.sighted(sighting, &self.members, now) {
            Reacted::Shown(member) if member != self.me => {
                tracing::info!(call = self.call, %member, "a call participant reacted");
                self.tell_reaction(&member);
            }
            quiet => {
                tracing::debug!(
                    call = self.call,
                    sender = by,
                    reason = no_news(&quiet),
                    "a call reaction is no news"
                );
            }
        }
    }

    fn tell_reaction(&mut self, member: &MemberKey) {
        let Some(who) = self
            .members
            .iter()
            .find(|candidate| MemberKey::of(candidate) == *member)
            .map(|candidate| candidate.user.clone())
        else {
            return;
        };
        let Some(reaction) = self.reaction_on(member) else {
            return;
        };
        let serial = self
            .reaction_news
            .as_ref()
            .map_or(0, |news| news.serial)
            .wrapping_add(1);
        self.reaction_news = Some(ReactionNews {
            serial,
            who,
            reaction,
        });
    }
}

const fn no_news(reacted: &Reacted) -> &'static str {
    match reacted {
        Reacted::BeforeYouJoined => "it was sent before you joined",
        Reacted::NotAMembership => "it names no current call membership of its sender",
        Reacted::NoEmoji => "it carries no emoji",
        Reacted::StillShowing(_) => "their last reaction still shows",
        Reacted::Again(_) => "it already showed",
        Reacted::Shown(_) => "it is yours",
    }
}
