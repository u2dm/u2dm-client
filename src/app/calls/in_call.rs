use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep_until;
use tracing::Instrument;

use super::joined::{Effect, Joined};
use super::lane::LaneEvent;
use super::{Calls, Ended, Failed, people, send};
use crate::app::event::CallEvent;
use crate::app::input::EventSender;
use crate::app::room_directory::RoomDirectory;
use crate::app::show_toast;
use crate::commands::messages::UserMessage;
use crate::commands::view::{ActiveCall, CallFailure, RoomCallSummary, Toast};
use crate::domain::call::keys::ReceivedKey;
use crate::domain::call::media::MediaEvent;
use crate::domain::call::reactions::ReactionOption;
use crate::domain::call::ring::OutgoingRing;
use crate::domain::call::sfu::{GrantRequest, GrantResult, SfuId};
use crate::domain::call::{KeySendReport, MemberKey, OutgoingKey};
use crate::domain::room::RoomId;
use crate::ports::matrix::{CallSignalPort, RtcMembershipPort};
use crate::util::unix_now_ms;

impl Calls {
    pub(super) fn change_joined_in(
        &mut self,
        room_id: &RoomId,
        directory: &RoomDirectory,
        change: impl FnOnce(&mut Joined),
    ) {
        if self.joined_room() == Some(room_id) {
            self.change_joined(directory, change);
        }
    }

    pub(in crate::app) fn joined_room(&self) -> Option<&RoomId> {
        self.joined.as_ref().map(|joined| &joined.room_id)
    }

    pub(super) fn key_arrived(&mut self, room_id: &RoomId, key: ReceivedKey) {
        let Some(joined) = self
            .joined
            .as_mut()
            .filter(|joined| joined.room_id == *room_id)
        else {
            tracing::debug!(%room_id, "a call key for a call you are not in");
            return;
        };
        let span = joined.span.clone();
        let _in_call = span.enter();
        let effects = joined.received_key(key);
        self.run_effects(effects);
    }

    pub(super) fn follow_joined(&mut self, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let Some(room) = directory.room(&joined.room_id) else {
            tracing::info!(call = joined.call, room_id = %joined.room_id, "the call's room left the list, hanging up");
            self.end_call(None, directory);
            return;
        };
        let members = room
            .call
            .as_ref()
            .map(|call| call.members().to_vec())
            .unwrap_or_default();
        let effects = joined.members_changed(members);
        self.run_effects(effects);
    }

    pub(super) fn announce_again(&mut self, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let span = joined.span.clone();
        let _in_call = span.enter();
        if let Some(effect) = joined.announce_again() {
            self.run_effect(effect);
            self.publish(directory);
        }
    }

    pub(in crate::app) fn left_room(&mut self, room_id: &RoomId, directory: &RoomDirectory) {
        if self
            .joined
            .as_ref()
            .is_some_and(|joined| joined.room_id == *room_id)
        {
            self.end_call(None, directory);
        }
    }

