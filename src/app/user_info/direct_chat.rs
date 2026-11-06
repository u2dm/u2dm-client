use std::sync::Arc;

use super::{ChatReady, DmOutcome, OpenSheet, UserInfo, VerifyOutcome, listed};
use crate::app::event::{AppEvent, UserInfoEvent};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{DirectChat, VerifyOffer};
use crate::domain::room::{Room, RoomId};
use crate::domain::user_info::{IdentityTrust, UserId, UserProfile};
use crate::error::AppError;
use crate::ports::matrix::{UserInfoPort, VerificationPort};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AfterStart {
    Open,
    Verify,
}

pub(super) enum DmStart {
    Requested(AfterStart),
    AwaitingSync(RoomId, AfterStart),
}

impl DmStart {
    fn then(&self) -> AfterStart {
        match self {
            Self::Requested(then) | Self::AwaitingSync(_, then) => *then,
        }
    }
}

pub(super) fn direct_with<'a>(rooms: &'a [Arc<Room>], user_id: &UserId) -> Option<&'a RoomId> {
    rooms
        .iter()
        .find(|room| room.direct_partner.as_ref() == Some(user_id))
        .map(|room| &room.id)
}

impl UserInfo {
    pub(in crate::app) fn message(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        user_id: &UserId,
    ) -> Option<RoomId> {
        let open = self.open.as_ref().filter(|open| open.user_id == *user_id)?;
        let direct = self.direct_chat(open);
        let profile = open.profile()?;
        match direct {
            DirectChat::Open => profile.direct_room.clone(),
            DirectChat::Start => {
                let name = profile.shown();
                self.start_dm(port, user_id.clone(), name, AfterStart::Open);
                None
            }
            DirectChat::Hidden | DirectChat::Starting => {
                tracing::debug!(%user_id, "ignoring a message request the sheet does not offer");
                None
            }
        }
    }

    pub(in crate::app) fn verify(
        &mut self,
        users: Arc<dyn UserInfoPort>,
        verification: Arc<dyn VerificationPort>,
        user_id: &UserId,
        rooms: &[Arc<Room>],
    ) {
        let Some(open) = self.open.as_ref().filter(|open| open.user_id == *user_id) else {
            return;
        };
        if self.verify_offer(open) != VerifyOffer::Offered {
            tracing::debug!(%user_id, "ignoring a verification the sheet does not offer");
            return;
        }
        if open.chat_holds_requests(rooms) {
            self.request_verification(verification, user_id.clone());
            return;
        }
        let Some(name) = open.profile().map(UserProfile::shown) else {
            return;
        };
        tracing::debug!(%user_id, "no direct chat to send the verification request in; starting one");
        self.start_dm(users, user_id.clone(), name, AfterStart::Verify);
    }

    pub(in crate::app) fn request_verification(
        &mut self,
        port: Arc<dyn VerificationPort>,
        user_id: UserId,
    ) {
        if !self.verifying.insert(user_id.clone()) {
            tracing::debug!(%user_id, "a verification request is already on its way");
            return;
        }
        let name = self.shown_name(&user_id);
        self.clear_error(&user_id);
        self.publish();

        self.actions.spawn_reporting(&self.events, async move {
            let outcome = match port.verify_user(&user_id).await {
                    Ok(()) => VerifyOutcome::Sent,
                    Err(AppError::PartnerLeftChat) => {
                        tracing::warn!(%user_id, "the person left the direct chat; not asking them to verify");
                        VerifyOutcome::PartnerLeft
                    }
                    Err(e) => {
                        tracing::warn!(%user_id, "failed to ask the person to verify: {e}");
                        VerifyOutcome::Failed
                    }
                };
            let settled = UserInfoEvent::VerifySettled {
                user_id,
                name,
                outcome,
            };
            AppEvent::UserInfo(settled)
        });
    }

    pub(in crate::app) fn verify_settled(
        &mut self,
        user_id: &UserId,
        name: &str,
        outcome: VerifyOutcome,
    ) {
        if !self.verifying.remove(user_id) {
            tracing::debug!(%user_id, "dropping a verification request nobody is waiting for");
            return;
        }
        match outcome {
            VerifyOutcome::Sent => {}
            VerifyOutcome::PartnerLeft => {
                self.report(
                    user_id,
                    UserMessage::about(UserMessageKind::PartnerLeftChat, &name),
                );
            }
            VerifyOutcome::Failed => {
                self.report(
                    user_id,
                    UserMessage::about(UserMessageKind::UserVerificationFailed, &name),
                );
            }
        }
        self.publish();
    }

