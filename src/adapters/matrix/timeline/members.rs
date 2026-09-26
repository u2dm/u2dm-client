use std::collections::{HashMap, HashSet};
use std::sync::Mutex as StdMutex;

use matrix_sdk::room::RoomMember;
use matrix_sdk::ruma::OwnedUserId;
use matrix_sdk::{Client, Room};
use tokio::sync::mpsc;

use crate::adapters::matrix::media::{MediaService, mxc_avatar_key};

pub(super) enum Lookup {
    Found(String),
    Missing,
    Unresolved,
}

#[derive(Clone, Copy)]
pub(super) enum Need {
    Avatar,
    Name,
}

pub(super) struct Batch {
    need: Need,
    lookups: Vec<(String, Lookup)>,
}

pub(super) struct Arrived {
    pub(super) need: Need,
    pub(super) users: HashSet<String>,
}

pub(super) struct Wanted {
    avatars: Vec<String>,
    names: Vec<String>,
}

impl Wanted {
    pub(super) fn is_empty(&self) -> bool {
        self.avatars.is_empty() && self.names.is_empty()
    }
}

#[derive(Default)]
struct Store {
    known: HashMap<String, Option<String>>,
    wanted: HashSet<String>,
    resolving: HashSet<String>,
}

#[derive(Default)]
pub(in crate::adapters::matrix) struct MemberCache {
    store: StdMutex<Store>,
}

impl MemberCache {
    pub(super) fn get(&self, user_id: &str) -> Option<String> {
        self.store
            .lock()
            .ok()?
            .known
            .get(user_id)
            .cloned()
            .flatten()
    }

    pub(super) fn want(&self, user_id: &str) {
        if let Ok(mut store) = self.store.lock()
            && !store.known.contains_key(user_id)
            && !store.resolving.contains(user_id)
        {
            store.wanted.insert(user_id.to_owned());
        }
    }

    fn take_wanted(&self) -> Vec<String> {
        let Ok(mut store) = self.store.lock() else {
            return Vec::new();
        };
        let wanted: Vec<String> = store.wanted.drain().collect();
        store.resolving.extend(wanted.iter().cloned());
        wanted
    }

    fn record(&self, lookups: Vec<(String, Lookup)>) -> HashSet<String> {
        let Ok(mut store) = self.store.lock() else {
            return HashSet::new();
        };
        let mut arrived = HashSet::new();
        for (user_id, lookup) in lookups {
            store.resolving.remove(&user_id);
            match lookup {
                Lookup::Found(value) => {
                    store.known.insert(user_id.clone(), Some(value));
                    arrived.insert(user_id);
                }
                Lookup::Missing => {
                    store.known.insert(user_id, None);
                }
                Lookup::Unresolved => {}
            }
        }
        arrived
    }
}

#[derive(Default)]
pub(in crate::adapters::matrix) struct Members {
    pub(super) avatars: MemberCache,
    pub(super) names: MemberCache,
}

impl Members {
    pub(super) fn take_wanted(&self) -> Wanted {
        Wanted {
            avatars: self.avatars.take_wanted(),
            names: self.names.take_wanted(),
        }
    }

    pub(super) fn record(&self, batch: Batch) -> Arrived {
        let cache = match batch.need {
            Need::Avatar => &self.avatars,
            Need::Name => &self.names,
        };
        Arrived {
            need: batch.need,
            users: cache.record(batch.lookups),
        }
    }
}

pub(super) async fn resolve_members(
    room: &Room,
    client: &Client,
    media: &MediaService,
    wanted: Wanted,
    batches: &mpsc::Sender<Batch>,
) {
    if !wanted.names.is_empty() {
        let mut lookups = Vec::with_capacity(wanted.names.len());
        for user_id in wanted.names {
            let lookup = resolve_name(room, &user_id).await;
            lookups.push((user_id, lookup));
        }
        let batch = Batch {
            need: Need::Name,
            lookups,
        };
        if batches.send(batch).await.is_err() {
            return;
        }
    }
    if !wanted.avatars.is_empty() {
        let mut lookups = Vec::with_capacity(wanted.avatars.len());
        for user_id in wanted.avatars {
            let lookup = resolve_avatar(room, client, media, &user_id).await;
            lookups.push((user_id, lookup));
        }
        let batch = Batch {
            need: Need::Avatar,
            lookups,
        };
        drop(batches.send(batch).await);
    }
}

enum Member {
    Found(RoomMember),
    Absent,
    Unreadable,
}

async fn member(room: &Room, user_id: &str) -> Member {
    let Ok(parsed) = OwnedUserId::try_from(user_id) else {
        return Member::Absent;
    };
    match room.get_member(&parsed).await {
        Ok(Some(member)) => Member::Found(member),
        Ok(None) => Member::Absent,
        Err(error) => {
            tracing::debug!(%error, user_id, "could not read a room member");
            Member::Unreadable
        }
    }
}

async fn resolve_name(room: &Room, user_id: &str) -> Lookup {
    match member(room, user_id).await {
        Member::Found(member) => member
            .display_name()
            .map_or(Lookup::Missing, |name| Lookup::Found(name.to_owned())),
        Member::Absent => Lookup::Missing,
        Member::Unreadable => Lookup::Unresolved,
    }
}

async fn resolve_avatar(
    room: &Room,
    client: &Client,
    media: &MediaService,
    user_id: &str,
) -> Lookup {
    let member = match member(room, user_id).await {
        Member::Found(member) => member,
        Member::Absent => return Lookup::Missing,
        Member::Unreadable => return Lookup::Unresolved,
    };
    let Some(mxc) = member.avatar_url().map(ToString::to_string) else {
        return Lookup::Missing;
    };
    if media
        .fetch_avatar_by_mxc(client, &mxc_avatar_key(&mxc), mxc.as_str().into())
        .await
        .is_none()
    {
        return Lookup::Unresolved;
    }
    Lookup::Found(mxc)
}
