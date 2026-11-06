use crate::domain::member_name::MemberName;

pub const REACTOR_AVATAR_LIMIT: usize = 3;

pub const REACTORS_NAMED: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reactor {
    pub user_id: String,
    pub name: MemberName,
    pub avatar_url: Option<String>,
}

impl Reactor {
    pub fn new(user_id: String) -> Self {
        Self {
            user_id,
            name: MemberName::Unset,
            avatar_url: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub key: String,
    pub image: Option<CustomEmoji>,
    pub senders: Vec<Reactor>,
    pub mine: bool,
    pub send: ReactionSend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomEmoji {
    Downloading,
    Downloaded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReactionSend {
    #[default]
    Sent,
    Sending,
    Failed,
}

impl Reaction {
    pub fn count(&self) -> usize {
        self.senders.len()
    }

    pub fn shows_reactors(&self) -> bool {
        self.senders.len() < REACTOR_AVATAR_LIMIT
    }

    pub fn named(&self) -> impl Iterator<Item = &Reactor> {
        self.senders.iter().take(REACTORS_NAMED)
    }

    pub fn named_mut(&mut self) -> impl Iterator<Item = &mut Reactor> {
        self.senders.iter_mut().take(REACTORS_NAMED)
    }
}