    pub(in crate::app) fn dm_started(
        &mut self,
        user_id: UserId,
        name: &str,
        outcome: DmOutcome,
        rooms: &[Arc<Room>],
    ) -> Option<ChatReady> {
        let Some(start) = self.starting.remove(&user_id) else {
            tracing::debug!(%user_id, "dropping a started chat nobody is waiting for");
            return None;
        };
        let then = start.then();
        let room_id = match outcome {
            DmOutcome::Started(room_id) => room_id,
            DmOutcome::Failed => {
                self.report(
                    &user_id,
                    UserMessage::about(UserMessageKind::DirectChatFailed, &name),
                );
                self.publish();
                return None;
            }
        };
        if !self
            .open
            .as_ref()
            .is_some_and(|open| open.user_id == user_id)
        {
            tracing::debug!(%user_id, %room_id, "started a chat for a sheet that has closed");
            self.publish();
            return None;
        }
        if let Some(ready) = chat_ready(rooms, &room_id, &user_id, then) {
            return Some(ready);
        }
        tracing::debug!(%user_id, %room_id, "started a chat, waiting for the room list to show it");
        self.starting
            .insert(user_id, DmStart::AwaitingSync(room_id, then));
        self.publish();
        None
    }

    pub(in crate::app) fn follow(&mut self, rooms: &[Arc<Room>]) -> Option<ChatReady> {
        let open = self.open.as_mut()?;
        let direct_listed = open
            .direct_room()
            .is_some_and(|room_id| listed(rooms, room_id));
        let moved = open.direct_listed != direct_listed;
        open.direct_listed = direct_listed;
        let arrived = match self.starting.get(&open.user_id) {
            Some(DmStart::AwaitingSync(room_id, then)) => {
                chat_ready(rooms, room_id, &open.user_id, *then)
            }
            Some(DmStart::Requested(_)) | None => None,
        };
        if arrived.is_some() {
            let user_id = open.user_id.clone();
            self.starting.remove(&user_id);
            return arrived;
        }
        if moved {
            self.publish();
        }
        None
    }

    pub(super) fn direct_chat(&self, open: &OpenSheet) -> DirectChat {
        let Some(profile) = open.profile() else {
            return DirectChat::Hidden;
        };
        if profile.is_self || profile.direct_room.as_ref() == Some(&open.room_id) {
            return DirectChat::Hidden;
        }
        if self.starting.contains_key(&open.user_id) {
            return DirectChat::Starting;
        }
        match (&profile.direct_room, open.direct_listed) {
            (None, _) => DirectChat::Start,
            (Some(_), true) => DirectChat::Open,
            (Some(_), false) => DirectChat::Hidden,
        }
    }

    pub(super) fn verify_offer(&self, open: &OpenSheet) -> VerifyOffer {
        let Some(profile) = open.profile() else {
            return VerifyOffer::Hidden;
        };
        if profile.is_self || !self.session_verified {
            return VerifyOffer::Hidden;
        }
        let start = self.starting.get(&open.user_id).map(DmStart::then);
        if self.verifying.contains(&open.user_id) || start == Some(AfterStart::Verify) {
            return VerifyOffer::Requesting;
        }
        if start.is_some() {
            return VerifyOffer::Hidden;
        }
        match open.identity() {
            Some(IdentityTrust::Unverified) => VerifyOffer::Offered,
            Some(
                IdentityTrust::Verified
                | IdentityTrust::NoLongerVerified
                | IdentityTrust::NoIdentity,
            )
            | None => VerifyOffer::Hidden,
        }
    }

    pub(super) fn forget_awaited_chats(&mut self) {
        self.starting
            .retain(|_, start| matches!(start, DmStart::Requested(_)));
    }

    fn start_dm(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        user_id: UserId,
        name: String,
        then: AfterStart,
    ) {
        self.starting
            .insert(user_id.clone(), DmStart::Requested(then));
        self.clear_error(&user_id);
        self.publish();
        self.actions.spawn_reporting(&self.events, async move {
            let outcome = match port.start_dm(&user_id).await {
                Ok(room_id) => DmOutcome::Started(room_id),
                Err(e) => {
                    tracing::warn!(%user_id, "failed to start a chat: {e}");
                    DmOutcome::Failed
                }
            };
            let started = UserInfoEvent::DmStarted {
                user_id,
                name,
                outcome,
            };
            AppEvent::UserInfo(started)
        });
    }
}

fn chat_ready(
    rooms: &[Arc<Room>],
    room_id: &RoomId,
    user_id: &UserId,
    then: AfterStart,
) -> Option<ChatReady> {
    match then {
        AfterStart::Open => listed(rooms, room_id).then(|| ChatReady::Open(room_id.clone())),
        AfterStart::Verify => rooms
            .iter()
            .any(|room| room.id == *room_id && room.direct_partner.as_ref() == Some(user_id))
            .then(|| ChatReady::Verify(user_id.clone())),
    }
}
