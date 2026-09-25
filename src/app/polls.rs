use std::sync::Arc;

use super::send_lanes::SendLanes;
use super::show_toast;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::Toast;
use crate::domain::poll::PollDraft;
use crate::domain::room::RoomId;
use crate::ports::matrix::TimelinePort;
use crate::ports::output::AppOutputPort;

pub(super) fn send(
    lanes: &mut SendLanes,
    output: Arc<dyn AppOutputPort>,
    timeline: Arc<dyn TimelinePort>,
    room_id: RoomId,
    draft: PollDraft,
) {
    lanes.spawn(room_id.clone(), async move {
        if let Err(e) = timeline.send_poll(&room_id, &draft).await {
            tracing::warn!("failed to queue a poll: {e}");
            show_toast(
                output.as_ref(),
                Toast::Error(UserMessage::new(UserMessageKind::SendMessageFailed)),
            );
        }
    });
}