    pub(super) fn change_joined(
        &mut self,
        directory: &RoomDirectory,
        change: impl FnOnce(&mut Joined),
    ) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let span = joined.span.clone();
        let _in_call = span.enter();
        change(joined);
        self.publish(directory);
    }

    pub(super) fn change_joined_effects(
        &mut self,
        directory: &RoomDirectory,
        change: impl FnOnce(&mut Joined) -> Vec<Effect>,
    ) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        let span = joined.span.clone();
        let _in_call = span.enter();
        let effects = change(joined);
        self.run_effects(effects);
        self.publish(directory);
    }

    pub(in crate::app) fn hang_up(&mut self) {
        if let Some(mut joined) = self.joined.take() {
            tracing::info!(call = joined.call, room_id = %joined.room_id, "hanging up");
            joined.order_farewell();
            self.key_tasks.cancel_and_detach();
            self.stage_wake = None;
            self.reaction_wake = None;
        }
    }

    pub(super) fn end_call(&mut self, ended: Option<Ended>, directory: &RoomDirectory) {
        let Some(mut joined) = self.joined.take() else {
            return;
        };
        tracing::info!(call = joined.call, room_id = %joined.room_id, "leaving the call");
        joined.order_farewell();
        self.key_tasks.cancel_and_detach();
        self.stage_wake = None;
        self.reaction_wake = None;
        if let Some(draft) = self.sheet.as_mut().and_then(|sheet| sheet.draft.take()) {
            tracing::info!("the call sheet's devices are chosen, now that no call runs");
            self.adopt_devices(draft);
            self.list_devices();
        }
        if let Some(ended) = ended {
            show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::about(ended.kind, &ended.detail)),
            );
        }
        drop(joined);
        self.publish(directory);
    }

    fn fail(&mut self, failure: CallFailure, directory: &RoomDirectory) {
        let Some(joined) = &self.joined else {
            return;
        };
        tracing::info!(call = joined.call, room_id = %joined.room_id, ?failure, "the call failed");
        self.failed = Some(Failed {
            room_id: joined.room_id.clone(),
            intent: joined.intent(),
            media: joined.media_sent(),
            failure,
        });
        self.end_call(None, directory);
    }

    pub(in crate::app) fn call_event(&mut self, event: CallEvent, directory: &RoomDirectory) {
        let span = self.call_span();
        let _in_call = span.enter();
        match event {
            CallEvent::Lane { event, .. } => self.lane_event(event, directory),
            CallEvent::Media { event, .. } => self.media_event(event, directory),
            CallEvent::MediaFailed { why, .. } => {
                tracing::warn!("the call's media did not start: {why}");
                self.fail(CallFailure::MediaFailed, directory);
            }
            CallEvent::Granted { sfu, outcome, .. } => self.granted(&sfu, *outcome, directory),
            CallEvent::Announced { ring, .. } => self.announced(ring, directory),
            CallEvent::RingOutTimeout { .. } => self.ring_out_timeout(directory),
            CallEvent::KeyWake { .. } => {
                if let Some(joined) = self.joined.as_mut() {
                    let effects = joined.key_wake();
                    self.run_effects(effects);
                }
            }
            CallEvent::StageWake { .. } => {
                self.stage_wake = None;
                self.publish(directory);
            }
            CallEvent::ReactionWake { .. } => {
                self.reaction_wake = None;
                self.publish(directory);
            }
            CallEvent::KeysSent { report, .. } => keys_sent(&report),
            CallEvent::Standings { standings, .. } => {
                self.change_joined(directory, |joined| joined.standings_known(standings));
            }
            CallEvent::HandSent {
                membership_event,
                outcome,
                ..
            } => self.change_joined_effects(directory, |joined| {
                joined.hand_sent(membership_event, outcome)
            }),
            CallEvent::HandLowered { reaction, ok, .. } => {
                self.change_joined(directory, |joined| joined.hand_lowered(&reaction, ok));
            }
            CallEvent::HandsFetched { sightings, .. } => {
                self.change_joined(directory, |joined| joined.hands_fetched(sightings));
            }
            CallEvent::ReactionSent {
                membership_event,
                reaction,
                outcome,
                ..
            } => self.change_joined(directory, |joined| {
                joined.reaction_sent(&membership_event, reaction, outcome);
            }),
            CallEvent::Signal { .. }
            | CallEvent::RingWindow { .. }
            | CallEvent::Declined { .. }
            | CallEvent::Transports { .. }
            | CallEvent::FacesReady { .. }
            | CallEvent::DevicesListed { .. }
            | CallEvent::Preview { .. }
            | CallEvent::DeviceTest { .. } => {}
        }
    }

    fn lane_event(&mut self, event: LaneEvent, directory: &RoomDirectory) {
        match event {
            LaneEvent::Joined(sent) => {
                self.change_joined_effects(directory, |joined| joined.lane_joined(sent));
            }
            LaneEvent::Failed(why) => {
                tracing::warn!("the call membership failed: {why}");
                self.fail(CallFailure::JoinFailed, directory);
            }
        }
    }

    fn media_event(&mut self, event: MediaEvent, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        if let Err(failure) = joined.media_event(event) {
            self.fail(failure, directory);
            return;
        }
        self.publish(directory);
    }

    fn granted(&mut self, sfu: &SfuId, outcome: GrantResult, directory: &RoomDirectory) {
        let Some(joined) = self.joined.as_mut() else {
            return;
        };
        if let Err(failure) = joined.granted(sfu, outcome) {
            self.fail(failure, directory);
            return;
        }
        self.publish(directory);
    }

    pub(super) fn run_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            self.run_effect(effect);
        }
    }

    fn run_effect(&mut self, effect: Effect) {
        let (Some(joined), Some(own)) = (&self.joined, &self.own) else {
            return;
        };
        let span = joined.span.clone();
        let context = EffectContext {
            port: Arc::clone(&own.port),
            rtc: Arc::clone(&own.rtc),
            room_id: joined.room_id.clone(),
            call: joined.call,
            events: self.events.clone(),
        };
        match effect {
            Effect::SendKey { to, key } => {
                self.key_tasks
                    .spawn(context.send_key(to, key).instrument(span));
            }
            Effect::WakeKeys(at) => self.wake_at(at, |call| CallEvent::KeyWake { call }),
            Effect::Grant(request) => self.actions.spawn(context.grant(*request).instrument(span)),
            Effect::Announce {
                membership_event_id,
                kind,
            } => {
                let ring = OutgoingRing {
                    membership_event_id,
                    kind,
                    intent: joined.intent(),
                    sent_ms: unix_now_ms(),
                };
                self.actions.spawn(context.announce(ring).instrument(span));
            }
            Effect::Standings(members) => {
                self.actions
                    .spawn(context.standings(members).instrument(span));
            }
            Effect::RaiseHand { membership_event } => {
                self.actions
                    .spawn(context.raise_hand(membership_event).instrument(span));
            }
            Effect::LowerHand { reaction } => {
                self.actions
                    .spawn(context.lower_hand(reaction).instrument(span));
            }
            Effect::FetchHands(memberships) => {
                self.actions
                    .spawn(context.fetch_hands(memberships).instrument(span));
            }
            Effect::SendReaction {
                membership_event,
                reaction,
            } => self
                .actions
                .spawn(context.react(membership_event, reaction).instrument(span)),
        }
    }

    pub(super) fn wake_stage(&mut self, at: Duration) {
        if self.joined.is_none() || self.stage_wake == Some(at) {
            return;
        }
        self.stage_wake = Some(at);
        self.wake_at(at, |call| CallEvent::StageWake { call });
    }

    pub(super) fn wake_reactions(&mut self, at: Duration) {
        if self.joined.is_none() || self.reaction_wake == Some(at) {
            return;
        }
        self.reaction_wake = Some(at);
        self.wake_at(at, |call| CallEvent::ReactionWake { call });
    }

    fn wake_at(&mut self, at: Duration, event: fn(u64) -> CallEvent) {
        let Some(joined) = self.joined.as_ref() else {
            return;
        };
        let call = joined.call;
        let wake = joined.wake_at(at);
        let events = self.events.clone();
        let token = self.key_tasks.token();
        self.key_tasks.spawn(async move {
            if token.run_until_cancelled(sleep_until(wake)).await.is_some() {
                send(&events, event(call));
            }
        });
    }

    pub(super) fn fetch_faces(
        &mut self,
        active: Option<&ActiveCall>,
        here: Option<&RoomCallSummary>,
    ) {
        let Some((port, session)) = self
            .own
            .as_ref()
            .map(|own| (Arc::clone(&own.port), own.session))
        else {
            return;
        };
        let in_call = active.into_iter().flat_map(|active| {
            active
                .stage
                .iter()
                .chain(active.tiles.iter())
                .chain(active.compact_self.iter())
                .map(|tile| tile.avatar_mxc.as_ref())
                .chain(
                    active
                        .people
                        .iter()
                        .map(|person| person.avatar_mxc.as_ref()),
                )
        });
        let in_room = here
            .into_iter()
            .flat_map(|here| here.faces.iter().map(|face| face.avatar_mxc.as_ref()));
        let fresh: Vec<String> = in_call
            .chain(in_room)
            .flatten()
            .filter(|mxc| self.faces_asked.insert((*mxc).clone()))
            .cloned()
            .collect();
        if fresh.is_empty() {
            return;
        }
        tracing::debug!(avatars = fresh.len(), "fetching the call's avatars");
        let events = self.events.clone();
        self.actions.spawn(async move {
            let ready = port.fetch_avatars(&fresh).await;
            send(&events, CallEvent::FacesReady { session, ready });
        });
    }

    pub(in crate::app) fn faces_fetched(&mut self, ready: usize, directory: &RoomDirectory) {
        if ready > 0 {
            self.faces_ready = self.faces_ready.saturating_add(1);
            self.publish(directory);
        }
    }

    pub(super) fn here(&self, directory: &RoomDirectory) -> Option<RoomCallSummary> {
        let room = directory.room(self.selected.as_ref()?)?;
        let call = room.call.as_deref()?;
        let me = self.own.as_ref().map(|own| &*own.user);
        Some(RoomCallSummary {
            room_id: room.id.clone(),
            people: call.people(),
            faces: people::faces(call, me).into(),
            faces_ready: self.faces_ready,
            video: call.is_video(),
            joined_by_you: me.is_some_and(|me| call.joined_by(me)),
        })
    }
}

