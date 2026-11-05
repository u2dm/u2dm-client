use async_trait::async_trait;
use tokio::sync::mpsc;

use super::{DemoAuthed, current_lists, unavailable};
use crate::adapters::demo::{
    data, invite_people, media, message_menu, room_info, typing, user_info,
};
use crate::domain::invite_people::{
    InviteFailure, InviteResult, PEOPLE_SEARCH_LIMIT, PeopleFound, PeopleSearchFailure, Person,
    SearchResult,
};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::mention::is_user_tag;
use crate::domain::presence::{Availability, BeaconResult, LookupResult};
use crate::domain::room::{NotifyMode, RoomId};
use crate::domain::room_info::{
    MemberRole, Reader, RoomAbout, RosterMember, RosterSection, sort_roster,
};
use crate::domain::user_info::{
    GlobalProfile, IdentityTrust, IgnoreChange, Moderation, RoomMembership, UserId, UserProfile,
    UserTrust, localpart,
};
use crate::error::Result;
use crate::ports::matrix::{
    InvitePeoplePort, PresencePort, RoomInfoPort, TypingPort, UserInfoPort,
};

impl DemoAuthed {
    fn is_ignored(&self, user_id: &str) -> bool {
        self.ignored
            .lock()
            .is_ok_and(|ignored| ignored.contains(user_id))
    }

    pub(super) fn trust_of(&self, user_id: &str) -> IdentityTrust {
        if !user_info::has_identity(user_id) {
            return IdentityTrust::NoIdentity;
        }
        if self.verified_people.contains(user_id) {
            return IdentityTrust::Verified;
        }
        if let Some(trust) = self.identity.trust_of(user_id) {
            return trust;
        }
        if user_info::verified_from_the_start(user_id) {
            IdentityTrust::Verified
        } else {
            IdentityTrust::Unverified
        }
    }

    fn user_trust(&self, user_id: &str) -> UserTrust {
        let identity = self.trust_of(user_id);
        UserTrust::new(identity, user_info::devices(user_id, identity))
    }
}

impl DemoAuthed {
    fn person(&self, room_id: &RoomId, user_id: &UserId) -> UserProfile {
        let mut profile = data::profile(room_id, user_id, user_info::avatar);
        profile.direct_room = self.chat_with(user_id);
        profile.ignored = self.is_ignored(user_id);
        user_info::shape(&mut profile);
        if !profile.is_self {
            profile.trust = self.trust_of(user_id);
        }
        if let Some(membership) = self.membership_override(room_id, user_id) {
            profile.membership = membership;
        }
        profile.powers = user_info::powers(data::role_of(room_id, data::own_user()), &profile);
        profile
    }

    fn membership_override(&self, room_id: &RoomId, user_id: &str) -> Option<RoomMembership> {
        let memberships = self.memberships.lock().ok()?;
        memberships
            .get(&(room_id.clone(), user_id.to_owned()))
            .copied()
    }

    pub(super) fn chat_with(&self, user_id: &str) -> Option<RoomId> {
        data::direct_room_with(user_id).or_else(|| {
            current_lists(&self.lists)
                .overrides
                .started_chats
                .iter()
                .find(|(_, person)| person.as_str() == user_id)
                .map(|(room_id, _)| RoomId::new(room_id))
        })
    }

    fn created(&self, room_id: &RoomId) -> Option<data::Created> {
        current_lists(&self.lists)
            .overrides
            .created
            .into_iter()
            .find(|created| created.id == room_id.as_ref())
    }

    fn invited_in(&self, room_id: &RoomId) -> Vec<String> {
        let Ok(memberships) = self.memberships.lock() else {
            return Vec::new();
        };
        let mut invited: Vec<String> = memberships
            .iter()
            .filter(|((room, _), membership)| {
                room == room_id && **membership == RoomMembership::Invited
            })
            .map(|((_, user_id), _)| user_id.clone())
            .collect();
        invited.sort();
        invited
    }

    fn session_roster(&self, room_id: &RoomId) -> Vec<RosterMember> {
        let mut roster = data::roster(room_id, room_info::member_avatar);
        if roster.is_empty() && self.created(room_id).is_some() {
            roster.push(data::named_member(
                data::own_user(),
                MemberRole::Admin,
                RosterSection::Joined,
                room_info::member_avatar,
            ));
        }
        for user_id in self.invited_in(room_id) {
            roster.retain(|member| member.user_id != user_id);
            roster.push(data::named_member(
                &user_id,
                MemberRole::Member,
                RosterSection::Invited,
                room_info::member_avatar,
            ));
        }
        sort_roster(&mut roster);
        roster
    }

    fn membership_in(&self, room_id: &RoomId, user_id: &str) -> RoomMembership {
        if let Some(membership) = self.membership_override(room_id, user_id) {
            return membership;
        }
        match self
            .session_roster(room_id)
            .into_iter()
            .find(|member| member.user_id == user_id)
            .map(|member| member.section)
        {
            Some(RosterSection::Joined) => RoomMembership::Joined,
            Some(RosterSection::Invited) => RoomMembership::Invited,
            None => RoomMembership::Outside,
        }
    }

