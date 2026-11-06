use super::ActionOutcome;
use crate::app::room_info::RosterOutcome;
use crate::app::room_preview::JoinedRoom;
use crate::app::space_index::{JoinOutcome, PageOutcome};
use crate::app::space_leave::SpaceLeaveOutcome;
use crate::domain::departure::AdminStanding;
use crate::domain::image_pack::PackId;
use crate::domain::matrix_link::RoomAddress;
use crate::domain::room::RoomId;
use crate::domain::room_creation::CreateResult;
use crate::domain::room_info::RoomAbout;
use crate::domain::room_preview::{PreviewFailure, RoomPreview};
use crate::ports::matrix::PackCatalog;

pub(in crate::app) enum PreviewEvent {
    Landed {
        generation: u64,
        outcome: Result<Box<RoomPreview>, PreviewFailure>,
    },
    AvatarsReady {
        generation: u64,
        ready: usize,
    },
    JoinSettled {
        room: RoomAddress,
        name: String,
        outcome: JoinedRoom,
    },
}

impl PreviewEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::Landed { .. } => "RoomPreviewLanded",
            Self::AvatarsReady { .. } => "RoomPreviewAvatarsReady",
            Self::JoinSettled { .. } => "RoomLinkJoinSettled",
        }
    }
}

pub(in crate::app) struct CreationSettled {
    pub(in crate::app) creation: u64,
    pub(in crate::app) outcome: CreateResult,
}

pub(in crate::app) enum PacksEvent {
    CatalogLoaded {
        generation: i32,
        catalog: PackCatalog,
    },
    ImagesReady {
        generation: i32,
        ready: usize,
    },
    GlobalFailed {
        op: u64,
        pack: PackId,
    },
}

impl PacksEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::CatalogLoaded { .. } => "PackCatalogLoaded",
            Self::ImagesReady { .. } => "PackImagesReady",
            Self::GlobalFailed { .. } => "PackGlobalFailed",
        }
    }
}

pub(in crate::app) enum DepartureEvent {
    RemovalForgotten {
        room_id: RoomId,
        name: String,
        outcome: ActionOutcome,
    },
    StandingRead {
        generation: u64,
        standing: Option<AdminStanding>,
    },
    SpaceLeft {
        space_id: RoomId,
        name: String,
        outcome: SpaceLeaveOutcome,
    },
}

impl DepartureEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::RemovalForgotten { .. } => "RemovalForgotten",
            Self::StandingRead { .. } => "SpaceAdminStandingRead",
            Self::SpaceLeft { .. } => "SpaceLeft",
        }
    }
}

pub(in crate::app) enum SpaceIndexEvent {
    Paged {
        generation: u64,
        outcome: PageOutcome,
    },
    AvatarsReady {
        generation: u64,
        ready: usize,
    },
    ChildJoinSettled {
        room_id: RoomId,
        name: String,
        outcome: JoinOutcome,
    },
}

impl SpaceIndexEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::Paged { .. } => "SpaceIndexPaged",
            Self::AvatarsReady { .. } => "SpaceIndexAvatarsReady",
            Self::ChildJoinSettled { .. } => "SpaceChildJoinSettled",
        }
    }
}

pub(in crate::app) enum RoomInfoEvent {
    RosterFetched {
        generation: u64,
        outcome: RosterOutcome,
    },
    AboutFetched {
        generation: u64,
        about: Option<RoomAbout>,
    },
    AvatarsReady {
        generation: u64,
        ready: usize,
    },
}

impl RoomInfoEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::RosterFetched { .. } => "RoomRosterFetched",
            Self::AboutFetched { .. } => "RoomAboutFetched",
            Self::AvatarsReady { .. } => "RoomInfoAvatarsReady",
        }
    }
}

pub(in crate::app) enum RoomActionEvent {
    NotifySettled {
        request: u64,
        room_id: RoomId,
        name: String,
        outcome: ActionOutcome,
    },
    LeaveSettled {
        room_id: RoomId,
        name: String,
        outcome: ActionOutcome,
    },
    ReadSettled {
        room_id: RoomId,
        name: String,
        outcome: ActionOutcome,
    },
    LinkResolved {
        request: i32,
        name: String,
        link: Option<String>,
    },
}

impl RoomActionEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::NotifySettled { .. } => "RoomNotifySettled",
            Self::LeaveSettled { .. } => "RoomLeaveSettled",
            Self::ReadSettled { .. } => "RoomReadSettled",
            Self::LinkResolved { .. } => "RoomLinkResolved",
        }
    }
}

pub(in crate::app) enum MessageActionEvent {
    LinkResolved {
        request: i32,
        link: Option<String>,
    },
    PinSettled {
        request: u64,
        event_id: String,
        outcome: ActionOutcome,
    },
    DeletionSettled {
        event_id: String,
        outcome: ActionOutcome,
    },
}

impl MessageActionEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::LinkResolved { .. } => "MessageLinkResolved",
            Self::PinSettled { .. } => "PinChangeSettled",
            Self::DeletionSettled { .. } => "DeletionSettled",
        }
    }
}
