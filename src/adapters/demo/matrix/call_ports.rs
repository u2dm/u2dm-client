use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::sleep;

use super::{DemoAuthed, current_lists, unavailable};
use crate::adapters::demo::calls::{self, Step};
use crate::adapters::demo::{data, media, room_info};
use crate::domain::call::hands::HandSighting;
use crate::domain::call::identity::spellings;
use crate::domain::call::reactions::ReactionOption;
use crate::domain::call::ring::{CallSignal, OutgoingRing, RingId};
use crate::domain::call::sfu::{GrantFailure, GrantRequest, GrantResult, SfuGrant};
use crate::domain::call::{
    DelayedLeave, DeviceStanding, KeySendReport, MemberKey, OutgoingKey, OwnMembership,
    RestartResult, ScheduleFailure, ScheduleResult, Secret, SentMembership, TransportLookup,
};
use crate::domain::room::RoomId;
use crate::domain::sync::SyncEvent;
use crate::domain::user_info::UserId;
use crate::error::Result;
use crate::ports::matrix::{CallSignalPort, RtcMembershipPort};
use crate::util::unix_now_ms;

const GRANT_REFUSED: u16 = 403;

impl DemoAuthed {
    fn leave_call(&self, room_id: &RoomId) {
        calls::book().leave_own(room_id.as_ref());
        tracing::info!(%room_id, "demo: you left the call");
        self.echo_rooms();
    }

    fn script_answer(&self, room_id: &RoomId, ring: &RingId) {
        let shaped = calls::scenario();
        let Some(partner) = calls::partner_of(room_id.as_ref()) else {
            return;
        };
        if !(shaped.answers || shaped.declines) {
            return;
        }
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        let room_id = room_id.clone();
        let ring = ring.clone();
        tokio::spawn(async move {
            sleep(calls::ANSWER_AFTER).await;
            if shaped.answers {
                tracing::info!(%room_id, "demo: the call is answered");
                calls::answer_in(room_id.as_ref());
                sink(SyncEvent::Rooms(current_lists(&lists).rooms().into()));
                for key in calls::book().keys_from_others(room_id.as_ref()) {
                    calls::book().signal(key).await;
                }
            } else {
                tracing::info!(%room_id, "demo: the call is declined by the callee");
                calls::book()
                    .signal(CallSignal::Declined {
                        room_id,
                        ring,
                        by: UserId::new(partner),
                    })
                    .await;
            }
        });
    }

    pub(super) fn spawn_call_script(&self) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        for scripted in calls::script() {
            let sink = Arc::clone(&sink);
            let lists = Arc::clone(&self.lists);
            tokio::spawn(async move {
                sleep(scripted.after).await;
                for step in (scripted.change)(calls::book()) {
                    match step {
                        Step::Rooms => {
                            tracing::debug!("demo: a call changed");
                            sink(SyncEvent::Rooms(current_lists(&lists).rooms().into()));
                        }
                        Step::Signal(signal) => calls::book().signal(signal).await,
                    }
                }
            });
        }
    }

    fn spawn_hands(&self, room_id: &RoomId) {
        let room_id = room_id.clone();
        let sink = self.sync_sink();
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            calls::book()
                .play_hands(room_id.as_ref(), || {
                    if let Some(sink) = &sink {
                        sink(SyncEvent::Rooms(current_lists(&lists).rooms().into()));
                    }
                })
                .await;
        });
    }
}

#[async_trait]
impl CallSignalPort for DemoAuthed {
    async fn follow_signals(&self, signals_tx: mpsc::Sender<CallSignal>) -> Result<()> {
        calls::book().follow(signals_tx.clone());
        signals_tx.closed().await;
        Ok(())
    }

    async fn decline(&self, room_id: &RoomId, ring: &RingId) -> Result<()> {
        calls::pause().await;
        if calls::scenario().decline_fails {
            return Err(unavailable("declining the call"));
        }
        tracing::info!(%room_id, %ring, "demo: the call is declined");
        Ok(())
    }

    async fn announce_call(&self, room_id: &RoomId, ring: &OutgoingRing) -> Result<RingId> {
        if calls::announcement_fails() {
            tracing::info!(%room_id, kind = ?ring.kind, "demo: the call announcement fails");
            return Err(unavailable("announcing the call"));
        }
        let id = RingId::new(format!("$demo-outgoing-{}", ring.sent_ms));
        tracing::info!(%room_id, kind = ?ring.kind, membership = ring.membership_event_id, "demo: the call is announced");
        self.script_answer(room_id, &id);
        Ok(id)
    }

    async fn send_key(
        &self,
        room_id: &RoomId,
        key: &OutgoingKey,
        to: &[MemberKey],
    ) -> KeySendReport {
        tracing::info!(%room_id, index = %key.index, to = to.len(), "demo: a call key is sent");
        KeySendReport {
            sent: to.to_vec(),
            withheld: Vec::new(),
        }
    }

