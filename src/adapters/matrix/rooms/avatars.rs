use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::ruma::OwnedMxcUri;
use tokio::task::{JoinError, JoinSet};
use tokio::time::Instant;

use crate::adapters::matrix::media::{MediaService, mxc_avatar_key};

const AVATAR_INFLIGHT: usize = 8;
const AVATAR_RETRY_COOLDOWN: Duration = Duration::from_mins(1);
const AVATAR_REVALIDATE: Duration = Duration::from_mins(5);

#[derive(Clone, Copy)]
pub(super) enum AvatarKind {
    Room,
    Space,
}

enum AvatarState {
    Queued,
    InFlight,
    Failed { retry_at: Instant },
    Ready { revalidate_at: Instant },
}

impl AvatarState {
    fn due_at(&self) -> Option<Instant> {
        match *self {
            AvatarState::Failed { retry_at } => Some(retry_at),
            AvatarState::Ready { revalidate_at } => Some(revalidate_at),
            AvatarState::Queued | AvatarState::InFlight => None,
        }
    }
}

struct TrackedAvatar {
    kind: AvatarKind,
    state: AvatarState,
}

pub(super) struct FetchedAvatar {
    mxc: String,
    kind: AvatarKind,
    fetched: bool,
}

pub(super) struct AvatarFetcher {
    media: Arc<MediaService>,
    tasks: JoinSet<FetchedAvatar>,
    tracked: HashMap<String, TrackedAvatar>,
    queue: VecDeque<String>,
    next_due: Option<Instant>,
}

impl AvatarFetcher {
    pub(super) fn new(media: Arc<MediaService>) -> Self {
        Self {
            media,
            tasks: JoinSet::new(),
            tracked: HashMap::new(),
            queue: VecDeque::new(),
            next_due: None,
        }
    }

    pub(super) fn request<'a>(
        &mut self,
        client: &Client,
        kind: AvatarKind,
        uris: impl Iterator<Item = &'a str>,
    ) {
        for mxc in uris {
            if self.tracked.contains_key(mxc) {
                continue;
            }
            self.arm(mxc.to_owned(), kind);
        }
        self.pump(client);
    }

    fn arm(&mut self, mxc: String, kind: AvatarKind) {
        let key = mxc_avatar_key(&mxc);
        let state = if self.media.cache_get(&key).is_some() {
            AvatarState::Ready {
                revalidate_at: self.schedule(AVATAR_REVALIDATE),
            }
        } else if self.media.is_failed(&key) {
            AvatarState::Failed {
                retry_at: self.schedule(AVATAR_RETRY_COOLDOWN),
            }
        } else {
            self.queue.push_back(mxc.clone());
            AvatarState::Queued
        };
        self.tracked.insert(mxc, TrackedAvatar { kind, state });
    }

    fn schedule(&mut self, after: Duration) -> Instant {
        let at = Instant::now() + after;
        self.next_due = Some(self.next_due.map_or(at, |due| due.min(at)));
        at
    }

    pub(super) fn due_at(&self) -> Option<Instant> {
        self.next_due
    }

    pub(super) fn wake(&mut self, client: &Client) {
        let now = Instant::now();
        let mut due: Vec<(String, AvatarKind)> = Vec::new();
        let mut next: Option<Instant> = None;
        for (mxc, tracked) in &self.tracked {
            match tracked.state.due_at() {
                Some(at) if at <= now => due.push((mxc.clone(), tracked.kind)),
                Some(at) => next = Some(next.map_or(at, |soonest: Instant| soonest.min(at))),
                None => {}
            }
        }
        self.next_due = next;
        for (mxc, kind) in due {
            self.arm(mxc, kind);
        }
        self.pump(client);
    }

    fn pump(&mut self, client: &Client) {
        while self.tasks.len() < AVATAR_INFLIGHT {
            let Some(mxc) = self.queue.pop_front() else {
                break;
            };
            let Some(tracked) = self.tracked.get_mut(&mxc) else {
                continue;
            };
            if !matches!(tracked.state, AvatarState::Queued) {
                continue;
            }
            tracked.state = AvatarState::InFlight;
            let kind = tracked.kind;
            self.spawn_fetch(client, mxc, kind);
        }
    }

    fn spawn_fetch(&mut self, client: &Client, mxc: String, kind: AvatarKind) {
        let client = client.clone();
        let media = Arc::clone(&self.media);
        self.tasks.spawn(async move {
            let key = mxc_avatar_key(&mxc);
            let fetched = media
                .fetch_avatar_by_mxc(&client, &key, OwnedMxcUri::from(mxc.as_str()))
                .await
                .is_some();
            FetchedAvatar { mxc, kind, fetched }
        });
    }

    pub(super) async fn join_next(&mut self) -> Option<Result<FetchedAvatar, JoinError>> {
        self.tasks.join_next().await
    }

    pub(super) fn finish(
        &mut self,
        client: &Client,
        joined: Result<FetchedAvatar, JoinError>,
    ) -> Option<AvatarKind> {
        let ready = self.record(joined);
        self.pump(client);
        ready
    }

    fn record(&mut self, joined: Result<FetchedAvatar, JoinError>) -> Option<AvatarKind> {
        let done = match joined {
            Ok(done) => done,
            Err(e) => {
                tracing::warn!("avatar fetch task did not complete: {e}");
                return None;
            }
        };
        let state = if done.fetched {
            AvatarState::Ready {
                revalidate_at: self.schedule(AVATAR_REVALIDATE),
            }
        } else {
            AvatarState::Failed {
                retry_at: self.schedule(AVATAR_RETRY_COOLDOWN),
            }
        };
        self.tracked.insert(
            done.mxc,
            TrackedAvatar {
                kind: done.kind,
                state,
            },
        );
        done.fetched.then_some(done.kind)
    }
}
