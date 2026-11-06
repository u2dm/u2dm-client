mod direct_chat;
mod moderation;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use self::direct_chat::{DmStart, direct_with};
use self::moderation::{pending_moderation, settle_accepted_ignore, settle_accepted_membership};
use super::event::{AppEvent, UserInfoEvent};
use super::input::EventSender;
use super::report_in;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{
    CardStatus, DirectChat, PendingModeration, UserCard, UserInfoView, VerifyOffer,
};
use crate::domain::room::{Room, RoomId};
use crate::domain::user_info::{
    GlobalProfile, IdentityTrust, IgnoreChange, Moderation, Pronouns, RoomMembership, UserId,
    UserProfile, UserTrust,
};
use crate::ports::matrix::UserInfoPort;
use crate::ports::output::AppOutputPort;

const TRUST_CHANNEL_CAP: usize = 8;

pub(super) enum ReadOutcome {
    Loaded(Box<UserProfile>),
    Failed,
}

pub(super) enum DmOutcome {
    Started(RoomId),
    Failed,
}

#[derive(Clone, Copy)]
pub(super) enum VerifyOutcome {
    Sent,
    PartnerLeft,
    Failed,
}

pub(super) enum ChatReady {
    Open(RoomId),
    Verify(UserId),
}

enum Card {
    Reading,
    Loaded(UserProfile),
    Failed,
    Retrying,
}

type Seat = (RoomId, UserId);

#[derive(Clone, Copy)]
struct SheetActions {
    direct: DirectChat,
    verify: VerifyOffer,
    ignore_busy: bool,
    moderating: PendingModeration,
}

struct OpenSheet {
    room_id: RoomId,
    user_id: UserId,
    generation: u64,
    card: Card,
    direct_listed: bool,
    trust: Option<UserTrust>,
    following_trust: bool,
    avatars_ready: usize,
    error: UserMessage,
}

impl OpenSheet {
    fn shows(&self, room_id: &RoomId, user_id: &UserId) -> bool {
        self.room_id == *room_id && self.user_id == *user_id
    }

    fn profile(&self) -> Option<&UserProfile> {
        match &self.card {
            Card::Loaded(profile) => Some(profile),
            Card::Reading | Card::Failed | Card::Retrying => None,
        }
    }

    fn loaded_mut(&mut self) -> Option<&mut UserProfile> {
        match &mut self.card {
            Card::Loaded(profile) => Some(profile),
            Card::Reading | Card::Failed | Card::Retrying => None,
        }
    }

    fn direct_room(&self) -> Option<&RoomId> {
        self.profile()?.direct_room.as_ref()
    }

    fn view(&self, actions: SheetActions) -> UserInfoView {
        let shown = match &self.card {
            Card::Reading => None,
            Card::Loaded(profile) => Some((profile.clone(), CardStatus::Loaded)),
            Card::Failed => Some((self.placeholder(), CardStatus::ReadFailed)),
            Card::Retrying => Some((self.placeholder(), CardStatus::Retrying)),
        };
        let Some((profile, status)) = shown else {
            return UserInfoView::default();
        };
        UserInfoView {
            card: Some(UserCard {
                room_id: self.room_id.clone(),
                profile,
                status,
            }),
            direct: actions.direct,
            verify: actions.verify,
            sessions: self
                .trust
                .as_ref()
                .map(|trust| Arc::from(trust.devices.as_slice())),
            ignore_busy: actions.ignore_busy,
            moderating: actions.moderating,
            avatars_ready: self.avatars_ready,
            error: self.error.clone(),
        }
    }

    fn placeholder(&self) -> UserProfile {
        UserProfile::placeholder(self.user_id.clone())
    }

    fn identity(&self) -> Option<IdentityTrust> {
        self.trust.as_ref().map(|trust| trust.identity)
    }

    fn chat_holds_requests(&self, rooms: &[Arc<Room>]) -> bool {
        self.direct_room().is_some() || direct_with(rooms, &self.user_id).is_some()
    }
}

enum Lookup {
    Global,
    Pronouns,
}

pub(super) struct UserInfo {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    actions: TaskGroup,
    generation: u64,
    open: Option<OpenSheet>,
    starting: HashMap<UserId, DmStart>,
    verifying: HashSet<UserId>,
    session_verified: bool,
    ignoring: HashMap<UserId, IgnoreChange>,
    accepted_ignores: HashMap<UserId, bool>,
    moderating: HashMap<Seat, Moderation>,
    accepted_moderation: HashMap<Seat, RoomMembership>,
}