    async fn device_standing(&self, members: &[MemberKey]) -> Vec<(MemberKey, DeviceStanding)> {
        let untrusted = calls::scenario().undecryptable;
        members
            .iter()
            .map(|member| {
                let standing = if untrusted && member.user == calls::UNTRUSTED {
                    DeviceStanding::NotCrossSigned
                } else {
                    DeviceStanding::CrossSigned
                };
                (member.clone(), standing)
            })
            .collect()
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        room_info::pause_avatars().await;
        media::fetch_member_avatars(mxcs)
    }

    async fn raise_hand(&self, room_id: &RoomId, membership_event: &str) -> Result<String> {
        calls::pause().await;
        Ok(calls::book().raise_own_hand(room_id.as_ref(), membership_event))
    }

    async fn lower_hand(&self, room_id: &RoomId, reaction: &str) -> Result<()> {
        calls::pause().await;
        if !calls::book().lower_own_hand(room_id.as_ref(), reaction) {
            tracing::info!(%room_id, reaction, "demo: that hand was already down");
        }
        Ok(())
    }

    async fn raised_hands(&self, room_id: &RoomId, memberships: &[String]) -> Vec<HandSighting> {
        calls::pause().await;
        let found = calls::book().raised_on(room_id.as_ref(), memberships);
        tracing::info!(%room_id, asked = memberships.len(), found = found.len(), "demo: the raised hands are read");
        found
    }

    async fn send_reaction(
        &self,
        room_id: &RoomId,
        membership_event: &str,
        reaction: ReactionOption,
    ) -> Result<String> {
        calls::pause().await;
        Ok(calls::book().send_own_reaction(room_id.as_ref(), membership_event, reaction))
    }
}

#[async_trait]
impl RtcMembershipPort for DemoAuthed {
    async fn own_transports(&self) -> Result<TransportLookup> {
        calls::pause().await;
        if calls::scenario().transports_refused {
            return Ok(TransportLookup::Refused);
        }
        if calls::scenario().no_sfu {
            return Ok(TransportLookup::NoneOffered);
        }
        Ok(TransportLookup::Found(vec![calls::own_transport()]))
    }

    async fn schedule_leave(&self, room_id: &RoomId, delay: Duration) -> ScheduleResult {
        if calls::scenario().delayed_unsupported {
            return Err(ScheduleFailure::Unsupported);
        }
        tracing::info!(%room_id, delay_ms = delay.as_millis(), "demo: the call's delayed leave is scheduled");
        Ok(DelayedLeave {
            room_id: room_id.clone(),
            delay_id: Secret::new("demo-delay"),
        })
    }

    async fn restart_leave(&self, _leave: &DelayedLeave) -> RestartResult {
        Ok(())
    }

    async fn send_leave(&self, leave: &DelayedLeave) -> Result<()> {
        self.leave_call(&leave.room_id);
        Ok(())
    }

    async fn clear_membership(&self, room_id: &RoomId) -> Result<()> {
        self.leave_call(room_id);
        Ok(())
    }

    async fn send_membership(
        &self,
        room_id: &RoomId,
        membership: &OwnMembership,
    ) -> Result<SentMembership> {
        calls::pause().await;
        let first = membership.created_ms.is_none();
        calls::book().own_join(room_id.as_ref(), membership.intent);
        tracing::info!(%room_id, first, intent = ?membership.intent, "demo: you joined the call");
        self.echo_rooms();
        if first {
            spawn_others_keys(room_id);
            self.spawn_hands(room_id);
            spawn_reactions(room_id);
        }
        Ok(SentMembership {
            event_id: calls::book()
                .own_membership_event(room_id.as_ref())
                .unwrap_or_else(|| format!("$demo-membership-{room_id}")),
            created_ms: membership.created_ms.unwrap_or_else(unix_now_ms),
        })
    }

    async fn sfu_grant(&self, _room_id: &RoomId, request: &GrantRequest) -> GrantResult {
        calls::pause().await;
        if calls::scenario().grant_fails {
            return Err(GrantFailure::Refused(GRANT_REFUSED));
        }
        let user = data::own_user();
        let [legacy, _, _] = spellings(user, &request.device, &request.member_id);
        Ok(SfuGrant {
            sfu: request.target.id.clone(),
            role: request.target.role,
            url: calls::DEMO_LIVEKIT.to_owned(),
            token: Secret::new("demo-token"),
            identity: legacy,
        })
    }
}

fn spawn_others_keys(room_id: &RoomId) {
    let room_id = room_id.clone();
    let late = calls::scenario().keys_late;
    tokio::spawn(async move {
        if late {
            sleep(calls::KEYS_LATE_BY).await;
        }
        for key in calls::book().keys_from_others(room_id.as_ref()) {
            calls::book().signal(key).await;
        }
    });
}

fn spawn_reactions(room_id: &RoomId) {
    let room_id = room_id.clone();
    tokio::spawn(async move {
        calls::book().play_reactions(room_id.as_ref()).await;
    });
}
