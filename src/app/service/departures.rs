use super::AppService;
use crate::app::event::DepartureEvent;
use crate::commands::ui::DepartureCommand;
use crate::domain::departure::RemovalList;
use crate::domain::room::RoomId;

impl AppService {
    pub(super) fn leave_room(&mut self, room_id: &RoomId) {
        if let Some(port) = self.port(|a| &a.leaves) {
            self.room_info
                .leave(port, self.room_directory.room(room_id));
        }
    }

    pub(super) fn handle_departure_command(&mut self, command: DepartureCommand) {
        match command {
            DepartureCommand::OpenRemoval(room_id) => {
                self.removals.open(self.room_directory.removal(&room_id));
            }
            DepartureCommand::CloseRemoval => self.removals.close(),
            DepartureCommand::ForgetRoom(room_id) => {
                if let Some(port) = self.port(|a| &a.leaves) {
                    self.removals
                        .forget(port, self.room_directory.removal(&room_id));
                }
            }
            DepartureCommand::OpenLeaveSpace(space_id) => {
                if let Some(port) = self.port(|a| &a.leaves) {
                    let departure = self.room_directory.space_departure(&space_id);
                    self.space_leave
                        .open(port, self.room_directory.space(&space_id), departure);
                }
            }
            DepartureCommand::CloseLeaveSpace => self.space_leave.close(),
            DepartureCommand::LeaveSpace {
                space_id,
                with_rooms,
            } => {
                if let Some(port) = self.port(|a| &a.leaves) {
                    if with_rooms {
                        for room_id in self.space_leave.departing_rooms(&space_id) {
                            self.calls.left_room(&room_id, &self.room_directory);
                        }
                    }
                    self.space_leave.leave(port, &space_id, with_rooms);
                }
            }
        }
    }

    pub(super) fn handle_departure_event(&mut self, event: DepartureEvent) {
        match event {
            DepartureEvent::RemovalForgotten {
                room_id,
                name,
                outcome,
            } => {
                if self.removals.forgotten(&room_id, &name, outcome) {
                    self.room_directory.forgot(&room_id);
                    self.room_directory.emit_directory(&self.selection);
                }
            }
            DepartureEvent::StandingRead {
                generation,
                standing,
            } => self.space_leave.standing_read(generation, standing),
            DepartureEvent::SpaceLeft {
                space_id,
                name,
                outcome,
            } => self.space_leave.left(&space_id, &name, outcome),
        }
    }

    pub(super) fn handle_removals_updated(&mut self, removals: RemovalList) {
        if self.held_session.running().is_none() {
            return;
        }
        if self.room_directory.store_removals(removals) {
            self.room_directory.emit_directory(&self.selection);
            self.follow_removals();
        }
    }

    pub(super) fn follow_removals(&mut self) {
        self.removals.follow(self.room_directory.removals());
    }

    pub(super) fn follow_space_leave(&mut self) {
        let Some(space_id) = self.space_leave.shown_space() else {
            return;
        };
        let departure = self.room_directory.space_departure(&space_id);
        self.space_leave
            .follow(self.room_directory.space(&space_id), departure);
    }
}
