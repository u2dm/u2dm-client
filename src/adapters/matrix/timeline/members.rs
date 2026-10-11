use std::collections::{HashMap, HashSet};
use std::sync::Mutex as StdMutex;

use matrix_sdk::room::RoomMember;
use matrix_sdk::ruma::OwnedUserId;
use matrix_sdk::{Client, Room};
use tokio::sync::mpsc;

use crate::adapters::matrix::media::{MediaService, mxc_avatar_key};
use crate::adapters::matrix::member_name::member_name;
use crate::domain::member_name::{MemberName, NamedUser};

pub(super) enum Lookup<T> {
    Found(T),
    Missing,
    Unresolved,
}

#[derive(Clone, Copy)]
pub(super) enum Need {
    Avatar,
    Name,
}

pub(super) enum Batch {
    Avatars(Vec<(String, Lookup<String>)>),
    Names(Vec<(String, Lookup<MemberName>)>),
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

struct Store<T> {
    known: HashMap<String, Option<T>>,
    wanted: HashSet<String>,
    resolving: HashSet<String>,
}

impl<T> Default for Store<T> {
    fn default() -> Self {
        Self {
            known: HashMap::new(),
            wanted: HashSet::new(),
            resolving: HashSet::new(),
        }
    }
}

impl<T> Store<T> {
    fn want(&mut self, user_id: &str) {
        if !self.known.contains_key(user_id) && !self.resolving.contains(user_id) {
            self.wanted.insert(user_id.to_owned());
        }
    }
}

pub(in crate::adapters::matrix) struct MemberCache<T> {
    store: StdMutex<Store<T>>,
}

impl<T> Default for MemberCache<T> {
    fn default() -> Self {
        Self {
            store: StdMutex::new(Store::default()),
        }
    }
}

impl<T: Clone> MemberCache<T> {
    pub(super) fn get(&self, user_id: &str) -> Option<T> {
        self.store
            .lock()
            .ok()?
            .known
            .get(user_id)
            .cloned()
            .flatten()
    }

    pub(super) fn want(&self, user_id: &str) {
        if let Ok(mut store) = self.store.lock() {
            store.want(user_id);
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

    fn record(&self, lookups: Vec<(String, Lookup<T>)>) -> HashSet<String> {
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

const NO_NAME: MemberName = MemberName::Unset;

impl MemberCache<MemberName> {
    pub(super) fn name_of(&self, user_id: &str) -> MemberName {
        self.get(user_id).unwrap_or_else(|| {
            self.want(user_id);
            MemberName::Unset
        })
    }

    pub(super) fn confirm(&self, user_id: &str, reported: MemberName) -> MemberName {
        let MemberName::Unique(name) = &reported else {
            return reported;
        };
        let Ok(mut store) = self.store.lock() else {
            return reported;
        };
        match store.known.get(user_id) {
            Some(Some(MemberName::Shared(shared))) if shared == name => {
                MemberName::Shared(shared.clone())
            }
            Some(_) => reported,
            None => {
                store.want(user_id);
                reported
            }
        }
    }

    pub(super) fn name_all(&self, users: &mut [NamedUser]) -> bool {
        let Ok(mut store) = self.store.lock() else {
            return false;
        };
        let mut renamed = false;
        for user in users {
            let current = if let Some(known) = store.known.get(&user.user_id) {
                known.as_ref().unwrap_or(&NO_NAME)
            } else {
                store.want(&user.user_id);
                &NO_NAME
            };
            if user.name != *current {
                user.name = current.clone();
                renamed = true;
            }
        }
        renamed
    }
}

#[derive(Default)]
pub(in crate::adapters::matrix) struct Members {
    pub(super) avatars: MemberCache<String>,
    pub(super) names: MemberCache<MemberName>,
}

impl Members {
    pub(super) fn take_wanted(&self) -> Wanted {
        Wanted {
            avatars: self.avatars.take_wanted(),
            names: self.names.take_wanted(),
        }
    }

    pub(super) fn record(&self, batch: Batch) -> Arrived {
        match batch {
            Batch::Avatars(lookups) => Arrived {
                need: Need::Avatar,
                users: self.avatars.record(lookups),
            },
            Batch::Names(lookups) => Arrived {
                need: Need::Name,
                users: self.names.record(lookups),
            },
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
        if batches.send(Batch::Names(lookups)).await.is_err() {
            return;
        }
    }
    if !wanted.avatars.is_empty() {
        let mut lookups = Vec::with_capacity(wanted.avatars.len());
        for user_id in wanted.avatars {
            let lookup = resolve_avatar(room, client, media, &user_id).await;
            lookups.push((user_id, lookup));
        }
        drop(batches.send(Batch::Avatars(lookups)).await);
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

async fn resolve_name(room: &Room, user_id: &str) -> Lookup<MemberName> {
    match member(room, user_id).await {
        Member::Found(member) => match member_name(room, &member).await {
            MemberName::Unset => Lookup::Missing,
            name => Lookup::Found(name),
        },
        Member::Absent => Lookup::Missing,
        Member::Unreadable => Lookup::Unresolved,
    }
}

async fn resolve_avatar(
    room: &Room,
    client: &Client,
    media: &MediaService,
    user_id: &str,
) -> Lookup<String> {
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
