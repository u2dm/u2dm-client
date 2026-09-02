use std::collections::{HashMap, HashSet};
use std::sync::Mutex as StdMutex;

use matrix_sdk::ruma::OwnedUserId;
use matrix_sdk::{Client, Room};

use crate::adapters::matrix::media::{MediaService, mxc_avatar_key};

pub(super) enum Resolution {
    Avatar(String),
    NoAvatar,
    Unresolved,
}

#[derive(Default)]
struct Store {
    avatars: HashMap<String, Option<String>>,
    wanted: HashSet<String>,
    resolving: HashSet<String>,
}

#[derive(Default)]
pub(in crate::adapters::matrix) struct ReactorAvatars {
    store: StdMutex<Store>,
}

impl ReactorAvatars {
    pub(super) fn avatar(&self, user_id: &str) -> Option<String> {
        self.store
            .lock()
            .ok()?
            .avatars
            .get(user_id)
            .cloned()
            .flatten()
    }

    pub(super) fn want(&self, user_id: &str) {
        if let Ok(mut store) = self.store.lock()
            && !store.avatars.contains_key(user_id)
            && !store.resolving.contains(user_id)
        {
            store.wanted.insert(user_id.to_owned());
        }
    }

    pub(super) fn take_wanted(&self) -> Vec<String> {
        let Ok(mut store) = self.store.lock() else {
            return Vec::new();
        };
        let wanted: Vec<String> = store.wanted.drain().collect();
        store.resolving.extend(wanted.iter().cloned());
        wanted
    }

    pub(super) fn record(&self, resolved: Vec<(String, Resolution)>) -> HashSet<String> {
        let Ok(mut store) = self.store.lock() else {
            return HashSet::new();
        };
        let mut arrived = HashSet::new();
        for (user_id, resolution) in resolved {
            store.resolving.remove(&user_id);
            match resolution {
                Resolution::Avatar(mxc) => {
                    store.avatars.insert(user_id.clone(), Some(mxc));
                    arrived.insert(user_id);
                }
                Resolution::NoAvatar => {
                    store.avatars.insert(user_id, None);
                }
                Resolution::Unresolved => {}
            }
        }
        arrived
    }
}

pub(super) async fn resolve_reactor_avatars(
    room: &Room,
    client: &Client,
    media: &MediaService,
    wanted: Vec<String>,
) -> Vec<(String, Resolution)> {
    let mut resolved = Vec::with_capacity(wanted.len());
    for user_id in wanted {
        let resolution = resolve_one(room, client, media, &user_id).await;
        resolved.push((user_id, resolution));
    }
    resolved
}

async fn resolve_one(
    room: &Room,
    client: &Client,
    media: &MediaService,
    user_id: &str,
) -> Resolution {
    let Ok(parsed) = OwnedUserId::try_from(user_id) else {
        return Resolution::NoAvatar;
    };
    let member = match room.get_member(&parsed).await {
        Ok(member) => member,
        Err(error) => {
            tracing::debug!(%error, user_id, "could not read a reactor's room member");
            return Resolution::Unresolved;
        }
    };
    let Some(mxc) = member
        .as_ref()
        .and_then(|member| member.avatar_url())
        .map(ToString::to_string)
    else {
        return Resolution::NoAvatar;
    };
    if media
        .fetch_avatar_by_mxc(client, &mxc_avatar_key(&mxc), mxc.as_str().into())
        .await
        .is_none()
    {
        return Resolution::Unresolved;
    }
    Resolution::Avatar(mxc)
}
