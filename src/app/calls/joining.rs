use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};
use tracing::Instrument;

use super::joined::{Joined, Joining};
use super::lane::{LEAVE_DELAY, Lane, MEMBERSHIP_LIFETIME, keep_membership};
use super::media::MediaDriver;
use super::offer::Transports;
use super::{Blocked, Calls, Failed, JoinRequest, RingEnd, send};
use crate::app::event::CallEvent;
use crate::app::room_directory::RoomDirectory;
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{CallFailure, FailedCall, JoinOffer};
use crate::domain::call::media::{MediaChoice, MediaSetup};
use crate::domain::call::ring::RingKind;
use crate::domain::call::{CallIntent, OwnMembership, Transport, TransportLookup};
use crate::domain::room::{Room, RoomId};

impl Calls {
    pub(super) fn discover(&mut self) {
        let Some(own) = &self.own else {
            return;
        };
        if self.discovering {
            return;
        }
        self.discovering = true;
        let rtc = Arc::clone(&own.rtc);
        let session = own.session;
        let events = self.events.clone();
        self.actions.spawn(async move {
            let found = match rtc.own_transports().await {
                Ok(lookup) => Some(lookup),
                Err(e) => {
                    tracing::warn!("could not learn this homeserver's call server: {e}");
                    None
                }
            };
            send(&events, CallEvent::Transports { session, found });
        });
    }

    pub(in crate::app) fn transports_found(
        &mut self,
        found: Option<TransportLookup>,
        directory: &RoomDirectory,
    ) {
        self.discovering = false;
        match found {
            Some(TransportLookup::Found(found)) if !found.is_empty() => {
                tracing::info!(count = found.len(), "found this homeserver's call server");
                self.transports = Transports::Found(found);
            }
            Some(TransportLookup::Found(_) | TransportLookup::NoneOffered) => {
                tracing::info!("this homeserver offers no call server, so calls cannot be joined");
                self.transports = Transports::Missing;
            }
            Some(TransportLookup::Refused) => {
                tracing::info!(
                    "this homeserver refuses you its call server, so calls cannot be joined"
                );
                self.transports = Transports::Refused;
            }
            None => {}
        }
        self.settle_pending_join(directory);
        self.publish(directory);
    }

    fn settle_pending_join(&mut self, directory: &RoomDirectory) {
        let Some(request) = self.pending_join.take() else {
            return;
        };
        if self.transports.first().is_some() {
            self.join(request, directory);
        } else if self.joined.is_some() {
            self.toast_about(
                UserMessageKind::CallServerUnreachable,
                &request.room_id,
                directory,
            );
        } else {
            let failure = if self.transports == Transports::Refused {
                CallFailure::ServerRefused
            } else {
                CallFailure::ServerUnreachable
            };
            tracing::info!(room_id = %request.room_id, ?failure, "the call could not start without a call server");
            self.failed = Some(Failed {
                room_id: request.room_id,
                intent: request.intent,
                media: request.media,
                failure,
            });
        }
    }

    pub(super) fn follow_failed(&mut self, directory: &RoomDirectory) {
        let Some(failed) = &self.failed else {
            return;
        };
        if directory.room(&failed.room_id).is_none() {
            tracing::debug!(room_id = %failed.room_id, "the room of the failed call left the list");
            self.failed = None;
        }
    }

    pub(super) fn clear_stale_memberships(&mut self, directory: &RoomDirectory) {
        if self.stale_checked || !directory.rooms_landed() {
            return;
        }
        let Some(own) = &self.own else {
            return;
        };
        self.stale_checked = true;
        let me = own.member();
        let joined_room = self.joined.as_ref().map(|joined| joined.room_id.clone());
        for room in directory.rooms() {
            let stale = room
                .call
                .as_ref()
                .is_some_and(|call| call.member(&me).is_some());
            if !stale || joined_room.as_ref() == Some(&room.id) {
                continue;
            }
            tracing::info!(room_id = %room.id, "clearing a call membership this device left behind");
            let rtc = Arc::clone(&own.rtc);
            let room_id = room.id.clone();
            self.actions.spawn(async move {
                if let Err(e) = rtc.clear_membership(&room_id).await {
                    tracing::warn!(%room_id, "could not clear a stale call membership: {e}");
                }
            });
        }
    }

