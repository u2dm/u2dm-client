use std::collections::HashMap;
use std::sync::Arc;

use super::event::{AppEvent, UserInfoEvent};
use super::input::EventSender;
use super::room_info::ActionOutcome;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{
    CardStatus, DirectChat, PendingModeration, Toast, UserCard, UserInfoView,
};
use crate::domain::room::{Room, RoomId};
use crate::domain::user_info::{
    GlobalProfile, IgnoreChange, Moderation, Pronouns, RoomMembership, UserId, UserProfile,
};
use crate::ports::matrix::UserInfoPort;
use crate::ports::output::AppOutputPort;

pub(super) enum ReadOutcome {
    Loaded(Box<UserProfile>),
    Failed,
}

pub(super) enum DmOutcome {
    Started(RoomId),
    Failed,
}

enum Card {
    Reading,
    Loaded(UserProfile),
    Failed,
    Retrying,
}

enum DmStart {
    Requested,
    AwaitingSync(RoomId),
}

type Seat = (RoomId, UserId);

#[derive(Clone, Copy)]
struct SheetActions {
    direct: DirectChat,
    ignore_busy: bool,
    moderating: PendingModeration,
}

struct OpenSheet {
    room_id: RoomId,
    user_id: UserId,
    generation: u64,
    card: Card,
    direct_listed: bool,
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
            ignore_busy: actions.ignore_busy,
            moderating: actions.moderating,
            avatars_ready: self.avatars_ready,
            error: self.error.clone(),
        }
    }

    fn placeholder(&self) -> UserProfile {
        UserProfile::placeholder(self.user_id.clone())
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
            avatars_ready: 0,
            error: UserMessage::default(),
        });
        self.publish();
        self.spawn_read(port, room_id, user_id, generation);
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
        open.card = Card::Loaded(profile);
        open.error = UserMessage::default();
        self.publish();
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

    pub(super) fn avatar_ready(&mut self, generation: u64) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.generation == generation)
        else {
            return;
        };
        open.avatars_ready = open.avatars_ready.saturating_add(1);
        self.publish();
    }

    pub(super) fn message(
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
                let name = profile.label().to_owned();
                self.start_dm(port, user_id.clone(), name);
                None
            }
            DirectChat::Hidden | DirectChat::Starting => {
                tracing::debug!(%user_id, "ignoring a message request the sheet does not offer");
                None
            }
        }
    }

    pub(super) fn dm_started(
        &mut self,
        user_id: UserId,
        name: &str,
        outcome: DmOutcome,
        rooms: &[Arc<Room>],
    ) -> Option<RoomId> {
        if self.starting.remove(&user_id).is_none() {
            tracing::debug!(%user_id, "dropping a started chat nobody is waiting for");
            return None;
        }
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
        if listed(rooms, &room_id) {
            return Some(room_id);
        }
        tracing::debug!(%user_id, %room_id, "started a chat, waiting for the room list to show it");
        self.starting
            .insert(user_id, DmStart::AwaitingSync(room_id));
        self.publish();
        None
    }

    pub(super) fn follow(&mut self, rooms: &[Arc<Room>]) -> Option<RoomId> {
        let open = self.open.as_mut()?;
        let direct_listed = open
            .direct_room()
            .is_some_and(|room_id| listed(rooms, room_id));
        let moved = open.direct_listed != direct_listed;
        open.direct_listed = direct_listed;
        let arrived = match self.starting.get(&open.user_id) {
            Some(DmStart::AwaitingSync(room_id)) if listed(rooms, room_id) => Some(room_id.clone()),
            Some(DmStart::Requested | DmStart::AwaitingSync(_)) | None => None,
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

    pub(super) fn set_ignored(
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
        let name = profile.label().to_owned();
        self.ignoring.insert(user_id.clone(), change);
        self.clear_error(user_id);
        self.publish();

        let user_id = user_id.clone();
        let events = self.events.clone();
        let cancel = self.actions.token();
        self.actions.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                changed = port.set_ignored(&user_id, change) => match changed {
                    Ok(()) => ActionOutcome::Done,
                    Err(e) => {
                        tracing::warn!(%user_id, ?change, "failed to change the ignore list: {e}");
                        ActionOutcome::Failed
                    }
                },
            };
            let settled = UserInfoEvent::IgnoreSettled {
                user_id,
                name,
                change,
                outcome,
            };
            drop(events.send(AppEvent::UserInfo(settled)));
        });
    }

    pub(super) fn ignore_settled(
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

    pub(super) fn moderate(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        user_id: &UserId,
        action: Moderation,
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
        let name = profile.label().to_owned();
        self.moderating.insert(seat.clone(), action);
        self.clear_error(user_id);
        self.publish();

        let events = self.events.clone();
        let cancel = self.actions.token();
        self.actions.spawn(async move {
            let (room_id, user_id) = seat;
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                done = port.moderate(&room_id, &user_id, action) => match done {
                    Ok(()) => ActionOutcome::Done,
                    Err(e) => {
                        tracing::warn!(%user_id, room = %room_id, ?action, "failed to moderate: {e}");
                        ActionOutcome::Failed
                    }
                },
            };
            let settled = UserInfoEvent::ModerationSettled {
                room_id,
                user_id,
                name,
                action,
                outcome,
            };
            drop(events.send(AppEvent::UserInfo(settled)));
        });
    }

    pub(super) fn moderation_settled(
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

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.tasks.restart(), self.actions.restart());
        self.open = None;
        self.starting.clear();
        self.ignoring.clear();
        self.accepted_ignores.clear();
        self.moderating.clear();
        self.accepted_moderation.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.tasks.shutdown(), self.actions.shutdown());
    }

    fn direct_chat(&self, open: &OpenSheet) -> DirectChat {
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

    fn forget_awaited_chats(&mut self) {
        self.starting
            .retain(|_, start| matches!(start, DmStart::Requested));
    }

    fn clear_error(&mut self, user_id: &UserId) {
        if let Some(open) = self.open.as_mut().filter(|open| open.user_id == *user_id) {
            open.error = UserMessage::default();
        }
    }

    fn report(&mut self, user_id: &UserId, message: UserMessage) {
        match self.open.as_mut().filter(|open| open.user_id == *user_id) {
            Some(open) => open.error = message,
            None => show_toast(self.output.as_ref(), Toast::Error(message)),
        }
    }

    fn loaded_mut(&mut self, generation: u64) -> Option<&mut UserProfile> {
        self.open
            .as_mut()
            .filter(|open| open.generation == generation)
            .and_then(OpenSheet::loaded_mut)
    }

    fn start_dm(&mut self, port: Arc<dyn UserInfoPort>, user_id: UserId, name: String) {
        self.starting.insert(user_id.clone(), DmStart::Requested);
        self.clear_error(&user_id);
        self.publish();
        let events = self.events.clone();
        let cancel = self.actions.token();
        self.actions.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                started = port.start_dm(&user_id) => match started {
                    Ok(room_id) => DmOutcome::Started(room_id),
                    Err(e) => {
                        tracing::warn!(%user_id, "failed to start a chat: {e}");
                        DmOutcome::Failed
                    }
                },
            };
            let started = UserInfoEvent::DmStarted {
                user_id,
                name,
                outcome,
            };
            drop(events.send(AppEvent::UserInfo(started)));
        });
    }

    fn spawn_read(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        room_id: RoomId,
        user_id: UserId,
        generation: u64,
    ) {
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                read = port.profile(&room_id, &user_id) => match read {
                    Ok(profile) => ReadOutcome::Loaded(Box::new(profile)),
                    Err(e) => {
                        tracing::warn!(%user_id, room = %room_id, "failed to read the user's profile: {e}");
                        ReadOutcome::Failed
                    }
                },
            };
            drop(events.send(AppEvent::UserInfo(UserInfoEvent::Read {
                generation,
                outcome,
            })));
        });
    }

    fn spawn_pronouns(&mut self, port: Arc<dyn UserInfoPort>, user_id: UserId, generation: u64) {
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let pronouns = tokio::select! {
                () = cancel.cancelled() => return,
                pronouns = port.pronouns(&user_id) => pronouns,
            };
            drop(
                events.send(AppEvent::UserInfo(UserInfoEvent::PronounsFetched {
                    generation,
                    pronouns,
                })),
            );
        });
    }

    fn spawn_global_profile(
        &mut self,
        port: Arc<dyn UserInfoPort>,
        user_id: UserId,
        generation: u64,
    ) {
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let profile = tokio::select! {
                () = cancel.cancelled() => return,
                profile = port.global_profile(&user_id) => profile
                    .inspect_err(|e| tracing::debug!(%user_id, "could not fetch the global profile: {e}"))
                    .ok(),
            };
            drop(events.send(AppEvent::UserInfo(UserInfoEvent::ProfileFetched {
                generation,
                profile,
            })));
        });
    }

    fn spawn_avatar(&mut self, port: Arc<dyn UserInfoPort>, mxc: String, generation: u64) {
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let mxcs = [mxc];
            let ready = tokio::select! {
                () = cancel.cancelled() => return,
                ready = port.fetch_avatars(&mxcs) => ready,
            };
            if ready > 0 {
                drop(events.send(AppEvent::UserInfo(UserInfoEvent::AvatarReady {
                    generation,
                })));
            }
        });
    }

    fn publish(&self) {
        let view = self
            .open
            .as_ref()
            .map_or_else(UserInfoView::default, |open| {
                open.view(SheetActions {
                    direct: self.direct_chat(open),
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

fn settle_accepted_membership(
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

fn pending_moderation(action: Option<&Moderation>) -> PendingModeration {
    match action {
        None => PendingModeration::None,
        Some(Moderation::Kick) => PendingModeration::Kick,
        Some(Moderation::Ban) => PendingModeration::Ban,
        Some(Moderation::Unban) => PendingModeration::Unban,
    }
}

fn settle_accepted_ignore(accepted: &mut HashMap<UserId, bool>, profile: &mut UserProfile) {
    let Some(&ignored) = accepted.get(&profile.user_id) else {
        return;
    };
    if profile.ignored == ignored {
        accepted.remove(&profile.user_id);
    } else {
        profile.ignored = ignored;
    }
}
