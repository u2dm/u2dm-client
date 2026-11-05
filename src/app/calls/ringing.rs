use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;

use super::devices::Devices;
use super::{Answering, Calls, Ended, Incoming, RING_CONFIRM_WAIT, RING_OUT_WAIT, RingEnd, send};
use crate::app::event::{CallEvent, RingDeadline};
use crate::app::room_directory::RoomDirectory;
use crate::commands::messages::UserMessageKind;
use crate::commands::ui::WindowFocus;
use crate::commands::view::{JoinOffer, RingView};
use crate::domain::call::devices::{DeviceKey, DeviceKind};
use crate::domain::call::ring::{CallSignal, Ring, RingId, RingKind};
use crate::domain::room::{Room, RoomId};
use crate::domain::user_info::UserId;
use crate::util::unix_now_ms;

impl Calls {
    pub(in crate::app) fn signal(&mut self, signal: CallSignal, directory: &RoomDirectory) {
        match signal {
            CallSignal::Rang(ring) => self.rang(*ring, directory),
            CallSignal::Declined { room_id, ring, by } => {
                if !self.declined_out(&ring, &by, directory) {
                    self.declined_by(&room_id, &ring, &by, directory);
                }
            }
            CallSignal::Key { room_id, key } => self.key_arrived(&room_id, *key),
            CallSignal::Hand { room_id, sighting } => {
                self.change_joined_in(&room_id, directory, |joined| {
                    joined.hand_sighted(sighting, true);
                });
            }
            CallSignal::Reacted { room_id, sighting } => {
                self.change_joined_in(&room_id, directory, |joined| {
                    joined.reaction_sighted(sighting);
                });
            }
            CallSignal::Redacted { room_id, event } => {
                self.change_joined_in(&room_id, directory, |joined| joined.hand_redacted(&event));
            }
        }
    }

    fn rang(&mut self, ring: Ring, directory: &RoomDirectory) {
        if let Some(reason) = self.ignored(&ring, directory) {
            tracing::debug!(room_id = %ring.room_id, ring = %ring.id, reason, "a call notification rings nothing");
            return;
        }
        if self.incoming.is_some() {
            self.stop(RingEnd::Superseded);
        }
        let quiet = directory.room(&ring.room_id).is_some_and(Room::muted);
        tracing::info!(room_id = %ring.room_id, ring = %ring.id, video = ring.is_video(), quiet, "a call rings");
        self.serial = self.serial.wrapping_add(1);
        self.arm_windows(&ring);
        self.incoming = Some(Incoming {
            ring,
            answering: Answering::Pending,
            serial: self.serial,
            devices: None,
            choosing: false,
        });
        self.directory_changed(directory);
    }