struct EffectContext {
    port: Arc<dyn CallSignalPort>,
    rtc: Arc<dyn RtcMembershipPort>,
    room_id: RoomId,
    call: u64,
    events: EventSender,
}

impl EffectContext {
    async fn send_key(self, to: Vec<MemberKey>, key: OutgoingKey) {
        let report = self.port.send_key(&self.room_id, &key, &to).await;
        send(
            &self.events,
            CallEvent::KeysSent {
                call: self.call,
                report,
            },
        );
    }

    async fn grant(self, request: GrantRequest) {
        let outcome = self.rtc.sfu_grant(&self.room_id, &request).await;
        let sfu = request.target.id.clone();
        send(
            &self.events,
            CallEvent::Granted {
                call: self.call,
                sfu,
                outcome: Box::new(outcome),
            },
        );
    }

    async fn announce(self, ring: OutgoingRing) {
        let room_id = &self.room_id;
        let kind = ring.kind;
        let ring = match self.port.announce_call(room_id, &ring).await {
            Ok(ring) => {
                tracing::info!(%room_id, ?kind, "announced the call");
                Some(ring)
            }
            Err(e) => {
                tracing::warn!(%room_id, "the call was not announced: {e}");
                None
            }
        };
        send(
            &self.events,
            CallEvent::Announced {
                call: self.call,
                ring,
            },
        );
    }