    pub(super) fn start_call(
        &mut self,
        room_id: RoomId,
        video: bool,
        media: MediaChoice,
        leaving: Option<RoomId>,
        directory: &RoomDirectory,
    ) {
        let intent = if video {
            CallIntent::Video
        } else {
            CallIntent::Voice
        };
        self.join(
            JoinRequest {
                room_id,
                intent,
                media,
                leaving,
            },
            directory,
        );
    }

    pub(super) fn join_call(
        &mut self,
        room_id: RoomId,
        media: MediaChoice,
        leaving: Option<RoomId>,
        directory: &RoomDirectory,
    ) {
        let Some(intent) = running_intent(directory, &room_id) else {
            tracing::info!(%room_id, "the call ended before you joined it");
            if let Some(sheet) = self.sheet.as_mut().filter(|sheet| sheet.room_id == room_id) {
                sheet.call_ended();
                self.publish(directory);
            }
            return;
        };
        self.join(
            JoinRequest {
                room_id,
                intent,
                media,
                leaving,
            },
            directory,
        );
    }

    pub(super) fn answer(
        &mut self,
        room_id: RoomId,
        camera: bool,
        leaving: Option<RoomId>,
        directory: &RoomDirectory,
    ) {
        let ringing = self
            .incoming
            .as_ref()
            .filter(|incoming| incoming.ring.room_id == room_id)
            .map(|incoming| incoming.ring.intent.unwrap_or(CallIntent::Voice));
        let Some(intent) = ringing else {
            tracing::debug!(%room_id, "nothing rings here to answer");
            return;
        };
        self.join(
            JoinRequest {
                room_id,
                intent,
                media: MediaChoice {
                    microphone: true,
                    camera,
                },
                leaving,
            },
            directory,
        );
    }