    fn directory_person(&self, room_id: &RoomId, user_id: &str, name: Option<&str>) -> Person {
        Person {
            user_id: UserId::new(user_id),
            name: MemberName::new(name, false),
            avatar_mxc: Some(room_info::member_avatar(user_id)),
            membership: self.membership_in(room_id, user_id),
        }
    }
}

#[async_trait]
impl InvitePeoplePort for DemoAuthed {
    async fn search_people(&self, room_id: &RoomId, term: &str) -> SearchResult {
        invite_people::pause().await;
        let term = term.trim();
        if invite_people::scenario().rate_limited {
            return Err(PeopleSearchFailure::RateLimited);
        }
        if invite_people::search_fails_now(term) {
            return Err(PeopleSearchFailure::Failed);
        }
        let own = data::own_user();
        let needle = term.to_lowercase();
        let mut matching: Vec<(String, String)> = data::directory_people()
            .into_iter()
            .filter(|(user_id, name)| {
                user_id != own
                    && (localpart(user_id).to_lowercase().contains(&needle)
                        || name.to_lowercase().contains(&needle))
            })
            .collect();
        let limit = if invite_people::scenario().limited {
            invite_people::LIMITED_RESULTS
        } else {
            usize::try_from(PEOPLE_SEARCH_LIMIT).unwrap_or(usize::MAX)
        };
        let limited = matching.len() > limit;
        matching.truncate(limit);
        let mut people: Vec<Person> = matching
            .iter()
            .map(|(user_id, name)| self.directory_person(room_id, user_id, Some(name)))
            .collect();
        if is_user_tag(term)
            && term != own
            && !people.iter().any(|person| person.user_id.as_ref() == term)
        {
            let name = data::person_name(term);
            people.insert(0, self.directory_person(room_id, term, name.as_deref()));
        }
        Ok(PeopleFound { people, limited })
    }

    async fn invite(
        &self,
        room_id: &RoomId,
        user_id: &UserId,
        reason: Option<&str>,
    ) -> InviteResult {
        invite_people::pause().await;
        let scenario = invite_people::scenario();
        if !invite_people::may_invite() {
            return Err(InviteFailure::Refused);
        }
        if scenario.invite_fails {
            return Err(InviteFailure::Failed);
        }
        if scenario.blocks_vera && user_id.as_ref() == invite_people::BLOCKED_USER {
            return Err(InviteFailure::Blocked);
        }
        if matches!(
            self.membership_in(room_id, user_id),
            RoomMembership::Joined | RoomMembership::Banned
        ) {
            return Err(InviteFailure::Refused);
        }
        tracing::info!(%room_id, %user_id, reason, "demo: inviting someone");
        if let Ok(mut memberships) = self.memberships.lock() {
            memberships.insert(
                (room_id.clone(), user_id.to_string()),
                RoomMembership::Invited,
            );
        }
        Ok(())
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        room_info::pause_avatars().await;
        media::fetch_member_avatars(mxcs)
    }
}

#[async_trait]
impl TypingPort for DemoAuthed {
    async fn follow_typing(
        &self,
        room_id: &RoomId,
        typists_tx: mpsc::Sender<Vec<NamedUser>>,
    ) -> Result<()> {
        let typists = typing::typists_in(room_id);
        if !typists.is_empty() {
            typing::type_in_the_room(typists, &typists_tx).await;
        }
        Ok(())
    }

    async fn notify_typing(&self, room_id: &RoomId, is_typing: bool) -> Result<()> {
        typing::pause_notice().await;
        if typing::scenario().notices_fail {
            return Err(unavailable("telling the room you are typing"));
        }
        self.typing_notices.notice(room_id, is_typing);
        Ok(())
    }
}

#[async_trait]
impl PresencePort for DemoAuthed {
    async fn announce(&self, availability: Availability, status: Option<&str>) -> BeaconResult {
        self.presence.announce(availability, status).await
    }

    async fn presence_of(&self, user_id: &UserId) -> LookupResult {
        self.presence.presence_of(user_id).await
    }
}

#[async_trait]
impl RoomInfoPort for DemoAuthed {
    async fn about(&self, room_id: &RoomId) -> Result<RoomAbout> {
        data::room_about(room_id)
            .or_else(|| self.created(room_id).as_ref().map(data::created_about))
            .ok_or_else(|| unavailable("this room's details"))
    }

    async fn roster(&self, room_id: &RoomId) -> Result<Vec<RosterMember>> {
        room_info::pause().await;
        if room_info::roster_fails_now(room_id) {
            return Err(unavailable("this room's member list"));
        }
        Ok(self.session_roster(room_id))
    }

    async fn readers(&self, room_id: &RoomId, user_ids: &[String]) -> Result<Vec<Reader>> {
        room_info::pause().await;
        Ok(data::readers(room_id, user_ids, room_info::member_avatar))
    }

