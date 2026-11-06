use std::ops::ControlFlow;

use matrix_sdk::Room;
use matrix_sdk_ui::timeline::TimelineItem;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::bundled;
use super::enrichment::{MEMBER_BATCHES, spawn_emoji_downloads, spawn_member_fetch};
use crate::adapters::matrix::timeline::TimelineContext;
use crate::adapters::matrix::timeline::convert::{involves, reacted_with};
use crate::adapters::matrix::timeline::diff::stamp_read_marks;
use crate::adapters::matrix::timeline::emoji_files::Settled;
use crate::adapters::matrix::timeline::filter::TimelineItems;
use crate::adapters::matrix::timeline::members::{Arrived, Batch, Need};
use crate::domain::message::TimelineMessage;
use crate::domain::timeline::{TimelinePatch, TimelineUpdate};

const SETTLED_IMAGES: usize = 8;

pub(super) enum Fetched {
    Members(Batch),
    Image(Settled),
}

pub(super) struct RowFetches {
    members_tx: mpsc::Sender<Batch>,
    members_rx: mpsc::Receiver<Batch>,
    images_tx: mpsc::Sender<Settled>,
    images_rx: mpsc::Receiver<Settled>,
}

impl RowFetches {
    pub(super) fn new() -> Self {
        let (members_tx, members_rx) = mpsc::channel(MEMBER_BATCHES);
        let (images_tx, images_rx) = mpsc::channel(SETTLED_IMAGES);
        Self {
            members_tx,
            members_rx,
            images_tx,
            images_rx,
        }
    }

    pub(super) fn spawn(
        &self,
        ctx: &TimelineContext<'_>,
        room: &Room,
        side_tasks: &mut JoinSet<()>,
    ) {
        spawn_member_fetch(ctx, room, side_tasks, &self.members_tx);
        spawn_emoji_downloads(ctx, side_tasks, &self.images_tx);
    }

    pub(super) async fn next(&mut self) -> Option<Fetched> {
        tokio::select! {
            Some(batch) = self.members_rx.recv() => Some(Fetched::Members(batch)),
            Some(settled) = self.images_rx.recv() => Some(Fetched::Image(settled)),
            else => None,
        }
    }
}

pub(super) fn fetched_patch(
    items: &mut TimelineItems,
    fetched: Fetched,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    match fetched {
        Fetched::Members(batch) => member_patch(items, &ctx.members.record(batch), ctx),
        Fetched::Image(settled) => image_patch(items, settled, ctx),
    }
}

fn member_patch(
    items: &mut TimelineItems,
    arrived: &Arrived,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    if arrived.users.is_empty() {
        return None;
    }
    let mut batch = reconvert_rows(items, ctx, |item, message| {
        involves(item, message, arrived, &ctx.members.names)
    });
    if matches!(arrived.need, Need::Name) {
        stamp_read_marks(items, &mut batch, ctx);
    }
    bundled(batch)
}

fn image_patch(
    items: &mut TimelineItems,
    settled: Settled,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let mxc = ctx.emoji_files.record(settled);
    bundled(reconvert_rows(items, ctx, |item, message| {
        reacted_with(item, &mxc)
            || message.is_some_and(|message| message.body_emoji.contains_key(&mxc))
    }))
}

pub(super) async fn send_patch(
    ctx: &TimelineContext<'_>,
    patch: Option<TimelinePatch>,
) -> ControlFlow<()> {
    let Some(patch) = patch else {
        return ControlFlow::Continue(());
    };
    match ctx.timeline_tx.send(TimelineUpdate::patch(patch)).await {
        Ok(()) => ControlFlow::Continue(()),
        Err(_) => ControlFlow::Break(()),
    }
}

fn reconvert_rows(
    items: &mut TimelineItems,
    ctx: &TimelineContext<'_>,
    involved: impl Fn(&TimelineItem, Option<&TimelineMessage>) -> bool,
) -> Vec<TimelinePatch> {
    let rows: Vec<usize> = items
        .items()
        .iter()
        .enumerate()
        .filter(|(raw_index, item)| involved(item, items.message_at(*raw_index)))
        .map(|(raw_index, _)| raw_index)
        .collect();
    let mut patches: Vec<TimelinePatch> = Vec::new();
    for raw_index in rows {
        if let Some(message) = items.reconvert(raw_index, ctx) {
            patches.push(TimelinePatch::Set {
                index: items.msg_index_at(raw_index),
                message,
            });
        }
    }
    patches
}
