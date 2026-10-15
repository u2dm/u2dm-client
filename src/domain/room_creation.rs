use crate::domain::mention::{ALIAS_SIGIL, is_alias_localpart};
use crate::domain::room::RoomId;

pub const ALIAS_MAX_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RoomKind {
    #[default]
    Room,
    Space,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RoomAccess {
    #[default]
    InviteOnly,
    SpaceMembers,
    Public,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomDraft {
    pub kind: RoomKind,
    pub name: String,
    pub topic: String,
    pub access: RoomAccess,
    pub address: String,
    pub encrypted: bool,
    pub in_space: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftProblem {
    NameMissing,
    AddressMissing,
    AddressUnusable,
    SpaceMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasLocalpart(String);

impl AliasLocalpart {
    pub fn parse(typed: &str, server: &str) -> Result<Self, DraftProblem> {
        let trimmed = typed.trim();
        let localpart = trimmed.strip_prefix(ALIAS_SIGIL).unwrap_or(trimmed);
        if localpart.is_empty() {
            return Err(DraftProblem::AddressMissing);
        }
        let parsed = Self(localpart.to_owned());
        if !is_alias_localpart(localpart) || parsed.alias_on(server).len() > ALIAS_MAX_BYTES {
            return Err(DraftProblem::AddressUnusable);
        }
        Ok(parsed)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn alias_on(&self, server: &str) -> String {
        format!("{ALIAS_SIGIL}{}:{server}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinAccess {
    InviteOnly,
    SpaceMembers(RoomId),
    Public(AliasLocalpart),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Room { encrypted: bool },
    Space,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomBlueprint {
    shape: Shape,
    name: String,
    topic: Option<String>,
    access: JoinAccess,
    parent: Option<RoomId>,
}

impl RoomBlueprint {
    pub fn new(
        draft: &RoomDraft,
        offered: Option<&RoomId>,
        server: &str,
    ) -> Result<Self, DraftProblem> {
        let name = draft.name.trim();
        if name.is_empty() {
            return Err(DraftProblem::NameMissing);
        }
        let access = match draft.access {
            RoomAccess::InviteOnly => JoinAccess::InviteOnly,
            RoomAccess::SpaceMembers => {
                JoinAccess::SpaceMembers(offered.cloned().ok_or(DraftProblem::SpaceMissing)?)
            }
            RoomAccess::Public => {
                JoinAccess::Public(AliasLocalpart::parse(&draft.address, server)?)
            }
        };
        let shape = match draft.kind {
            RoomKind::Room => Shape::Room {
                encrypted: draft.encrypted && !matches!(access, JoinAccess::Public(_)),
            },
            RoomKind::Space => Shape::Space,
        };
        let topic = draft.topic.trim();
        Ok(Self {
            shape,
            name: name.to_owned(),
            topic: (!topic.is_empty()).then(|| topic.to_owned()),
            access,
            parent: offered.filter(|_| draft.in_space).cloned(),
        })
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub fn kind(&self) -> RoomKind {
        match self.shape {
            Shape::Room { .. } => RoomKind::Room,
            Shape::Space => RoomKind::Space,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn topic(&self) -> Option<&str> {
        self.topic.as_deref()
    }

    pub fn access(&self) -> &JoinAccess {
        &self.access
    }

    pub fn parent(&self) -> Option<&RoomId> {
        self.parent.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildLink {
    NotAsked,
    Added,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedRoom {
    pub room_id: RoomId,
    pub child: ChildLink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateFailure {
    AddressTaken,
    Refused,
    Unsupported,
    RateLimited,
    Failed,
}

pub type CreateResult = Result<CreatedRoom, CreateFailure>;
