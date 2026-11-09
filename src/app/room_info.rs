mod actions;
mod roster;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use self::actions::NotifyChange;
use self::roster::{Fetch, MEMBER_PAGE, Members, Roster, derive_rows, unrequested_avatars};
use super::input::EventSender;
use super::link_requests::LinkRequests;
use super::report_in;
use super::task_group::TaskGroup;
use crate::commands::messages::UserMessage;
use crate::commands::view::{RoomCard, RoomInfoPlacement, RoomInfoView, RoomMenuTarget, RosterRow};
use crate::domain::room::{NotifyMode, Room, RoomId, Space};
use crate::domain::room_info::{MemberQuery, RoomAbout};
use crate::domain::settings::ReceiptVisibility;
use crate::ports::matrix::RoomInfoPort;
use crate::ports::output::AppOutputPort;

pub(super) enum RosterOutcome {
    Loaded(Members),
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Room {
        unread: bool,
        notify: NotifyMode,
        has_call: bool,
    },
    Space {
        may_add_children: bool,
    },
}

#[derive(Clone, PartialEq, Eq)]
struct MenuCard {
    id: RoomId,
    name: String,
    may_invite: bool,
    kind: MenuKind,
}

struct OpenSheet {
    card: RoomCard,
    generation: u64,
    roster: Roster,
    fetch: Fetch,
    about: Option<RoomAbout>,
    query: String,
    shown: usize,
    rows: Arc<[RosterRow]>,
    has_more: bool,
    requested: HashSet<String>,
    avatars_ready: usize,
    error: UserMessage,
}

impl OpenSheet {
    fn adopt(&mut self, outcome: RosterOutcome) {
        match outcome {
            RosterOutcome::Loaded(members) => {
                tracing::debug!(
                    room = %self.card.id,
                    members = members.len(),
                    "loaded the member list"
                );
                self.roster = Roster::Ready(members);
            }
            RosterOutcome::Failed if matches!(self.roster, Roster::Ready(_)) => {
                tracing::debug!(room = %self.card.id, "keeping the member list a refresh failed to replace");
            }
            RosterOutcome::Failed => self.roster = Roster::Failed,
        }
    }
}

pub(super) struct RoomInfo {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    actions: TaskGroup,
    generation: u64,
    requests: u64,
    pages_landed: i32,
    open: Option<OpenSheet>,
    pane_up: bool,
    menu: Option<MenuCard>,
    notify: HashMap<RoomId, NotifyChange>,
    leaving: HashSet<RoomId>,
    marking: HashSet<RoomId>,
    links: LinkRequests,
    receipts: ReceiptVisibility,
}

