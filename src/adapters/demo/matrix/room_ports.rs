use std::sync::atomic::Ordering;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::sleep;

use super::{DemoAuthed, current_lists, unavailable, unavailable_if};
use crate::adapters::demo::{
    account, create_room, data, departures, invites, links, media, notifications, room_info,
    space_index, upgrades,
};
use crate::domain::departure::AdminStanding;
use crate::domain::matrix_link::RoomAddress;
use crate::domain::push::PushNotice;
use crate::domain::room::RoomId;
use crate::domain::room_creation::{
    ChildLink, CreateFailure, CreateResult, CreatedRoom, JoinAccess, RoomBlueprint, RoomKind, Shape,
};
use crate::domain::room_preview::{PreviewFailure, PreviewMembership, PreviewResult};
use crate::domain::space_index::HierarchyPage;
use crate::domain::user_info::server_part;
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    InvitePort, LeavePort, PushPort, RoomCreationPort, RoomPreviewPort, SpaceIndexPort,
    SpaceOrderPort, UpgradePort,
};
use crate::util::unix_now_ms;

#[async_trait]
impl RoomCreationPort for DemoAuthed {
    async fn create_room(&self, blueprint: &RoomBlueprint) -> CreateResult {
        create_room::pause().await;
        let scenario = create_room::scenario();
        if scenario.refused {
            return Err(CreateFailure::Refused);
        }
        if scenario.fails {
            return Err(CreateFailure::Failed);
        }
        let own_server = server_part(data::own_user());
        let alias = match blueprint.access() {
            JoinAccess::Public(address) if self.address_taken(address.as_str()) => {
                return Err(CreateFailure::AddressTaken);
            }
            JoinAccess::Public(address) => Some(address.alias_on(own_server)),
            JoinAccess::InviteOnly | JoinAccess::SpaceMembers(_) => None,
        };
        let serial = self
            .creations
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let room_id = RoomId::new(data::created_room_id(serial));
        let child = match blueprint.parent() {
            None => ChildLink::NotAsked,
            Some(_) if scenario.child_fails => ChildLink::Failed,
            Some(_) => ChildLink::Added,
        };
        tracing::info!(%room_id, kind = ?blueprint.kind(), ?child, "demo: creating a room");
        if scenario.echo_lost {
            return Ok(CreatedRoom { room_id, child });
        }
        let created = data::Created {
            id: room_id.to_string(),
            space: blueprint.kind() == RoomKind::Space,
            name: blueprint.name().to_owned(),
            topic: blueprint.topic().map(str::to_owned),
            alias,
            encrypted: matches!(blueprint.shape(), Shape::Room { encrypted: true }),
            at_ms: unix_now_ms(),
        };
        self.change_lists(|lists| lists.overrides.created.push(created));
        if let (ChildLink::Added, Some(parent)) = (child, blueprint.parent()) {
            if scenario.parent_late {
                self.list_child_later(parent.clone(), room_id.clone());
            } else {
                self.list_child(parent, &room_id);
            }
        }
        self.echo_join();
        Ok(CreatedRoom { room_id, child })
    }
}

#[async_trait]
impl SpaceIndexPort for DemoAuthed {
    async fn hierarchy_page(&self, space_id: &RoomId, from: Option<&str>) -> Result<HierarchyPage> {
        space_index::pause().await;
        let offset = from.and_then(|token| token.parse().ok()).unwrap_or(0);
        if space_index::page_fails_now(space_id, offset) {
            return Err(unavailable("this page of the space index"));
        }
        let children = data::space_children(space_id);
        let next = offset.saturating_add(space_index::PAGE_SIZE);
        Ok(HierarchyPage {
            next: (next < children.len()).then(|| next.to_string()),
            children: children
                .into_iter()
                .skip(offset)
                .take(space_index::PAGE_SIZE)
                .collect(),
        })
    }

    async fn join(&self, room_id: &RoomId, via: &[String]) -> Result<()> {
        space_index::pause().await;
        unavailable_if(space_index::scenario().join_fails, "joining rooms")?;
        tracing::debug!(%room_id, ?via, "demo: joining a room from the space index");
        self.remember_join(room_id);
        self.echo_join();
        Ok(())
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        space_index::pause_avatars().await;
        media::fetch_unjoined_avatars(mxcs)
    }
}

#[async_trait]
impl InvitePort for DemoAuthed {
    async fn accept(&self, room_id: &RoomId) -> Result<()> {
        invites::pause().await;
        unavailable_if(invites::scenario().accept_fails, "accepting invites")?;
        tracing::debug!(%room_id, "demo: accepting an invite");
        if invites::scenario().echo_lost {
            return Ok(());
        }
        self.remember_join(room_id);
        self.echo_join();
        Ok(())
    }

    async fn decline(&self, room_id: &RoomId) -> Result<()> {
        invites::pause().await;
        unavailable_if(invites::scenario().decline_fails, "declining invites")?;
        tracing::debug!(%room_id, "demo: declining an invite");
        self.change_lists(|lists| {
            lists.overrides.declined.insert(room_id.to_string());
        });
        self.echo_join();
        Ok(())
    }
}

#[async_trait]
impl UpgradePort for DemoAuthed {
    async fn join_successor(&self, room_id: &RoomId) -> Result<()> {
        self.join_linked(room_id, data::successor_of(room_id)).await
    }

    async fn join_predecessor(&self, room_id: &RoomId) -> Result<()> {
        self.join_linked(room_id, data::predecessor_of(room_id))
            .await
    }
}

