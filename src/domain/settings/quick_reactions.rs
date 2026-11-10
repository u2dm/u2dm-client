use std::fmt;

use super::replaced;

pub const QUICK_REACTION_COUNT: usize = 8;

pub const DEFAULT_QUICK_REACTIONS: [&str; QUICK_REACTION_COUNT] =
    ["❤️", "😕", "🎉", "😭", "😄", "👎", "👍", "🫃"];

pub const DEFAULT_ONE_CLICK: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactionSlot(usize);

impl ReactionSlot {
    pub const fn new(index: usize) -> Option<Self> {
        if index < QUICK_REACTION_COUNT {
            Some(Self(index))
        } else {
            None
        }
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct QuickReaction(String);

impl QuickReaction {
    pub fn parse(text: &str) -> Option<Self> {
        emojis::get(text.trim()).map(|emoji| Self(emoji.as_str().to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for QuickReaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("QuickReaction(..)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickReactionChange {
    Place(ReactionSlot, QuickReaction),
    OneClick(ReactionSlot),
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickReactions {
    keys: [QuickReaction; QUICK_REACTION_COUNT],
    one_click: ReactionSlot,
}

impl Default for QuickReactions {
    fn default() -> Self {
        Self {
            keys: default_keys(),
            one_click: ReactionSlot(DEFAULT_ONE_CLICK),
        }
    }
}

impl QuickReactions {
    pub fn from_slots(
        read: [Option<QuickReaction>; QUICK_REACTION_COUNT],
        one_click: ReactionSlot,
    ) -> Self {
        let mut held = Vec::with_capacity(QUICK_REACTION_COUNT);
        let unique = read.map(|key| key.filter(|key| hold_first(&mut held, key)));
        let mut keys = Vec::with_capacity(QUICK_REACTION_COUNT);
        for (key, own) in unique.into_iter().zip(default_keys()) {
            let key = key.unwrap_or_else(|| {
                let free = free_default(&held, own);
                held.push(free.clone());
                free
            });
            keys.push(key);
        }
        <[QuickReaction; QUICK_REACTION_COUNT]>::try_from(keys)
            .map_or_else(|_| Self::default(), |keys| Self { keys, one_click })
    }

    pub const fn keys(&self) -> &[QuickReaction; QUICK_REACTION_COUNT] {
        &self.keys
    }

    pub const fn one_click(&self) -> ReactionSlot {
        self.one_click
    }

    pub fn is_default(&self) -> bool {
        self.one_click.0 == DEFAULT_ONE_CLICK
            && self
                .keys
                .iter()
                .map(QuickReaction::as_str)
                .eq(DEFAULT_QUICK_REACTIONS)
    }

    pub fn apply(&mut self, change: QuickReactionChange) -> bool {
        match change {
            QuickReactionChange::Place(slot, key) => self.place(slot, key),
            QuickReactionChange::OneClick(slot) => replaced(&mut self.one_click, slot),
            QuickReactionChange::Reset => replaced(self, Self::default()),
        }
    }

    fn place(&mut self, slot: ReactionSlot, key: QuickReaction) -> bool {
        match self.keys.iter().position(|held| *held == key) {
            Some(held) if held == slot.0 => false,
            Some(held) => {
                self.keys.swap(held, slot.0);
                true
            }
            None => match self.keys.get_mut(slot.0) {
                Some(cell) => {
                    *cell = key;
                    true
                }
                None => false,
            },
        }
    }
}

fn default_keys() -> [QuickReaction; QUICK_REACTION_COUNT] {
    DEFAULT_QUICK_REACTIONS.map(|key| QuickReaction(key.to_owned()))
}

fn hold_first(held: &mut Vec<QuickReaction>, key: &QuickReaction) -> bool {
    let first = !held.contains(key);
    if first {
        held.push(key.clone());
    }
    first
}

fn free_default(held: &[QuickReaction], own: QuickReaction) -> QuickReaction {
    if !held.contains(&own) {
        return own;
    }
    default_keys()
        .into_iter()
        .find(|key| !held.contains(key))
        .unwrap_or(own)
}
