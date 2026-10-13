use std::sync::Arc;

use tokio::sync::{mpsc, watch};

use super::event::AppEvent;
use super::input::EventSender;
use super::task_group::TaskGroup;
use crate::commands::ui::ComposerActivity;
use crate::commands::view::TypingView;
use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;
use crate::domain::typing::Typists;
use crate::ports::matrix::TypingPort;
use crate::ports::output::AppOutputPort;

const TYPISTS_CHANNEL_CAP: usize = 8;

struct NoticeLane {
    room_id: RoomId,
    wanted: watch::Sender<bool>,
}

impl NoticeLane {
    fn says_typing(&self) -> bool {
        *self.wanted.borrow()
    }

    fn want(&self, typing: bool) {
        self.wanted.send_replace(typing);
    }
}

pub(super) struct Typing {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    follow: TaskGroup,
    notices: TaskGroup,
    watch: u64,
    room_id: Option<RoomId>,
    typists: Typists,
    edits: i32,
    lane: Option<NoticeLane>,
}

impl Typing {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            follow: TaskGroup::new("typists"),
            notices: TaskGroup::new("typing-notices"),
            watch: 0,
            room_id: None,
            typists: Typists::default(),
            edits: 0,
            lane: None,
        }
    }

    pub(super) fn retarget(&mut self, port: Arc<dyn TypingPort>, room_id: RoomId) {
        if self
            .lane
            .as_ref()
            .is_some_and(|lane| lane.room_id != room_id)
        {
            self.stop_typing();
        }
        self.follow.cancel_and_detach();
        self.watch = self.watch.wrapping_add(1);
        if self.room_id.as_ref() != Some(&room_id) {
            self.room_id = Some(room_id.clone());
            self.typists = Typists::default();
            self.publish();
        }
        tracing::debug!(%room_id, watch = self.watch, "following who is typing");

        let watch = self.watch;
        let events = self.events.clone();
        let token = self.follow.token();
        self.follow.spawn(async move {
            let (typists_tx, typists_rx) = mpsc::channel(TYPISTS_CHANNEL_CAP);
            let follow = async {
                tokio::join!(
                    follow_typists(port.as_ref(), &room_id, typists_tx),
                    forward_typists(&events, watch, typists_rx),
                )
            };
            token.run_until_cancelled(follow).await;
        });
    }

    pub(super) fn clear(&mut self) {
        self.stop_typing();
        self.follow.cancel_and_detach();
        self.watch = self.watch.wrapping_add(1);
        self.room_id = None;
        self.typists = Typists::default();
        self.publish();
    }

    pub(super) fn typists_changed(&mut self, watch: u64, typing: Vec<NamedUser>) {
        if watch != self.watch {
            tracing::debug!(watch, "dropping typists from a superseded follow");
            return;
        }
        let typists = self.typists.replaced_by(typing);
        if typists == self.typists {
            return;
        }
        if let Some(room_id) = &self.room_id {
            tracing::debug!(%room_id, typists = typists.users().len(), "who is typing changed");
        }
        self.typists = typists;
        self.publish();
    }

    pub(super) fn composer_changed(
        &mut self,
        port: Option<Arc<dyn TypingPort>>,
        selected: Option<&RoomId>,
        activity: &ComposerActivity,
    ) {
        let typed = activity.edits != self.edits;
        self.edits = activity.edits;
        let here = activity
            .room_id
            .as_ref()
            .filter(|room_id| Some(*room_id) == selected);
        let stopped = self.lane.as_ref().is_some_and(|lane| {
            lane.says_typing() && (!activity.composing || here != Some(&lane.room_id))
        });
        if stopped {
            self.stop_typing();
        }
        let Some(room_id) = here.filter(|_| typed && activity.composing) else {
            return;
        };
        let Some(port) = port else {
            return;
        };
        let lane = self.lane_for(port, room_id);
        if !lane.says_typing() {
            tracing::debug!(%room_id, "you started typing");
        }
        lane.want(true);
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.follow.restart(), self.notices.restart());
        self.watch = self.watch.wrapping_add(1);
        self.room_id = None;
        self.typists = Typists::default();
        self.lane = None;
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.follow.shutdown(), self.notices.shutdown());
    }

    fn stop_typing(&self) {
        if let Some(lane) = self.lane.as_ref().filter(|lane| lane.says_typing()) {
            tracing::debug!(room_id = %lane.room_id, "you stopped typing");
            lane.want(false);
        }
    }

    fn lane_for(&mut self, port: Arc<dyn TypingPort>, room_id: &RoomId) -> &NoticeLane {
        match self.lane.take() {
            Some(lane) if lane.room_id == *room_id && !lane.wanted.is_closed() => {
                self.lane.insert(lane)
            }
            _ => {
                let lane = self.open_lane(port, room_id);
                self.lane.insert(lane)
            }
        }
    }

    fn open_lane(&mut self, port: Arc<dyn TypingPort>, room_id: &RoomId) -> NoticeLane {
        let (wanted, wanted_rx) = watch::channel(false);
        let room = room_id.clone();
        let token = self.notices.token();
        self.notices.spawn(async move {
            token
                .run_until_cancelled(tell_the_room(port.as_ref(), &room, wanted_rx))
                .await;
        });
        NoticeLane {
            room_id: room_id.clone(),
            wanted,
        }
    }

    fn publish(&self) {
        let view = TypingView {
            room_id: self.room_id.clone(),
            typists: self.typists.clone(),
        };
        self.output
            .publish(Box::new(move |state| state.typing = view));
    }
}

async fn tell_the_room(port: &dyn TypingPort, room_id: &RoomId, mut wanted: watch::Receiver<bool>) {
    while wanted.changed().await.is_ok() {
        let typing = *wanted.borrow_and_update();
        if let Err(e) = port.notify_typing(room_id, typing).await {
            tracing::debug!(%room_id, typing, "the room was not told whether you are typing: {e}");
        }
    }
}

async fn follow_typists(
    port: &dyn TypingPort,
    room_id: &RoomId,
    typists_tx: mpsc::Sender<Vec<NamedUser>>,
) {
    if let Err(e) = port.follow_typing(room_id, typists_tx).await {
        tracing::warn!(%room_id, "failed to follow who is typing: {e}");
    }
}

async fn forward_typists(
    events: &EventSender,
    watch: u64,
    mut typists_rx: mpsc::Receiver<Vec<NamedUser>>,
) {
    while let Some(typists) = typists_rx.recv().await {
        if events
            .send(AppEvent::TypistsChanged { watch, typists })
            .is_err()
        {
            return;
        }
    }
}