    fn admitted<'a>(
        &self,
        request: &JoinRequest,
        directory: &'a RoomDirectory,
    ) -> Result<&'a Room, Blocked> {
        let room = directory.room(&request.room_id).ok_or(Blocked::NotListed)?;
        match self.offer_for(room) {
            JoinOffer::Refused(refusal) => return Err(Blocked::Refused(refusal)),
            JoinOffer::InCall => return Err(Blocked::AlreadyIn),
            JoinOffer::Start | JoinOffer::Join => {}
        }
        match &self.joined {
            Some(joined) if request.leaving.as_ref() != Some(&joined.room_id) => {
                Err(Blocked::InAnotherCall(joined.room_id.clone()))
            }
            _ => Ok(room),
        }
    }

    fn explain_refusal(&self, blocked: &Blocked, room_id: &RoomId, directory: &RoomDirectory) {
        tracing::info!(%room_id, reason = %blocked, "this call cannot be joined");
        if matches!(blocked, Blocked::Refused(_)) {
            self.toast_about(UserMessageKind::CallUnjoinable, room_id, directory);
        }
    }

    fn wait_for_transports(&mut self, request: JoinRequest, directory: &RoomDirectory) {
        tracing::debug!(room_id = %request.room_id, "learning the call server before joining");
        self.pending_join = Some(request);
        self.discover();
        self.publish(directory);
    }

    fn join(&mut self, request: JoinRequest, directory: &RoomDirectory) {
        let room = match self.admitted(&request, directory) {
            Ok(room) => room,
            Err(blocked) => {
                self.explain_refusal(&blocked, &request.room_id, directory);
                return;
            }
        };
        self.take_chosen_devices(&request.room_id);
        self.failed = None;
        if self.rings_in(&request.room_id) {
            self.stop(RingEnd::Answered);
        }
        let Some(transport) = self.transports.first().cloned() else {
            self.wait_for_transports(request, directory);
            return;
        };
        if self.joined.is_some() {
            self.end_call(None, directory);
        }
        let Some(joining) = self.joining(room, &request, transport) else {
            return;
        };
        let members = room
            .call
            .as_ref()
            .map(|call| call.members().to_vec())
            .unwrap_or_default();
        tracing::info!(
            call = joining.call,
            room_id = %request.room_id,
            intent = ?request.intent,
            microphone = request.media.microphone,
            camera = request.media.camera,
            encrypted = room.is_encrypted,
            "joining the call"
        );
        let (joined, effects) = Joined::start(joining, &members);
        self.joined = Some(joined);
        self.run_effects(effects);
        self.publish(directory);
    }

    fn take_chosen_devices(&mut self, room_id: &RoomId) {
        if let Some(draft) = self
            .sheet
            .take()
            .filter(|sheet| sheet.room_id == *room_id)
            .and_then(|sheet| sheet.draft)
        {
            tracing::info!("the call opens on the devices chosen in its sheet");
            self.adopt_devices(draft);
        }
        if let Some(draft) = self
            .incoming
            .as_mut()
            .filter(|incoming| incoming.ring.room_id == *room_id)
            .and_then(|incoming| incoming.devices.take())
        {
            tracing::info!("the call opens on the devices chosen while it rang");
            self.adopt_devices(draft);
        }
    }

    fn joining(
        &mut self,
        room: &Room,
        request: &JoinRequest,
        transport: Transport,
    ) -> Option<Joining> {
        let own = self.own.as_ref()?;
        let media = Arc::clone(self.media.as_ref()?);
        self.generation = self.generation.wrapping_add(1);
        let call = self.generation;
        let span = tracing::debug_span!("call", room_id = %room.id, call);
        let me = own.member();
        let member_id = format!("{}:{}", own.user, own.device);
        let (media_tx, media_rx) = mpsc::unbounded_channel();
        let driver = MediaDriver {
            port: media,
            setup: MediaSetup {
                room_id: room.id.clone(),
                encrypted: room.is_encrypted,
                devices: self.devices.clone(),
            },
            events: self.events.clone(),
            call,
            turn: Arc::clone(&self.media_turn),
        };
        self.media_tasks
            .spawn(driver.run(media_rx).instrument(span.clone()));
        let (leave_tx, leave_rx) = oneshot::channel();
        let first_intent = CallIntent::of_camera(request.media.camera);
        let (intent_tx, intent_rx) = watch::channel(first_intent);
        let lane = Lane {
            port: Arc::clone(&own.rtc),
            room_id: room.id.clone(),
            membership: OwnMembership {
                device: own.device.clone(),
                member_id: member_id.clone(),
                intent: first_intent,
                transport: transport.clone(),
                created_ms: None,
                lifetime_ms: u64::try_from(MEMBERSHIP_LIFETIME.as_millis()).unwrap_or(u64::MAX),
            },
            intent: intent_rx,
            leave_delay: LEAVE_DELAY,
            events: self.events.clone(),
            call,
        };
        let token = self.lane.token();
        self.lane.spawn(
            async move {
                token
                    .run_until_cancelled(keep_membership(lane, leave_rx))
                    .await;
            }
            .instrument(span.clone()),
        );
        let alone = room
            .call
            .as_ref()
            .is_none_or(|existing| existing.others(&me).next().is_none());
        let outgoing = alone.then_some(if room.is_direct {
            RingKind::Ring
        } else {
            RingKind::Notify
        });
        Some(Joining {
            call,
            span,
            room_id: room.id.clone(),
            room_name: room.display_name.clone(),
            intent: request.intent,
            encrypted: room.is_encrypted,
            me,
            member_id,
            transport,
            leave_tx,
            intent_tx,
            media_tx,
            media: request.media,
            devices: self.devices.clone(),
            outgoing,
            follow_speaker: self.preferences.follow_speaker,
        })
    }

    pub(super) fn retry_failed(&mut self, directory: &RoomDirectory) {
        let Some(failed) = self.failed.take() else {
            tracing::debug!("no failed call to try again");
            return;
        };
        tracing::info!(room_id = %failed.room_id, "trying the failed call again");
        let intent = running_intent(directory, &failed.room_id).unwrap_or(failed.intent);
        self.join(
            JoinRequest {
                room_id: failed.room_id,
                intent,
                media: failed.media,
                leaving: None,
            },
            directory,
        );
        self.publish(directory);
    }

    pub(super) fn dismiss_failed(&mut self, directory: &RoomDirectory) {
        if self.failed.take().is_some() {
            self.publish(directory);
        }
    }

    pub(super) fn failed_view(&self, directory: &RoomDirectory) -> Option<FailedCall> {
        let failed = self.failed.as_ref()?;
        let room = directory.room(&failed.room_id)?;
        Some(FailedCall {
            room_id: room.id.clone(),
            room_name: room.display_name.clone(),
            failure: failed.failure,
            retry: matches!(self.offer_for(room), JoinOffer::Join | JoinOffer::Start),
        })
    }
}

fn running_intent(directory: &RoomDirectory, room_id: &RoomId) -> Option<CallIntent> {
    directory
        .room(room_id)
        .and_then(|room| room.call.as_deref())
        .map(|call| call.intent().unwrap_or(CallIntent::Voice))
}
