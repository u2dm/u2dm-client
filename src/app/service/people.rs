use std::sync::Arc;

use super::AppService;
use crate::app::event::{InvitePeopleEvent, PresenceEvent, UserInfoEvent};
use crate::app::user_info::ChatReady;
use crate::commands::ui::WindowAttention;
use crate::domain::room::RoomId;
use crate::domain::user_info::{IgnoreChange, Moderation, UserId};

impl AppService {
    pub(super) fn open_invite_people(&mut self, room_id: &RoomId) {
        let target = self.room_directory.invite_target(room_id);
        self.invite_people.open(target);
    }

    pub(super) fn search_people(&mut self, term: &str) {
        if let Some(port) = self.port(|a| &a.invite_people) {
            self.invite_people.search(port, term);
        }
    }

    pub(super) fn retry_people_search(&mut self) {
        if let Some(port) = self.port(|a| &a.invite_people) {
            self.invite_people.retry(port);
        }
    }

    pub(super) fn send_invites(&mut self, reason: &str) {
        if let Some(port) = self.port(|a| &a.invite_people) {
            self.invite_people.send(port, reason);
        }
    }

    pub(super) fn follow_invite_people(&mut self) {
        let membership = self.room_directory.membership(&self.selection);
        self.invite_people.follow(&membership);
    }

    pub(super) fn handle_invite_people(&mut self, event: InvitePeopleEvent) {
        let Some(port) = self.port(|a| &a.invite_people) else {
            return;
        };
        match event {
            InvitePeopleEvent::Found {
                generation,
                outcome,
            } => self.invite_people.found(port, generation, outcome),
            InvitePeopleEvent::AvatarsReady { generation, ready } => {
                self.invite_people.avatars_landed(generation, ready);
            }
            InvitePeopleEvent::Sent { batch, outcomes } => {
                if let Some(room_id) = self.invite_people.sent(batch, outcomes)
                    && let Some(room_info) = self.port(|a| &a.room_info)
                {
                    self.room_info.members_changed(room_info, &room_id);
                }
            }
        }
    }

    pub(super) fn open_user_info(&mut self, user_id: UserId) {
        if let Some(port) = self.port(|a| &a.user_info) {
            self.user_info
                .open(port, self.selection.room.clone(), user_id);
        }
        self.follow_presence();
    }

    pub(super) fn retry_user_info(&mut self) {
        if let Some(port) = self.port(|a| &a.user_info) {
            self.user_info.retry(port);
        }
    }

    pub(super) fn set_ignored(&mut self, user_id: &UserId, change: IgnoreChange) {
        if let Some(port) = self.port(|a| &a.user_info) {
            self.user_info.set_ignored(port, user_id, change);
        }
    }

    pub(super) fn moderate(
        &mut self,
        user_id: &UserId,
        action: Moderation,
        reason: Option<String>,
    ) {
        if let Some(port) = self.port(|a| &a.user_info) {
            self.user_info.moderate(port, user_id, action, reason);
        }
    }

    pub(super) async fn message_user(&mut self, user_id: &UserId) {
        let Some(port) = self.port(|a| &a.user_info) else {
            return;
        };
        if let Some(room_id) = self.user_info.message(port, user_id) {
            self.select_room(room_id).await;
        }
    }

    pub(super) fn verify_user(&mut self, user_id: &UserId) {
        let (Some(users), Some(verification)) =
            (self.port(|a| &a.user_info), self.port(|a| &a.verification))
        else {
            return;
        };
        self.user_info
            .verify(users, verification, user_id, self.room_directory.rooms());
    }

    pub(super) async fn direct_chat_ready(&mut self, ready: ChatReady) {
        match ready {
            ChatReady::Open(room_id) => self.select_room(room_id).await,
            ChatReady::Verify(user_id) => {
                if let Some(port) = self.port(|a| &a.verification) {
                    self.user_info.request_verification(port, user_id);
                }
            }
        }
    }

    pub(super) async fn handle_user_info(&mut self, event: UserInfoEvent) {
        let Some(port) = self.port(|a| &a.user_info) else {
            return;
        };
        match event {
            UserInfoEvent::Read {
                generation,
                outcome,
            } => {
                self.user_info
                    .read_landed(port, generation, outcome, self.room_directory.rooms());
            }
            UserInfoEvent::PronounsFetched {
                generation,
                pronouns,
            } => {
                self.user_info.pronouns_landed(generation, pronouns);
            }
            UserInfoEvent::ProfileFetched {
                generation,
                profile,
            } => {
                self.user_info.profile_landed(port, generation, profile);
            }
            UserInfoEvent::AvatarReady { generation } => {
                self.user_info.avatar_ready(generation);
            }
            UserInfoEvent::DmStarted {
                user_id,
                name,
                outcome,
            } => {
                let ready =
                    self.user_info
                        .dm_started(user_id, &name, outcome, self.room_directory.rooms());
                if let Some(ready) = ready {
                    self.direct_chat_ready(ready).await;
                }
            }
            UserInfoEvent::TrustChanged { generation, trust } => {
                self.user_info.trust_changed(generation, trust);
            }
            UserInfoEvent::VerifySettled {
                user_id,
                name,
                outcome,
            } => {
                self.user_info.verify_settled(&user_id, &name, outcome);
            }
            UserInfoEvent::IgnoreSettled {
                user_id,
                name,
                change,
                outcome,
            } => {
                self.user_info
                    .ignore_settled(&user_id, &name, change, outcome);
            }
            UserInfoEvent::ModerationSettled {
                room_id,
                user_id,
                name,
                action,
                outcome,
            } => {
                self.user_info
                    .moderation_settled((room_id, user_id), &name, action, outcome);
            }
        }
    }

    pub(super) fn attention_changed(&mut self, attention: WindowAttention) {
        self.notifications.focus_changed(attention.focus);
        self.calls
            .focus_changed(attention.focus, &self.room_directory);
        let port = self.port(|a| &a.presence);
        self.presence.attention_changed(port, attention);
    }

    pub(super) fn follow_presence(&mut self) {
        let partner = self.selection.room.as_ref().and_then(|room_id| {
            let partner = self.room_directory.room(room_id)?.direct_partner.clone()?;
            Some((room_id.clone(), partner))
        });
        let person = self.user_info.person().cloned();
        let port = self.port(|a| &a.presence);
        self.presence.follow(port, partner, person);
    }

    pub(super) fn close_user_info(&mut self) {
        self.user_info.close();
        self.follow_presence();
    }

    pub(super) fn handle_presence(&mut self, event: PresenceEvent) {
        match event {
            PresenceEvent::Disabled { session } => {
                if self.lifecycle.runs(session) {
                    self.presence.disabled();
                }
            }
            PresenceEvent::Looked {
                watch,
                user_id,
                result,
            } => {
                if self.held_session.running().is_some() {
                    self.presence.looked(watch, user_id, result);
                }
            }
        }
    }

    pub(super) fn begin_presence(&mut self, session: u64) {
        let Some((port, own)) = self.held_session.running().map(|a| {
            (
                Arc::clone(&a.presence),
                UserId::new(a.session.user_id.clone()),
            )
        }) else {
            return;
        };
        self.presence.begin_session(port, session, own);
    }
}
