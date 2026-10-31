use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::ruma::events::typing::SyncTypingEvent;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId};
use matrix_sdk::{Client, Room};
use tokio::sync::{mpsc, watch};
use tracing::Instrument;

use crate::adapters::matrix::member_name::named_member;
use crate::adapters::matrix::requests::in_time;
use crate::adapters::matrix::session::{ClientHandle, SessionResource};
use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;
use crate::error::Result;
use crate::ports::matrix::TypingPort;

const NOTICE_LIFETIME: Duration = Duration::from_secs(4);

type TypingBoard = HashMap<OwnedRoomId, Vec<OwnedUserId>>;

pub(in crate::adapters::matrix) struct MatrixTyping {
    matrix: Arc<ClientHandle>,
    board: Arc<watch::Sender<TypingBoard>>,
    listener: Mutex<Option<EventHandlerDropGuard>>,
}

impl MatrixTyping {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>, client: &Client) -> Self {
        let board = Arc::new(watch::Sender::new(TypingBoard::new()));
        let handle = client.add_event_handler({
            let board = Arc::clone(&board);
            move |event: SyncTypingEvent, room: Room| {
                let board = Arc::clone(&board);
                async move { note(&board, &room, event.content.user_ids) }
            }
        });
        Self {
            matrix,
            board,
            listener: Mutex::new(Some(client.event_handler_drop_guard(handle))),
        }
    }
}

#[async_trait]
impl SessionResource for MatrixTyping {
    async fn release(&self) {
        let listener = self
            .listener
            .lock()
            .ok()
            .and_then(|mut listener| listener.take());
        drop(listener);
    }
}

#[async_trait]
impl TypingPort for MatrixTyping {
    async fn follow_typing(
        &self,
        room_id: &RoomId,
        typists_tx: mpsc::Sender<Vec<NamedUser>>,
    ) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        let mut board = self.board.subscribe();
        let mut reported = None;
        loop {
            let typing = board
                .borrow_and_update()
                .get(room.room_id())
                .cloned()
                .unwrap_or_default();
            if reported.as_ref() != Some(&typing) {
                if typists_tx.send(named(&room, &typing).await).await.is_err() {
                    return Ok(());
                }
                reported = Some(typing);
            }
            if board.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    async fn notify_typing(&self, room_id: &RoomId, typing: bool) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        in_time(
            NOTICE_LIFETIME,
            "the homeserver took longer than the notice lasts",
            room.typing_notice(typing),
        )
        .instrument(tracing::debug_span!("typing", %room_id, typing))
        .await
    }
}

fn note(board: &watch::Sender<TypingBoard>, room: &Room, user_ids: Vec<OwnedUserId>) {
    let mut typing: Vec<OwnedUserId> = user_ids
        .into_iter()
        .filter(|user_id| user_id != room.own_user_id())
        .collect();
    typing.sort_unstable();
    typing.dedup();
    board.send_if_modified(|rooms| {
        if typing.is_empty() {
            return rooms.remove(room.room_id()).is_some();
        }
        if rooms.get(room.room_id()) == Some(&typing) {
            return false;
        }
        rooms.insert(room.room_id().to_owned(), typing);
        true
    });
}

async fn named(room: &Room, typing: &[OwnedUserId]) -> Vec<NamedUser> {
    let client = room.client();
    let mut typists = Vec::with_capacity(typing.len());
    for user_id in typing {
        if !client.is_user_ignored(user_id).await {
            typists.push(named_member(room, user_id).await);
        }
    }
    typists
}