    async fn set_notify(&self, room_id: &RoomId, mode: NotifyMode) -> Result<()> {
        room_info::pause().await;
        if room_info::scenario().notify_fails {
            return Err(unavailable("changing notifications"));
        }
        tracing::debug!(%room_id, ?mode, "demo: changing a room's notifications");
        if room_info::scenario().echo_is_lost {
            return Ok(());
        }
        self.change_lists(|lists| {
            lists.overrides.notify.insert(room_id.to_string(), mode);
        });
        self.echo_rooms();
        Ok(())
    }

    async fn mark_read(&self, room_id: &RoomId) -> Result<()> {
        room_info::pause().await;
        if room_info::scenario().read_fails {
            return Err(unavailable("marking rooms as read"));
        }
        tracing::debug!(%room_id, "demo: marking a room as read");
        self.change_lists(|lists| {
            lists.overrides.read.insert(room_id.to_string());
        });
        self.echo_rooms();
        Ok(())
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        room_info::pause_avatars().await;
        media::fetch_member_avatars(mxcs)
    }

    async fn room_link(&self, room_id: &RoomId) -> Result<String> {
        room_info::pause().await;
        if room_info::scenario().link_fails {
            return Err(unavailable("room links"));
        }
        data::room_link_of(room_id).ok_or_else(|| unavailable("this room's link"))
    }

    async fn event_link(&self, room_id: &RoomId, event_id: &str) -> Result<String> {
        message_menu::pause().await;
        if message_menu::scenario().link_fails {
            return Err(unavailable("message links"));
        }
        Ok(data::event_link(room_id, event_id))
    }
}

#[async_trait]
impl UserInfoPort for DemoAuthed {
    async fn profile(&self, room_id: &RoomId, user_id: &UserId) -> Result<UserProfile> {
        if user_info::profile_fails_now(user_id) {
            return Err(unavailable("this profile"));
        }
        Ok(self.person(room_id, user_id))
    }

    async fn pronouns(&self, user_id: &UserId) -> Vec<String> {
        user_info::pause().await;
        data::pronouns(user_id)
    }

    async fn global_profile(&self, user_id: &UserId) -> Result<GlobalProfile> {
        user_info::pause().await;
        Ok(GlobalProfile {
            display_name: data::person_name(user_id),
            avatar_mxc: Some(user_info::avatar(user_id)),
            pronouns: data::pronouns(user_id),
        })
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        user_info::pause().await;
        media::fetch_member_avatars(mxcs)
    }

    async fn start_dm(&self, user_id: &UserId) -> Result<RoomId> {
        user_info::pause().await;
        if user_info::scenario().dm_fails {
            return Err(unavailable("starting chats"));
        }
        if let Some(existing) = self.chat_with(user_id) {
            return Ok(existing);
        }
        let room_id = RoomId::new(data::started_chat_id(user_id));
        tracing::debug!(%room_id, %user_id, "demo: starting a chat");
        if user_info::scenario().dm_echo_lost {
            return Ok(room_id);
        }
        self.change_lists(|lists| {
            lists
                .overrides
                .started_chats
                .insert(room_id.to_string(), user_id.to_string());
        });
        self.echo_rooms();
        Ok(room_id)
    }

    async fn moderate(
        &self,
        room_id: &RoomId,
        user_id: &UserId,
        action: Moderation,
        reason: Option<&str>,
    ) -> Result<()> {
        user_info::pause().await;
        if user_info::moderation_fails(action) {
            return Err(unavailable("moderating this room"));
        }
        if !self.person(room_id, user_id).offers(action) {
            return Err(unavailable("that moderation"));
        }
        tracing::info!(%room_id, %user_id, ?action, reason, "demo: moderating");
        if let Ok(mut memberships) = self.memberships.lock() {
            memberships.insert((room_id.clone(), user_id.to_string()), action.leaves());
        }
        Ok(())
    }

    async fn set_ignored(&self, user_id: &UserId, change: IgnoreChange) -> Result<()> {
        user_info::pause().await;
        if user_info::scenario().ignore_fails {
            return Err(unavailable("changing the ignore list"));
        }
        tracing::debug!(%user_id, ?change, "demo: changing the ignore list");
        if let Ok(mut ignored) = self.ignored.lock() {
            match change {
                IgnoreChange::Ignore => {
                    ignored.insert(user_id.to_string());
                }
                IgnoreChange::Unignore => {
                    ignored.remove(user_id.as_ref());
                }
            }
        }
        self.replay_live_window();
        Ok(())
    }

    async fn follow_trust(
        &self,
        user_id: &UserId,
        trust_tx: mpsc::Sender<UserTrust>,
    ) -> Result<()> {
        user_info::pause().await;
        let mut decisions = self.identity.subscribe();
        let mut verifications = self.verified_people.subscribe();
        let mut reported = None;
        loop {
            let trust = self.user_trust(user_id);
            if reported.as_ref() != Some(&trust) {
                if trust_tx.send(trust.clone()).await.is_err() {
                    return Ok(());
                }
                reported = Some(trust);
            }
            let changed = tokio::select! {
                changed = decisions.changed() => changed,
                changed = verifications.changed() => changed,
            };
            if changed.is_err() {
                return Ok(());
            }
        }
    }
}