impl DemoAuthed {
    async fn join_linked(&self, room_id: &RoomId, linked: Option<RoomId>) -> Result<()> {
        upgrades::pause().await;
        if upgrades::scenario().join_fails {
            return Err(unavailable("joining across a room upgrade"));
        }
        let Some(linked) = linked else {
            return Err(AppError::Other(format!("no upgrade links {room_id}")));
        };
        tracing::debug!(%room_id, %linked, "demo: joining the other end of a room upgrade");
        if upgrades::scenario().echo_lost {
            return Ok(());
        }
        self.remember_join(&linked);
        self.echo_join();
        Ok(())
    }
}

#[async_trait]
impl RoomPreviewPort for DemoAuthed {
    async fn preview(&self, room: &RoomAddress, via: &[String]) -> PreviewResult {
        links::pause().await;
        tracing::debug!(%room, ?via, "demo: previewing a linked room");
        if links::preview_fails_now(room.as_str()) {
            return Err(PreviewFailure::Failed);
        }
        let room_id = match room {
            RoomAddress::Id(room_id) => room_id.clone(),
            RoomAddress::Alias(alias) => {
                data::alias_target(alias).ok_or(PreviewFailure::NoSuchRoom)?
            }
        };
        let membership = self.membership_of(&room_id);
        if links::scenario().unavailable && membership == PreviewMembership::Outsider {
            return Err(PreviewFailure::Unavailable);
        }
        let mut preview = data::room_card(&room_id).ok_or(PreviewFailure::Unavailable)?;
        preview.membership = membership;
        Ok(preview)
    }

    async fn join(
        &self,
        room: &RoomAddress,
        known: Option<&RoomId>,
        via: &[String],
    ) -> Result<RoomId> {
        links::pause().await;
        unavailable_if(links::scenario().join_fails, "joining linked rooms")?;
        let room_id = match room {
            RoomAddress::Id(room_id) => Some(room_id.clone()),
            RoomAddress::Alias(alias) => data::alias_target(alias),
        }
        .or_else(|| known.cloned())
        .filter(|room_id| data::room_card(room_id).is_some())
        .ok_or_else(|| AppError::Other(format!("no demo room has the address {room}")))?;
        tracing::debug!(%room_id, ?via, "demo: joining a linked room");
        if !links::scenario().echo_lost {
            self.remember_join(&room_id);
            self.echo_join();
        }
        Ok(room_id)
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        links::pause().await;
        media::fetch_unjoined_avatars(mxcs)
    }
}

impl DemoAuthed {
    fn membership_of(&self, room_id: &RoomId) -> PreviewMembership {
        let lists = current_lists(&self.lists);
        let joined = lists.rooms().iter().any(|room| room.id == *room_id)
            || lists
                .spaces()
                .iter()
                .any(|space| space.id == room_id.as_ref());
        if joined {
            return PreviewMembership::Joined;
        }
        if lists.invites().iter().any(|invite| invite.id == *room_id) {
            return PreviewMembership::Invited;
        }
        PreviewMembership::Outsider
    }
}

#[async_trait]
impl PushPort for DemoAuthed {
    async fn follow_notifications(&self, notices_tx: mpsc::Sender<PushNotice>) -> Result<()> {
        for timed in notifications::script() {
            sleep(timed.after).await;
            let following = match timed.step {
                notifications::Step::Arrive(arrival) => self.arrive(&arrival, &notices_tx).await,
                notifications::Step::ReadElsewhere(room_id) => {
                    self.read_elsewhere(&room_id);
                    true
                }
                notifications::Step::Invite => self.release_notified_invite(&notices_tx).await,
            };
            if !following {
                break;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl LeavePort for DemoAuthed {
    async fn leave(&self, room_id: &RoomId) -> Result<()> {
        room_info::pause().await;
        departures::pause().await;
        unavailable_if(room_info::scenario().leave_fails, "leaving rooms")?;
        if departures::scenario().room_stays && room_id.as_ref() == departures::STAYING_ROOM {
            return Err(unavailable("leaving this room"));
        }
        if account::scenario().forgets_rooms {
            tracing::info!(%room_id, "demo: leaving a room; the homeserver forgets it by itself");
        } else {
            tracing::info!(%room_id, "demo: leaving and forgetting a room");
        }
        self.change_lists(|lists| {
            lists.joined.retain(|joined| joined != room_id);
            lists.overrides.left.insert(room_id.to_string());
        });
        self.echo_departure();
        Ok(())
    }

    async fn forget(&self, room_id: &RoomId) -> Result<()> {
        departures::pause().await;
        unavailable_if(departures::scenario().forget_fails, "forgetting rooms")?;
        tracing::info!(%room_id, "demo: forgetting the room you were removed from");
        self.change_lists(|lists| {
            lists.overrides.forgotten.insert(room_id.to_string());
        });
        self.echo_departure();
        Ok(())
    }

    async fn admin_standing(&self, room_id: &RoomId) -> Result<AdminStanding> {
        departures::pause().await;
        tracing::debug!(%room_id, "demo: reading who else manages the room");
        if departures::scenario().sole_admin {
            return Ok(AdminStanding::OnlyAdmin);
        }
        Ok(AdminStanding::NotOnlyAdmin)
    }
}

#[async_trait]
impl SpaceOrderPort for DemoAuthed {
    async fn set_space_order(&self, space_id: &RoomId, order: &str) -> Result<()> {
        tracing::debug!(%space_id, order, "demo: ignoring space order write");
        Ok(())
    }
}
