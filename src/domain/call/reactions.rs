use std::collections::HashMap;
use std::time::Duration;

use unicode_segmentation::UnicodeSegmentation;

use super::{CallMember, MemberKey};

pub const REACTION_SHOWN: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReactionOption {
    pub name: &'static str,
    pub emoji: &'static str,
}

pub const REACTION_OPTIONS: [ReactionOption; 17] = [
    ReactionOption {
        name: "thumbsup",
        emoji: "\u{1f44d}",
    },
    ReactionOption {
        name: "party",
        emoji: "\u{1f389}",
    },
    ReactionOption {
        name: "clapping",
        emoji: "\u{1f44f}",
    },
    ReactionOption {
        name: "dog",
        emoji: "\u{1f436}",
    },
    ReactionOption {
        name: "cat",
        emoji: "\u{1f431}",
    },
    ReactionOption {
        name: "lightbulb",
        emoji: "\u{1f4a1}",
    },
    ReactionOption {
        name: "crickets",
        emoji: "\u{1f997}",
    },
    ReactionOption {
        name: "thumbsdown",
        emoji: "\u{1f44e}",
    },
    ReactionOption {
        name: "dizzy",
        emoji: "\u{1f635}\u{200d}\u{1f4ab}",
    },
    ReactionOption {
        name: "ok",
        emoji: "\u{1f44c}",
    },
    ReactionOption {
        name: "heart",
        emoji: "\u{1f970}",
    },
    ReactionOption {
        name: "laugh",
        emoji: "\u{1f604}",
    },
    ReactionOption {
        name: "deer",
        emoji: "\u{1f98c}",
    },
    ReactionOption {
        name: "rock",
        emoji: "\u{1f918}",
    },
    ReactionOption {
        name: "wave",
        emoji: "\u{1f44b}",
    },
    ReactionOption {
        name: "drum",
        emoji: "\u{1f941}",
    },
    ReactionOption {
        name: "bell",
        emoji: "\u{1f6ce}\u{fe0f}",
    },
];

pub fn reaction_named(name: &str) -> Option<ReactionOption> {
    REACTION_OPTIONS
        .iter()
        .find(|option| option.name == name)
        .copied()
}

pub fn shown_emoji(name: &str, emoji: &str) -> Option<String> {
    let first = emoji.graphemes(true).next()?;
    if first.trim().is_empty() {
        return None;
    }
    let shown = reaction_named(name).map_or(first, |option| option.emoji);
    Some(shown.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionSighting {
    pub by: String,
    pub membership_event: String,
    pub reaction: String,
    pub emoji: String,
    pub name: String,
    pub age: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub emoji: String,
    pub name: String,
}

impl Reaction {
    pub fn of(option: ReactionOption) -> Self {
        Self {
            emoji: option.emoji.to_owned(),
            name: option.name.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Showing {
    reaction: Reaction,
    event: String,
    until: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reacted {
    BeforeYouJoined,
    NotAMembership,
    NoEmoji,
    StillShowing(MemberKey),
    Again(MemberKey),
    Shown(MemberKey),
}

#[derive(Debug, Default)]
pub struct Reactions {
    showing: HashMap<MemberKey, Showing>,
}

impl Reactions {
    pub fn sighted(
        &mut self,
        sighting: ReactionSighting,
        members: &[CallMember],
        now: Duration,
    ) -> Reacted {
        if sighting.age.is_some_and(|age| age > now) {
            return Reacted::BeforeYouJoined;
        }
        let Some(member) = members.iter().find(|member| {
            member.user.user_id == sighting.by
                && member.membership_event.as_deref() == Some(sighting.membership_event.as_str())
        }) else {
            return Reacted::NotAMembership;
        };
        let Some(emoji) = shown_emoji(&sighting.name, &sighting.emoji) else {
            return Reacted::NoEmoji;
        };
        let reaction = Reaction {
            emoji,
            name: sighting.name,
        };
        self.show(MemberKey::of(member), reaction, sighting.reaction, now)
    }

    pub fn show(
        &mut self,
        member: MemberKey,
        reaction: Reaction,
        event: String,
        now: Duration,
    ) -> Reacted {
        match self.showing.get(&member) {
            Some(showing) if showing.event == event => return Reacted::Again(member),
            Some(showing) if showing.until > now => return Reacted::StillShowing(member),
            _ => {}
        }
        self.showing.insert(
            member.clone(),
            Showing {
                reaction,
                event,
                until: now.saturating_add(REACTION_SHOWN),
            },
        );
        Reacted::Shown(member)
    }

    pub fn of(&self, member: &MemberKey, now: Duration) -> Option<&Reaction> {
        self.showing
            .get(member)
            .filter(|showing| showing.until > now)
            .map(|showing| &showing.reaction)
    }

    pub fn next_end(&self, now: Duration) -> Option<Duration> {
        self.showing
            .values()
            .map(|showing| showing.until)
            .filter(|until| *until > now)
            .min()
    }
}
