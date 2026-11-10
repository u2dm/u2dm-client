use std::collections::HashMap;
use std::sync::Arc;

use super::{OpenSheet, Seat, UserInfo};
use crate::app::event::{ActionOutcome, AppEvent, UserInfoEvent};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::PendingModeration;
use crate::domain::room::RoomId;
use crate::domain::user_info::{IgnoreChange, Moderation, RoomMembership, UserId, UserProfile};
use crate::ports::matrix::UserInfoPort;

impl UserInfo {
    pub(in crate::app) fn set_ignored(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        user_id: &UserId,
        change: IgnoreChange,
    ) {
        let Some(profile) = self
            .open
            .as_ref()
            .filter(|open| open.user_id == *user_id)
            .and_then(OpenSheet::profile)
        else {
            return;
        };
        if profile.is_self
            || profile.ignored == change.ignores()
            || self.ignoring.contains_key(user_id)
        {
            tracing::debug!(%user_id, ?change, "ignoring an ignore change that has nothing to do");
            return;
        }
        let name = profile.shown();
        self.ignoring.insert(user_id.clone(), change);
        self.clear_error(user_id);
        self.publish();

        let user_id = user_id.clone();
        self.actions.spawn_reporting(&self.events, async move {
            let outcome = ActionOutcome::of(
                port.set_ignored(&user_id, change).await,
                |e| tracing::warn!(%user_id, ?change, "failed to change the ignore list: {e}"),
            );
            let settled = UserInfoEvent::IgnoreSettled {
                user_id,
                name,
                change,
                outcome,
            };
            AppEvent::UserInfo(settled)
        });
    }

    pub(in crate::app) fn ignore_settled(
        &mut self,
        user_id: &UserId,
        name: &str,
        change: IgnoreChange,
        outcome: ActionOutcome,
    ) {
        if self.ignoring.remove(user_id).is_none() {
            tracing::debug!(%user_id, "dropping an ignore change nobody is waiting for");
            return;
        }
        match outcome {
            ActionOutcome::Done => {
                self.accepted_ignores
                    .insert(user_id.clone(), change.ignores());
                if let Some(profile) = self
                    .open
                    .as_mut()
                    .filter(|open| open.user_id == *user_id)
                    .and_then(OpenSheet::loaded_mut)
                {
                    profile.ignored = change.ignores();
                }
            }
            ActionOutcome::Failed => {
                let kind = match change {
                    IgnoreChange::Ignore => UserMessageKind::IgnoreFailed,
                    IgnoreChange::Unignore => UserMessageKind::UnignoreFailed,
                };
                self.report(user_id, UserMessage::about(kind, &name));
            }
        }
        self.publish();
    }

    pub(in crate::app) fn ignore_changed_elsewhere(
        &mut self,
        user_id: &UserId,
        change: IgnoreChange,
    ) {
        self.accepted_ignores
            .insert(user_id.clone(), change.ignores());
        if let Some(profile) = self
            .open
            .as_mut()
            .filter(|open| open.user_id == *user_id)
            .and_then(OpenSheet::loaded_mut)
        {
            profile.ignored = change.ignores();
            self.publish();
        }
    }

    pub(in crate::app) fn moderate(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        user_id: &UserId,
        action: Moderation,
        reason: Option<String>,
    ) {
        let Some(open) = self.open.as_ref().filter(|open| open.user_id == *user_id) else {
            return;
        };
        let Some(profile) = open.profile() else {
            return;
        };
        let seat = (open.room_id.clone(), user_id.clone());
        if !profile.offers(action) || self.moderating.contains_key(&seat) {
            tracing::debug!(%user_id, ?action, "ignoring a moderation the sheet does not offer");
            return;
        }
        let name = profile.shown();
        self.moderating.insert(seat.clone(), action);
        self.clear_error(user_id);
        self.publish();

        self.actions.spawn_reporting(&self.events, async move {
            let (room_id, user_id) = seat;
            let outcome = ActionOutcome::of(
                port.moderate(&room_id, &user_id, action, reason.as_deref())
                    .await,
                |e| tracing::warn!(%user_id, room = %room_id, ?action, "failed to moderate: {e}"),
            );
            let settled = UserInfoEvent::ModerationSettled {
                room_id,
                user_id,
                name,
                action,
                outcome,
            };
            AppEvent::UserInfo(settled)
        });
    }

    pub(in crate::app) fn moderation_settled(
        &mut self,
        seat: Seat,
        name: &str,
        action: Moderation,
        outcome: ActionOutcome,
    ) {
        if self.moderating.remove(&seat).is_none() {
            tracing::debug!(user_id = %seat.1, "dropping a moderation nobody is waiting for");
            return;
        }
        match outcome {
            ActionOutcome::Done => {
                let (room_id, user_id) = &seat;
                if let Some(profile) = self
                    .open
                    .as_mut()
                    .filter(|open| open.shows(room_id, user_id))
                    .and_then(OpenSheet::loaded_mut)
                {
                    profile.membership = action.leaves();
                }
                self.accepted_moderation.insert(seat, action.leaves());
            }
            ActionOutcome::Failed => {
                let kind = match action {
                    Moderation::Kick => UserMessageKind::KickFailed,
                    Moderation::Ban => UserMessageKind::BanFailed,
                    Moderation::Unban => UserMessageKind::UnbanFailed,
                };
                self.report(&seat.1, UserMessage::about(kind, &name));
            }
        }
        self.publish();
    }
}

pub(super) fn settle_accepted_membership(
    accepted: &mut HashMap<Seat, RoomMembership>,
    room_id: &RoomId,
    profile: &mut UserProfile,
) {
    let seat = (room_id.clone(), profile.user_id.clone());
    let Some(&membership) = accepted.get(&seat) else {
        return;
    };
    if profile.membership == membership {
        accepted.remove(&seat);
    } else {
        profile.membership = membership;
    }
}

pub(super) fn pending_moderation(action: Option<&Moderation>) -> PendingModeration {
    match action {
        None => PendingModeration::None,
        Some(Moderation::Kick) => PendingModeration::Kick,
        Some(Moderation::Ban) => PendingModeration::Ban,
        Some(Moderation::Unban) => PendingModeration::Unban,
    }
}

pub(super) fn settle_accepted_ignore(
    accepted: &mut HashMap<UserId, bool>,
    profile: &mut UserProfile,
) {
    let Some(&ignored) = accepted.get(&profile.user_id) else {
        return;
    };
    if profile.ignored == ignored {
        accepted.remove(&profile.user_id);
    } else {
        profile.ignored = ignored;
    }
}