    fn ignored(&self, ring: &Ring, directory: &RoomDirectory) -> Option<&'static str> {
        if ring.kind == RingKind::Notify {
            return Some("it does not ask to ring");
        }
        if self
            .incoming
            .as_ref()
            .is_some_and(|incoming| incoming.ring.id == ring.id || incoming.ring.same_call(ring))
        {
            return Some("it already rings");
        }
        if self.rung.iter().any(|rung| rung.same_call(ring)) {
            return Some("this call already rang here");
        }
        let Some(room) = directory.room(&ring.room_id) else {
            return Some("its room is not listed");
        };
        let joined = self.own.as_ref().is_some_and(|own| {
            room.call
                .as_ref()
                .is_some_and(|call| call.joined_by(&own.user))
        });
        joined.then_some("you are already in the call")
    }

    fn declined_out(&mut self, ring: &RingId, by: &UserId, directory: &RoomDirectory) -> bool {
        let ours = self
            .joined
            .as_ref()
            .is_some_and(|joined| joined.announced_as(ring));
        let by_us = self.own.as_ref().is_some_and(|own| own.user == *by);
        if !ours || by_us {
            return false;
        }
        let name = self
            .joined
            .as_ref()
            .and_then(|joined| directory.room(&joined.room_id))
            .filter(|room| room.is_direct)
            .map_or_else(|| by.to_string(), |room| room.display_name.clone());
        tracing::info!(%by, "the call you started was declined");
        self.end_call(
            Some(Ended {
                kind: UserMessageKind::CallDeclinedThere,
                detail: name,
            }),
            directory,
        );
        true
    }

    fn declined_by(
        &mut self,
        room_id: &RoomId,
        ring: &RingId,
        by: &UserId,
        directory: &RoomDirectory,
    ) {
        let Some(incoming) = &self.incoming else {
            return;
        };
        if incoming.ring.id != *ring {
            return;
        }
        let ours = self.own.as_ref().is_some_and(|own| own.user == *by);
        if !ours {
            tracing::debug!(%room_id, %by, "someone else declined the call ringing here");
            return;
        }
        self.stop(RingEnd::DeclinedElsewhere);
        self.publish(directory);
    }

    pub(super) fn judge_ring(&mut self, directory: &RoomDirectory) -> Option<RingEnd> {
        let incoming = self.incoming.as_mut()?;
        let room_id = &incoming.ring.room_id;
        let Some(room) = directory.room(room_id) else {
            return Some(RingEnd::RoomGone);
        };
        let call = room.call.as_deref();
        if let Some(own) = &self.own
            && call.is_some_and(|call| call.joined_elsewhere(&own.user, &own.device))
        {
            return Some(RingEnd::AnsweredElsewhere);
        }
        let caller_in = call.is_some_and(|call| incoming.ring.caller_in(call));
        match (incoming.answering, caller_in) {
            (Answering::Pending, true) => {
                tracing::debug!(%room_id, ring = %incoming.ring.id, "the caller is in the call, so it rings");
                incoming.answering = Answering::Showing;
                None
            }
            (Answering::Showing | Answering::Silenced, false) => Some(RingEnd::CallerLeft),
            _ => None,
        }
    }

    pub(in crate::app) fn ring_window(
        &mut self,
        ring: &RingId,
        deadline: RingDeadline,
        directory: &RoomDirectory,
    ) {
        let Some(incoming) = &self.incoming else {
            return;
        };
        if incoming.ring.id != *ring {
            return;
        }
        match deadline {
            RingDeadline::Unconfirmed if incoming.answering == Answering::Pending => {
                self.stop(RingEnd::NeverJoined);
            }
            RingDeadline::Unconfirmed => return,
            RingDeadline::Expired => self.stop(RingEnd::Expired),
        }
        self.publish(directory);
    }

    pub(super) fn decline(&mut self, room_id: &RoomId, directory: &RoomDirectory) {
        let Some(incoming) = self
            .incoming
            .as_ref()
            .filter(|incoming| incoming.ring.room_id == *room_id)
        else {
            tracing::debug!(%room_id, "nothing rings here to decline");
            return;
        };
        let ring = incoming.ring.id.clone();
        if let Some(own) = &self.own {
            let port = Arc::clone(&own.port);
            let events = self.events.clone();
            let room_id = room_id.clone();
            self.actions.spawn(async move {
                let sent = match port.decline(&room_id, &ring).await {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!(%room_id, "the call could not be declined: {e}");
                        false
                    }
                };
                send(&events, CallEvent::Declined { room_id, sent });
            });
        }
        self.stop(RingEnd::Declined);
        self.publish(directory);
    }

    pub(in crate::app) fn decline_settled(
        &self,
        room_id: &RoomId,
        sent: bool,
        directory: &RoomDirectory,
    ) {
        if !sent {
            self.toast_about(UserMessageKind::CallDeclineFailed, room_id, directory);
        }
    }

    pub(super) fn silence(&mut self, directory: &RoomDirectory) {
        let Some(incoming) = self
            .incoming
            .as_mut()
            .filter(|incoming| incoming.answering == Answering::Showing)
        else {
            return;
        };
        tracing::debug!(room_id = %incoming.ring.room_id, "the ring was silenced");
        incoming.answering = Answering::Silenced;
        self.publish(directory);
    }

    pub(super) fn ring_choosing(&self) -> bool {
        self.incoming
            .as_ref()
            .is_some_and(|incoming| incoming.choosing)
    }

    pub(super) fn ring_devices(&mut self, open: bool, directory: &RoomDirectory) {
        let choices = self.devices.clone();
        let Some(incoming) = self.incoming.as_mut() else {
            tracing::debug!("nothing rings to choose devices for");
            return;
        };
        tracing::debug!(open, "choosing devices for the call that rings");
        incoming.choosing = open;
        incoming.devices.get_or_insert(choices);
        if open && self.joined.is_none() {
            self.list_devices();
        }
        self.publish(directory);
    }

    fn ring_choices(&self) -> Option<Devices> {
        let mut devices = Devices::new(self.incoming.as_ref()?.devices.clone()?);
        match &self.joined {
            Some(joined) => devices.listed(joined.device_lists()),
            None => devices.listed(self.listed.clone()?),
        }
        Some(devices)
    }

    pub(super) fn choose_for_ring(
        &mut self,
        kind: DeviceKind,
        key: Option<&DeviceKey>,
        directory: &RoomDirectory,
    ) {
        let Some(choice) = self
            .ring_choices()
            .and_then(|devices| devices.choice_for(kind, key))
        else {
            tracing::debug!(%kind, "no device of that key to choose for the call that rings");
            return;
        };
        tracing::info!(%kind, device = ?choice.key(), "choosing a device for the call that rings");
        if let Some(draft) = self
            .incoming
            .as_mut()
            .and_then(|incoming| incoming.devices.as_mut())
        {
            *draft.of_mut(kind) = choice;
        }
        self.publish(directory);
    }

    pub(super) fn announced(&mut self, ring: Option<RingId>, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let call = joined.call;
        if joined.announced(ring) {
            let events = self.events.clone();
            let token = self.key_tasks.token();
            self.key_tasks.spawn(async move {
                if token
                    .run_until_cancelled(sleep(RING_OUT_WAIT))
                    .await
                    .is_some()
                {
                    send(&events, CallEvent::RingOutTimeout { call });
                }
            });
        }
        self.publish(directory);
    }

    pub(super) fn ring_out_timeout(&mut self, directory: &RoomDirectory) {
        let Some(joined) = self
            .joined
            .as_ref()
            .filter(|joined| joined.is_ringing_out())
        else {
            return;
        };
        tracing::info!(call = joined.call, "nobody answered the call you started");
        let detail = directory.room(&joined.room_id).map_or_else(
            || joined.room_id.to_string(),
            |room| room.display_name.clone(),
        );
        self.end_call(
            Some(Ended {
                kind: UserMessageKind::CallNoAnswer,
                detail,
            }),
            directory,
        );
    }

    pub(in crate::app) fn rings_in(&self, room_id: &RoomId) -> bool {
        self.incoming
            .as_ref()
            .is_some_and(|incoming| incoming.ring.room_id == *room_id)
    }

    fn arm_windows(&mut self, ring: &Ring) {
        let expires_in = Duration::from_millis(ring.expires_ms.saturating_sub(unix_now_ms()));
        for (wait, deadline) in [
            (RING_CONFIRM_WAIT, RingDeadline::Unconfirmed),
            (expires_in, RingDeadline::Expired),
        ] {
            let events = self.events.clone();
            let ring = ring.id.clone();
            let token = self.rings.token();
            self.rings.spawn(async move {
                if token.run_until_cancelled(sleep(wait)).await.is_none() {
                    return;
                }
                send(&events, CallEvent::RingWindow { ring, deadline });
            });
        }
    }

    pub(super) fn ring_view(&self, directory: &RoomDirectory) -> Option<RingView> {
        let incoming = self
            .incoming
            .as_ref()
            .filter(|incoming| incoming.answering == Answering::Showing)?;
        let room = directory.room(&incoming.ring.room_id)?;
        let offer = self.offer_for(room);
        let quiet = room.muted();
        Some(RingView {
            serial: incoming.serial,
            room_id: room.id.clone(),
            room_name: room.display_name.clone(),
            room_avatar_mxc: room.avatar_mxc.clone(),
            direct: room.is_direct,
            caller: incoming.ring.caller.clone(),
            video: incoming.ring.is_video(),
            quiet,
            desktop: !quiet && self.focus == WindowFocus::Unfocused,
            answerable: matches!(offer, JoinOffer::Join | JoinOffer::Start),
            refusal: match offer {
                JoinOffer::Refused(refusal) => Some(refusal),
                JoinOffer::Start | JoinOffer::Join | JoinOffer::InCall => None,
            },
            devices: if incoming.choosing {
                self.ring_choices().map(|devices| devices.view())
            } else {
                None
            },
        })
    }
}
