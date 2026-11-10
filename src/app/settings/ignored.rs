use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{Semaphore, mpsc};

use super::change::{Change, after_echo, after_write};
use crate::app::event::{ActionOutcome, AppEvent, SettingsEvent};
use crate::app::input::EventSender;
use crate::app::task_group::TaskGroup;
use crate::commands::view::{IgnoredPerson, IgnoredUsersView};
use crate::domain::member_name::MemberName;
use crate::domain::user_info::{IgnoreChange, UserId};
use crate::ports::matrix::UserInfoPort;

const IGNORED_CHANNEL_CAP: usize = 4;
const NAME_LOOKUPS_INFLIGHT: usize = 4;

struct Unignore {
    user_id: UserId,
    change: Change<bool>,
}

pub(super) struct IgnoredUsers {
    events: EventSender,
    follow: TaskGroup,
    lookups: TaskGroup,
    writes: TaskGroup,
    lookup_permits: Arc<Semaphore>,
    showing: Option<Arc<dyn UserInfoPort>>,
    listed: Option<Vec<UserId>>,
    names: HashMap<UserId, MemberName>,
    asked: HashSet<UserId>,
    leaving: Vec<Unignore>,
    generation: u64,
    requests: u64,
    failed: Option<String>,
}

impl IgnoredUsers {
    pub(super) fn new(events: EventSender) -> Self {
        Self {
            events,
            follow: TaskGroup::new("ignored-users-follow"),
            lookups: TaskGroup::new("ignored-users-names"),
            writes: TaskGroup::new("ignored-users-writes"),
            lookup_permits: Arc::new(Semaphore::new(NAME_LOOKUPS_INFLIGHT)),
            showing: None,
            listed: None,
            names: HashMap::new(),
            asked: HashSet::new(),
            leaving: Vec::new(),
            generation: 0,
            requests: 0,
            failed: None,
        }
    }

    pub(super) fn start(&mut self, port: Arc<dyn UserInfoPort>) {
        self.stop();
        let follower = Arc::clone(&port);
        self.follow.spawn_follow(
            &self.events,
            IGNORED_CHANNEL_CAP,
            move |ignored_tx| async move { follow_ignored(follower.as_ref(), ignored_tx).await },
            |listed| AppEvent::Settings(SettingsEvent::IgnoredListed(listed)),
        );
        self.showing = Some(port);
        self.ask_names();
    }

    pub(super) fn stop(&mut self) {
        self.showing = None;
        self.failed = None;
        self.asked.clear();
        self.follow.cancel_and_detach();
        self.lookups.cancel_and_detach();
    }

    pub(super) fn forget_session(&mut self) {
        self.stop();
        self.writes.cancel_and_detach();
        self.listed = None;
        self.names.clear();
        self.leaving.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(
            self.follow.shutdown(),
            self.lookups.shutdown(),
            self.writes.shutdown()
        );
    }

    pub(super) fn listed(&mut self, listed: Vec<UserId>) {
        self.leaving.retain(|leaving| {
            after_echo(
                Some(leaving.change),
                Some(listed.contains(&leaving.user_id)),
            )
            .is_some()
        });
        self.listed = Some(listed);
        self.ask_names();
    }

    pub(super) fn named(&mut self, generation: u64, user_id: UserId, name: MemberName) {
        if generation == self.generation {
            self.names.insert(user_id, name);
        }
    }

    pub(super) fn stop_ignoring(&mut self, port: Arc<dyn UserInfoPort>, user_id: UserId) {
        if !self.lists(&user_id) || self.is_leaving(&user_id) {
            tracing::debug!(%user_id, "not unignoring someone the list does not show or who is already going");
            return;
        }
        self.failed = None;
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        self.leaving.push(Unignore {
            user_id: user_id.clone(),
            change: Change::Requested {
                request,
                target: false,
            },
        });
        self.writes.spawn_reporting(&self.events, async move {
            let outcome = ActionOutcome::of(
                port.set_ignored(&user_id, IgnoreChange::Unignore).await,
                |e| tracing::warn!(%user_id, "failed to stop ignoring someone: {e}"),
            );
            AppEvent::Settings(SettingsEvent::Unignored {
                request,
                user_id,
                outcome,
            })
        });
    }

    pub(super) fn unignored(&mut self, request: u64, user_id: &UserId, outcome: ActionOutcome) {
        let listed = Some(self.lists(user_id));
        self.leaving.retain_mut(|leaving| {
            if leaving.user_id != *user_id {
                return true;
            }
            match after_write(Some(leaving.change), request, outcome, listed) {
                Some(change) => {
                    leaving.change = change;
                    true
                }
                None => false,
            }
        });
    }

    pub(super) fn show_failure(&mut self, user_id: &UserId) -> bool {
        self.failed = self.showing.is_some().then(|| self.shown_name(user_id));
        self.failed.is_some()
    }

    pub(super) fn shown_name(&self, user_id: &UserId) -> String {
        self.names
            .get(user_id)
            .unwrap_or(&MemberName::Unset)
            .shown(user_id)
    }

    pub(super) fn view(&self) -> IgnoredUsersView {
        let Some(listed) = &self.listed else {
            return IgnoredUsersView::default();
        };
        IgnoredUsersView {
            known: true,
            failed: self.failed.clone(),
            people: listed
                .iter()
                .map(|user_id| IgnoredPerson {
                    user_id: user_id.clone(),
                    name: self.names.get(user_id).cloned().unwrap_or_default(),
                    busy: self.is_leaving(user_id),
                })
                .collect(),
        }
    }

    fn lists(&self, user_id: &UserId) -> bool {
        self.listed
            .as_ref()
            .is_some_and(|listed| listed.contains(user_id))
    }

    fn is_leaving(&self, user_id: &UserId) -> bool {
        self.leaving
            .iter()
            .any(|leaving| leaving.user_id == *user_id)
    }

    fn ask_names(&mut self) {
        let (Some(port), Some(listed)) = (&self.showing, &self.listed) else {
            return;
        };
        for user_id in listed {
            if !self.asked.insert(user_id.clone()) {
                continue;
            }
            let port = Arc::clone(port);
            let permits = Arc::clone(&self.lookup_permits);
            let generation = self.generation;
            let user_id = user_id.clone();
            self.lookups.spawn_reporting(&self.events, async move {
                let _turn = permits.acquire().await.ok()?;
                let name = look_up_name(port.as_ref(), &user_id).await?;
                Some(AppEvent::Settings(SettingsEvent::IgnoredNamed {
                    generation,
                    user_id,
                    name,
                }))
            });
        }
    }
}

async fn follow_ignored(port: &dyn UserInfoPort, ignored_tx: mpsc::Sender<Vec<UserId>>) {
    if let Err(e) = port.follow_ignored(ignored_tx).await {
        tracing::warn!("failed to follow the people this account ignores: {e}");
    }
}

async fn look_up_name(port: &dyn UserInfoPort, user_id: &UserId) -> Option<MemberName> {
    match port.global_profile(user_id).await {
        Ok(profile) => Some(MemberName::new(profile.display_name.as_deref(), false)),
        Err(e) => {
            tracing::debug!(%user_id, "could not look up an ignored person's name: {e}");
            None
        }
    }
}