impl RoomInfo {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("room-info"),
            actions: TaskGroup::new("room-actions"),
            generation: 0,
            requests: 0,
            pages_landed: 0,
            open: None,
            pane_up: false,
            menu: None,
            notify: HashMap::new(),
            leaving: HashSet::new(),
            marking: HashSet::new(),
            links: LinkRequests::default(),
            receipts: ReceiptVisibility::default(),
        }
    }

    pub(super) const fn set_receipts(&mut self, receipts: ReceiptVisibility) {
        self.receipts = receipts;
    }

    pub(super) fn open(&mut self, port: Arc<dyn RoomInfoPort>, room: Option<&Room>) {
        let Some(room) = room else {
            tracing::debug!("the room is not listed, not opening its room info");
            return;
        };
        if !self.shows(&room.id) {
            self.begin(port, card_of(room));
        }
    }

    pub(super) fn close(&mut self) {
        if self.open.take().is_none() {
            return;
        }
        self.tasks.cancel_and_detach();
        self.publish();
    }

    pub(super) fn close_dialog(&mut self) {
        if self.pane_up {
            tracing::debug!("the room info is in the pane, which only its toggle closes");
            return;
        }
        self.close();
    }

    pub(super) fn show_pane(&mut self, port: Arc<dyn RoomInfoPort>, selected: Option<&Room>) {
        if self.pane_up {
            return;
        }
        self.pane_up = true;
        if self.open.is_some() {
            self.publish();
        }
        self.follow_selection(port, selected);
    }

    pub(super) fn hide_pane(&mut self) {
        if !self.pane_up {
            return;
        }
        self.pane_up = false;
        self.close();
    }

    pub(super) fn follow_selection(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        selected: Option<&Room>,
    ) {
        match selected.filter(|_| self.pane_up) {
            Some(room) if !self.shows(&room.id) => self.begin(port, card_of(room)),
            Some(_) => {}
            None => self.close(),
        }
    }

    pub(super) fn open_menu(&mut self, room: Option<&Room>, space: Option<&Space>) {
        let menu = room.map(menu_card_of).or_else(|| space.map(space_card_of));
        if menu.is_none() {
            tracing::debug!("neither a listed room nor a joined space, not opening its menu");
        }
        if self.menu != menu {
            self.menu = menu;
            self.publish();
        }
    }

    pub(super) fn close_menu(&mut self) {
        if self.menu.take().is_some() {
            self.publish();
        }
    }

    pub(super) fn follow_spaces(&mut self, spaces: &[Space]) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        if !matches!(menu.kind, MenuKind::Space { .. }) {
            return;
        }
        let next = spaces
            .iter()
            .find(|space| space.id == menu.id.as_ref())
            .map(space_card_of);
        if self.menu != next {
            self.menu = next;
            self.publish();
        }
    }

    pub(super) fn follow(&mut self, port: Arc<dyn RoomInfoPort>, rooms: &[Arc<Room>]) {
        let echoed = self.settle_echoes(rooms);
        let menu_moved = self.follow_menu(rooms);
        let sheet_moved = self.follow_sheet(port, rooms);
        if echoed || menu_moved || sheet_moved {
            self.publish();
        }
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.tasks.restart(), self.actions.restart());
        self.open = None;
        self.pane_up = false;
        self.menu = None;
        self.notify.clear();
        self.leaving.clear();
        self.marking.clear();
        self.links.forget();
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.tasks.shutdown(), self.actions.shutdown());
    }

    fn begin(&mut self, port: Arc<dyn RoomInfoPort>, card: RoomCard) {
        self.tasks.cancel_and_detach();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let room_id = card.id.clone();
        tracing::debug!(room = %room_id, "opening the room info");
        self.open = Some(OpenSheet {
            card,
            generation,
            roster: Roster::Loading,
            fetch: Fetch::InFlight,
            about: None,
            query: String::new(),
            shown: MEMBER_PAGE,
            rows: Arc::from(Vec::new()),
            has_more: false,
            requested: HashSet::new(),
            avatars_ready: 0,
            error: UserMessage::default(),
        });
        self.publish();
        self.spawn_about(Arc::clone(&port), room_id.clone(), generation);
        self.spawn_roster(port, room_id, generation);
    }

    fn follow_menu(&mut self, rooms: &[Arc<Room>]) -> bool {
        let Some(menu) = self.menu.as_ref() else {
            return false;
        };
        if matches!(menu.kind, MenuKind::Space { .. }) {
            return false;
        }
        let next = rooms
            .iter()
            .find(|room| room.id == menu.id)
            .map(|room| menu_card_of(room));
        if self.menu == next {
            return false;
        }
        self.menu = next;
        true
    }

    fn follow_sheet(&mut self, port: Arc<dyn RoomInfoPort>, rooms: &[Arc<Room>]) -> bool {
        let Some(open) = self.open.as_mut() else {
            return false;
        };
        let Some(room) = rooms.iter().find(|room| room.id == open.card.id) else {
            self.open = None;
            self.tasks.cancel_and_detach();
            return true;
        };
        let card = card_of(room);
        let members_moved = card.member_count != open.card.member_count;
        let about_moved = card.alias != open.card.alias || card.topic != open.card.topic;
        let changed = card != open.card;
        open.card = card;
        if members_moved {
            self.refetch_roster(Arc::clone(&port));
        }
        if about_moved {
            self.refetch_about(port);
        }
        changed
    }

    fn shows(&self, room_id: &RoomId) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| open.card.id == *room_id)
    }

    fn drop_sheet_of(&mut self, room_id: &RoomId) -> bool {
        if !self.shows(room_id) {
            return false;
        }
        self.open = None;
        self.tasks.cancel_and_detach();
        true
    }

    fn settle_echoes(&mut self, rooms: &[Arc<Room>]) -> bool {
        let before = self.notify.len();
        self.notify.retain(|room_id, change| match change {
            NotifyChange::Requested { .. } => true,
            NotifyChange::AwaitingEcho { target } => rooms
                .iter()
                .find(|room| room.id == *room_id)
                .is_some_and(|room| room.notify != *target),
        });
        self.notify.len() != before
    }

    fn reveal(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let (rows, has_more) =
            derive_rows(&open.roster, &MemberQuery::new(&open.query), open.shown);
        if *open.rows != *rows || open.has_more != has_more {
            open.rows = rows.into();
            open.has_more = has_more;
            self.pages_landed = self.pages_landed.wrapping_add(1);
        }
        let wanted = unrequested_avatars(open);
        let generation = open.generation;
        self.publish();
        self.spawn_avatars(port, generation, wanted);
    }

    fn report(&mut self, room_id: &RoomId, message: UserMessage) {
        let shown = self.open.as_mut().filter(|open| open.card.id == *room_id);
        report_in(
            self.output.as_ref(),
            shown.map(|open| &mut open.error),
            message,
        );
    }

    fn clear_error(&mut self, room_id: &RoomId) {
        if let Some(open) = self.open.as_mut().filter(|open| open.card.id == *room_id) {
            open.error = UserMessage::default();
        }
    }

    fn menu_target(&self, menu: &MenuCard) -> RoomMenuTarget {
        match menu.kind {
            MenuKind::Room {
                unread,
                notify,
                has_call,
            } => {
                let (notify, notify_busy) = self.pending_notify(&menu.id, notify);
                RoomMenuTarget {
                    room_id: menu.id.clone(),
                    name: menu.name.clone(),
                    is_space: false,
                    unread,
                    notify,
                    notify_busy,
                    leaving: self.leaving.contains(&menu.id),
                    may_invite: menu.may_invite,
                    may_add_children: false,
                    has_call,
                }
            }
            MenuKind::Space { may_add_children } => RoomMenuTarget {
                room_id: menu.id.clone(),
                name: menu.name.clone(),
                is_space: true,
                unread: false,
                notify: NotifyMode::default(),
                notify_busy: false,
                leaving: false,
                may_invite: menu.may_invite,
                may_add_children,
                has_call: false,
            },
        }
    }

    fn current(&mut self, generation: u64) -> Option<&mut OpenSheet> {
        self.open
            .as_mut()
            .filter(|open| open.generation == generation)
    }

    fn publish(&self) {
        let sheet = self
            .open
            .as_ref()
            .map_or_else(RoomInfoView::default, |open| {
                let (notify, notify_busy) = self.pending_notify(&open.card.id, open.card.notify);
                RoomInfoView {
                    placement: if self.pane_up {
                        RoomInfoPlacement::Pane
                    } else {
                        RoomInfoPlacement::Dialog
                    },
                    card: Some(open.card.clone()),
                    about: open.about.clone(),
                    roster: open.roster.status(),
                    rows: Arc::clone(&open.rows),
                    has_more: open.has_more,
                    pages_landed: self.pages_landed,
                    avatars_ready: open.avatars_ready,
                    notify,
                    notify_busy,
                    leaving: self.leaving.contains(&open.card.id),
                    error: open.error.clone(),
                }
            });
        let menu = self.menu.as_ref().map(|menu| self.menu_target(menu));
        self.output.publish(Box::new(move |state| {
            state.room_info = sheet;
            state.room_menu = menu;
        }));
    }
}

fn menu_card_of(room: &Room) -> MenuCard {
    MenuCard {
        id: room.id.clone(),
        name: room.display_name.clone(),
        may_invite: room.may_invite,
        kind: MenuKind::Room {
            unread: room.shows_unread(),
            notify: room.notify,
            has_call: room.call.is_some(),
        },
    }
}

fn space_card_of(space: &Space) -> MenuCard {
    MenuCard {
        id: RoomId::new(space.id.clone()),
        name: space.name.clone(),
        may_invite: space.powers.invite,
        kind: MenuKind::Space {
            may_add_children: space.powers.add_children,
        },
    }
}

fn card_of(room: &Room) -> RoomCard {
    RoomCard {
        id: room.id.clone(),
        name: room.display_name.clone(),
        avatar_mxc: room.avatar_mxc.clone(),
        member_count: room.member_count,
        is_direct: room.is_direct,
        topic: room.topic.clone(),
        alias: room.canonical_alias.clone(),
        notify: room.notify,
        may_invite: room.may_invite,
        readable_by_anyone: room.readable_by_anyone(),
    }
}
