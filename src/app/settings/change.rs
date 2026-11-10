use crate::app::event::ActionOutcome;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Change<T> {
    Requested { request: u64, target: T },
    AwaitingEcho { target: T },
}

impl<T: Copy + PartialEq> Change<T> {
    pub(super) fn target(self) -> T {
        match self {
            Self::Requested { target, .. } | Self::AwaitingEcho { target } => target,
        }
    }

    pub(super) fn in_flight(self) -> bool {
        matches!(self, Self::Requested { .. })
    }
}

pub(super) fn is_wanted<T: Copy + PartialEq>(
    change: Option<Change<T>>,
    listed: T,
    wanted: T,
) -> bool {
    !change.is_some_and(Change::in_flight) && change.map_or(listed, Change::target) != wanted
}

pub(super) fn after_write<T: Copy + PartialEq>(
    change: Option<Change<T>>,
    request: u64,
    outcome: ActionOutcome,
    listed: Option<T>,
) -> Option<Change<T>> {
    let Some(Change::Requested {
        request: pending,
        target,
    }) = change
    else {
        return change;
    };
    if pending != request {
        return change;
    }
    (outcome == ActionOutcome::Done && listed != Some(target))
        .then_some(Change::AwaitingEcho { target })
}

pub(super) fn after_echo<T: Copy + PartialEq>(
    change: Option<Change<T>>,
    listed: Option<T>,
) -> Option<Change<T>> {
    change.filter(|change| change.in_flight() || listed != Some(change.target()))
}