    async fn standings(self, members: Vec<MemberKey>) {
        let standings = self.port.device_standing(&members).await;
        send(
            &self.events,
            CallEvent::Standings {
                call: self.call,
                standings,
            },
        );
    }

    async fn raise_hand(self, membership_event: String) {
        let outcome = self
            .port
            .raise_hand(&self.room_id, &membership_event)
            .await
            .map_err(|e| e.to_string());
        send(
            &self.events,
            CallEvent::HandSent {
                call: self.call,
                membership_event,
                outcome,
            },
        );
    }

    async fn lower_hand(self, reaction: String) {
        let room_id = &self.room_id;
        let lowered = self.port.lower_hand(room_id, &reaction).await;
        if let Err(e) = &lowered {
            tracing::warn!(%room_id, "a raised hand was not lowered: {e}");
        }
        send(
            &self.events,
            CallEvent::HandLowered {
                call: self.call,
                reaction,
                ok: lowered.is_ok(),
            },
        );
    }

    async fn fetch_hands(self, memberships: Vec<String>) {
        let room_id = &self.room_id;
        let sightings = self.port.raised_hands(room_id, &memberships).await;
        tracing::debug!(%room_id, asked = memberships.len(), found = sightings.len(), "asked which hands are raised");
        send(
            &self.events,
            CallEvent::HandsFetched {
                call: self.call,
                sightings,
            },
        );
    }

    async fn react(self, membership_event: String, reaction: ReactionOption) {
        let outcome = self
            .port
            .send_reaction(&self.room_id, &membership_event, reaction)
            .await
            .map_err(|e| e.to_string());
        send(
            &self.events,
            CallEvent::ReactionSent {
                call: self.call,
                membership_event,
                reaction,
                outcome,
            },
        );
    }
}

fn keys_sent(report: &KeySendReport) {
    if report.withheld.is_empty() {
        tracing::debug!(sent = report.sent.len(), "sent the call key");
        return;
    }
    let withheld: Vec<String> = report.withheld.iter().map(ToString::to_string).collect();
    tracing::info!(
        sent = report.sent.len(),
        ?withheld,
        "the call key did not reach every device in the call"
    );
}