impl UserInfo {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("user-info"),
            actions: TaskGroup::new("user-actions"),
            generation: 0,
            open: None,
            starting: HashMap::new(),
            verifying: HashSet::new(),
            session_verified: false,
            ignoring: HashMap::new(),
            accepted_ignores: HashMap::new(),
            moderating: HashMap::new(),
            accepted_moderation: HashMap::new(),
        }
    }

    pub(super) fn open(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        room_id: Option<RoomId>,
        user_id: UserId,
    ) {
        let Some(room_id) = room_id else {
            tracing::debug!(%user_id, "no room is selected, not opening the user info");
            return;
        };
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.shows(&room_id, &user_id))
        {
            return;
        }
        self.tasks.cancel_and_detach();
        self.forget_awaited_chats();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        tracing::debug!(%user_id, room = %room_id, "opening the user info");
        self.open = Some(OpenSheet {
            room_id: room_id.clone(),
            user_id: user_id.clone(),
            generation,
            card: Card::Reading,
            direct_listed: false,
            trust: None,
            following_trust: false,
            avatars_ready: 0,
            error: UserMessage::default(),
        });
        self.publish();
        self.spawn_read(port, room_id, user_id, generation);
    }

    pub(super) fn person(&self) -> Option<&UserId> {
        self.open.as_ref().map(|open| &open.user_id)
    }

    pub(super) fn close(&mut self) {
        if self.open.take().is_none() {
            return;
        }
        self.tasks.cancel_and_detach();
        self.forget_awaited_chats();
        self.publish();
    }

    pub(super) fn retry(&mut self, port: Arc<dyn UserInfoPort>) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| matches!(open.card, Card::Failed))
        else {
            return;
        };
        open.card = Card::Retrying;
        let (room_id, user_id, generation) =
            (open.room_id.clone(), open.user_id.clone(), open.generation);
        self.publish();
        self.spawn_read(port, room_id, user_id, generation);
    }

    pub(super) fn read_landed(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        generation: u64,
        outcome: ReadOutcome,
        rooms: &[Arc<Room>],
    ) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.generation == generation)
        else {
            tracing::debug!(generation, "dropping a superseded profile read");
            return;
        };
        let ReadOutcome::Loaded(profile) = outcome else {
            open.card = Card::Failed;
            open.error = UserMessage::about(UserMessageKind::UserInfoFailed, &open.user_id);
            self.publish();
            return;
        };
        let mut profile = *profile;
        settle_accepted_ignore(&mut self.accepted_ignores, &mut profile);
        settle_accepted_membership(&mut self.accepted_moderation, &open.room_id, &mut profile);
        let lookup = if profile.wants_global_profile() {
            Some(Lookup::Global)
        } else if profile.wants_pronouns() {
            Some(Lookup::Pronouns)
        } else {
            None
        };
        let avatar = profile.avatar_mxc.clone();
        let user_id = open.user_id.clone();
        open.direct_listed = profile
            .direct_room
            .as_ref()
            .is_some_and(|room_id| listed(rooms, room_id));
        if let Some(identity) = open.identity() {
            profile.trust = identity;
        }
        let follows_trust = !profile.is_self && !open.following_trust;
        open.following_trust |= follows_trust;
        open.card = Card::Loaded(profile);
        open.error = UserMessage::default();
        self.publish();
        if follows_trust {
            self.spawn_trust_follow(Arc::clone(&port), user_id.clone(), generation);
        }
        match lookup {
            Some(Lookup::Global) => {
                self.spawn_global_profile(Arc::clone(&port), user_id, generation);
            }
            Some(Lookup::Pronouns) => {
                self.spawn_pronouns(Arc::clone(&port), user_id, generation);
            }
            None => {}
        }
        if let Some(mxc) = avatar {
            self.spawn_avatar(port, mxc, generation);
        }
    }

    pub(super) fn pronouns_landed(&mut self, generation: u64, pronouns: Vec<String>) {
        let Some(profile) = self.loaded_mut(generation) else {
            return;
        };
        profile.pronouns = Pronouns::Known(pronouns);
        self.publish();
    }

    pub(super) fn profile_landed(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        generation: u64,
        global: Option<GlobalProfile>,
    ) {
        let Some(global) = global else {
            return;
        };
        let Some(profile) = self.loaded_mut(generation) else {
            return;
        };
        let had_avatar = profile.avatar_mxc.is_some();
        profile.adopt_global(global);
        let fresh_avatar = profile.avatar_mxc.clone().filter(|_| !had_avatar);
        self.publish();
        if let Some(mxc) = fresh_avatar {
            self.spawn_avatar(port, mxc, generation);
        }
    }

    pub(super) fn trust_changed(&mut self, generation: u64, trust: UserTrust) {
        let Some(open) = self.current(generation) else {
            tracing::debug!(generation, "dropping a superseded identity read");
            return;
        };
        if let Some(profile) = open.loaded_mut() {
            profile.trust = trust.identity;
        }
        open.trust = Some(trust);
        self.publish();
    }

    pub(super) fn session_trust_changed(&mut self, verified: bool) {
        if self.session_verified == verified {
            return;
        }
        self.session_verified = verified;
        self.publish();
    }

    pub(super) fn avatar_ready(&mut self, generation: u64) {
        let Some(open) = self.current(generation) else {
            return;
        };
        open.avatars_ready = open.avatars_ready.saturating_add(1);
        self.publish();
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.tasks.restart(), self.actions.restart());
        self.open = None;
        self.starting.clear();
        self.verifying.clear();
        self.session_verified = false;
        self.ignoring.clear();
        self.accepted_ignores.clear();
        self.moderating.clear();
        self.accepted_moderation.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.tasks.shutdown(), self.actions.shutdown());
    }

    fn shown_name(&self, user_id: &UserId) -> String {
        self.open
            .as_ref()
            .filter(|open| open.user_id == *user_id)
            .and_then(OpenSheet::profile)
            .map_or_else(|| user_id.to_string(), UserProfile::shown)
    }

    fn clear_error(&mut self, user_id: &UserId) {
        if let Some(open) = self.open.as_mut().filter(|open| open.user_id == *user_id) {
            open.error = UserMessage::default();
        }
    }

    fn report(&mut self, user_id: &UserId, message: UserMessage) {
        let shown = self.open.as_mut().filter(|open| open.user_id == *user_id);
        report_in(
            self.output.as_ref(),
            shown.map(|open| &mut open.error),
            message,
        );
    }

    fn loaded_mut(&mut self, generation: u64) -> Option<&mut UserProfile> {
        self.current(generation).and_then(OpenSheet::loaded_mut)
    }

    fn spawn_read(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        room_id: RoomId,
        user_id: UserId,
        generation: u64,
    ) {
        self.tasks.spawn_reporting(&self.events, async move {
            let outcome = match port.profile(&room_id, &user_id).await {
                    Ok(profile) => ReadOutcome::Loaded(Box::new(profile)),
                    Err(e) => {
                        tracing::warn!(%user_id, room = %room_id, "failed to read the user's profile: {e}");
                        ReadOutcome::Failed
                    }
                };
            AppEvent::UserInfo(UserInfoEvent::Read {
                generation,
                outcome,
            })
        });
    }

    fn spawn_pronouns(&mut self, port: Arc<dyn UserInfoPort>, user_id: UserId, generation: u64) {
        self.tasks.spawn_reporting(&self.events, async move {
            let pronouns = port.pronouns(&user_id).await;
            AppEvent::UserInfo(UserInfoEvent::PronounsFetched {
                generation,
                pronouns,
            })
        });
    }

    fn spawn_global_profile(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        user_id: UserId,
        generation: u64,
    ) {
        self.tasks.spawn_reporting(&self.events, async move {
            let profile = port
                .global_profile(&user_id)
                .await
                .inspect_err(
                    |e| tracing::debug!(%user_id, "could not fetch the global profile: {e}"),
                )
                .ok();
            AppEvent::UserInfo(UserInfoEvent::ProfileFetched {
                generation,
                profile,
            })
        });
    }

    fn spawn_trust_follow(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        user_id: UserId,
        generation: u64,
    ) {
        self.tasks.spawn_follow(
            &self.events,
            TRUST_CHANNEL_CAP,
            move |trust_tx| async move {
                if let Err(e) = port.follow_trust(&user_id, trust_tx).await {
                    tracing::debug!(%user_id, "stopped following the user's sessions: {e}");
                }
            },
            move |trust| AppEvent::UserInfo(UserInfoEvent::TrustChanged { generation, trust }),
        );
    }

    fn spawn_avatar(&mut self, port: Arc<dyn UserInfoPort>, mxc: String, generation: u64) {
        self.tasks.spawn_reporting(&self.events, async move {
            let ready = port.fetch_avatars(&[mxc]).await;
            (ready > 0).then_some(AppEvent::UserInfo(UserInfoEvent::AvatarReady {
                generation,
            }))
        });
    }

    fn current(&mut self, generation: u64) -> Option<&mut OpenSheet> {
        self.open
            .as_mut()
            .filter(|open| open.generation == generation)
    }

    fn publish(&self) {
        let view = self
            .open
            .as_ref()
            .map_or_else(UserInfoView::default, |open| {
                open.view(SheetActions {
                    direct: self.direct_chat(open),
                    verify: self.verify_offer(open),
                    ignore_busy: self.ignoring.contains_key(&open.user_id),
                    moderating: pending_moderation(
                        self.moderating
                            .get(&(open.room_id.clone(), open.user_id.clone())),
                    ),
                })
            });
        self.output
            .publish(Box::new(move |state| state.user_info = view));
    }
}

fn listed(rooms: &[Arc<Room>], room_id: &RoomId) -> bool {
    rooms.iter().any(|room| room.id == *room_id)
}
