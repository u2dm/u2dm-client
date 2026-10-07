use std::collections::HashSet;
use std::sync::Arc;

use super::event::{AppEvent, MentionsEvent};
use super::input::EventSender;
use super::space_index::AVATAR_BATCH;
use super::task_group::TaskGroup;
use crate::commands::view::MentionsView;
use crate::domain::mention::{MentionCandidate, MentionQuery, is_user_tag, rank};
use crate::domain::room::RoomId;
use crate::domain::room_info::{RosterMember, RosterSection};
use crate::error::Result;
use crate::ports::matrix::RoomInfoPort;
use crate::ports::output::AppOutputPort;

const MENTION_LIMIT: usize = 20;

pub(super) struct Suggestion {
    pub(super) room_id: RoomId,
    pub(super) joined_count: u64,
    pub(super) offers_room: bool,
    pub(super) own_user: String,
    pub(super) query: String,
    pub(super) recent: Vec<String>,
}

struct Asked {
    query: MentionQuery,
    recent: Vec<String>,
    offers_room: bool,
}

#[derive(Default)]
struct Roster {
    candidates: Arc<[MentionCandidate]>,
    fetched_at: Option<u64>,
    fetching: Option<u64>,
    failed_this_word: bool,
}

pub(super) struct MentionSuggestions {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    requests: u64,
    room: Option<RoomId>,
    own_user: String,
    roster: Roster,
    asked: Option<Asked>,
    requested: HashSet<String>,
    avatars_ready: usize,
}

impl MentionSuggestions {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("mentions"),
            requests: 0,
            room: None,
            own_user: String::new(),
            roster: Roster::default(),
            asked: None,
            requested: HashSet::new(),
            avatars_ready: 0,
        }
    }

    pub(super) fn suggest(&mut self, port: Arc<dyn RoomInfoPort>, suggestion: Suggestion) {
        if self.room.as_ref() != Some(&suggestion.room_id) {
            self.forget();
            self.room = Some(suggestion.room_id.clone());
        }
        self.own_user = suggestion.own_user;
        let query = MentionQuery::new(&suggestion.query);
        self.asked = Some(Asked {
            offers_room: suggestion.offers_room && query.names_room(),
            query,
            recent: suggestion.recent,
        });
        let stale = self.roster.fetched_at != Some(suggestion.joined_count);
        let idle = self.roster.fetching.is_none() && !self.roster.failed_this_word;
        if stale && idle {
            self.fetch(
                Arc::clone(&port),
                suggestion.room_id,
                suggestion.joined_count,
            );
        }
        self.publish(port);
    }

    pub(super) fn end(&mut self) {
        self.roster.failed_this_word = false;
        if self.asked.take().is_some() {
            self.publish_rows(Arc::from(Vec::new()), false);
        }
    }

    pub(super) fn roster_loaded(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        request: u64,
        joined_count: u64,
        roster: Result<Vec<RosterMember>>,
    ) {
        if request != self.requests || self.roster.fetching != Some(joined_count) {
            tracing::debug!(request, "dropping a mention roster nobody waits for");
            return;
        }
        self.roster.fetching = None;
        match roster {
            Ok(members) => {
                self.roster.candidates = members
                    .into_iter()
                    .filter(|member| self.mentionable(member))
                    .map(|member| MentionCandidate::new(Arc::new(member)))
                    .collect();
                self.roster.fetched_at = Some(joined_count);
            }
            Err(e) => {
                tracing::warn!("failed to read the members to suggest: {e}");
                self.roster.failed_this_word = true;
            }
        }
        if self.asked.is_some() {
            self.publish(port);
        }
    }

    pub(super) fn avatars_ready(&mut self, request: u64, ready: usize) {
        if request != self.requests {
            return;
        }
        self.avatars_ready = self.avatars_ready.saturating_add(ready);
        if self.asked.is_some() {
            let avatars_ready = self.avatars_ready;
            self.output.publish(Box::new(move |state| {
                state.mentions.avatars_ready = avatars_ready;
            }));
        }
    }

    pub(super) fn reset(&mut self) {
        let shown = self.asked.is_some();
        self.forget();
        if shown {
            self.output
                .publish(Box::new(|state| state.mentions = MentionsView::default()));
        }
    }

    pub(super) async fn restart(&mut self) {
        self.tasks.restart().await;
        self.forget();
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }

    fn forget(&mut self) {
        self.tasks.cancel_and_detach();
        self.requests = self.requests.wrapping_add(1);
        self.room = None;
        self.roster = Roster::default();
        self.asked = None;
        self.requested.clear();
        self.avatars_ready = 0;
    }

    fn mentionable(&self, member: &RosterMember) -> bool {
        member.section == RosterSection::Joined
            && member.user_id != self.own_user
            && is_user_tag(&member.user_id)
    }

    fn fetch(&mut self, port: Arc<dyn RoomInfoPort>, room_id: RoomId, joined_count: u64) {
        self.roster.fetching = Some(joined_count);
        let request = self.requests;
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let roster = tokio::select! {
                () = cancel.cancelled() => return,
                roster = port.roster(&room_id) => roster,
            };
            let loaded = MentionsEvent::RosterLoaded {
                request,
                joined_count,
                roster,
            };
            drop(events.send(AppEvent::Mentions(loaded)));
        });
    }

    fn publish(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(asked) = self.asked.as_ref() else {
            return;
        };
        let rows: Arc<[Arc<RosterMember>]> = rank(
            &self.roster.candidates,
            &asked.query,
            &asked.recent,
            MENTION_LIMIT,
        )
        .into();
        let offers_room = asked.offers_room;
        let wanted: Vec<String> = rows
            .iter()
            .filter_map(|member| member.avatar_mxc.clone())
            .filter(|mxc| self.requested.insert(mxc.clone()))
            .collect();
        self.publish_rows(rows, offers_room);
        self.fetch_avatars(port, wanted);
    }

    fn publish_rows(&self, rows: Arc<[Arc<RosterMember>]>, offers_room: bool) {
        let view = MentionsView {
            room_id: self.room.clone(),
            offers_room,
            rows,
            avatars_ready: self.avatars_ready,
        };
        self.output
            .publish(Box::new(move |state| state.mentions = view));
    }

    fn fetch_avatars(&mut self, port: Arc<dyn RoomInfoPort>, mxcs: Vec<String>) {
        if mxcs.is_empty() {
            return;
        }
        let request = self.requests;
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            for batch in mxcs.chunks(AVATAR_BATCH) {
                let ready = tokio::select! {
                    () = cancel.cancelled() => return,
                    ready = port.fetch_avatars(batch) => ready,
                };
                if ready > 0 {
                    let landed = MentionsEvent::AvatarsReady { request, ready };
                    drop(events.send(AppEvent::Mentions(landed)));
                }
            }
        });
    }
}
