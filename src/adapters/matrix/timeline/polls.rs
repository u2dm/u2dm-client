use std::fmt::Write;

use matrix_sdk::ruma::events::AnyMessageLikeEventContent;
use matrix_sdk::ruma::events::poll::start::PollKind;
use matrix_sdk::ruma::events::poll::unstable_end::UnstablePollEndEventContent;
use matrix_sdk::ruma::events::poll::unstable_response::UnstablePollResponseEventContent;
use matrix_sdk::ruma::events::poll::unstable_start::{
    NewUnstablePollStartEventContent, UnstablePollAnswer, UnstablePollAnswers,
    UnstablePollStartContentBlock,
};
use matrix_sdk::ruma::{OwnedEventId, UInt};
use matrix_sdk_ui::timeline::Timeline;
use tokio::sync::mpsc;

use super::filter::TimelineItems;
use crate::domain::poll::{Poll, PollAction, PollDisclosure, PollDraft};
use crate::domain::timeline::TimelineUpdate;
use crate::error::{AppError, Result};
use crate::util::random_hex;

const ANSWER_ID_BYTES: usize = 8;

pub(super) fn start_content(draft: &PollDraft) -> Result<AnyMessageLikeEventContent> {
    let answers: Vec<UnstablePollAnswer> = draft
        .answers()
        .iter()
        .map(|text| UnstablePollAnswer::new(random_hex(ANSWER_ID_BYTES), text))
        .collect();
    let answers =
        UnstablePollAnswers::try_from(answers).map_err(|e| AppError::Other(e.to_string()))?;
    let mut block = UnstablePollStartContentBlock::new(draft.question(), answers);
    block.kind = match draft.disclosure() {
        PollDisclosure::Disclosed => PollKind::Disclosed,
        PollDisclosure::Undisclosed => PollKind::Undisclosed,
    };
    block.max_selections = UInt::try_from(draft.choice().max()).unwrap_or(UInt::MAX);
    let content = NewUnstablePollStartEventContent::plain_text(start_fallback(draft), block);
    Ok(AnyMessageLikeEventContent::UnstablePollStart(
        content.into(),
    ))
}

fn start_fallback(draft: &PollDraft) -> String {
    let mut text = draft.question().to_owned();
    for (number, answer) in (1..).zip(draft.answers()) {
        write!(text, "\n{number}. {answer}").ok();
    }
    text
}

pub(super) async fn vote(
    timeline: &Timeline,
    items: &TimelineItems,
    event_id: &str,
    answer_id: &str,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) {
    let Ok(target) = OwnedEventId::try_from(event_id) else {
        tracing::warn!(event_id, "ignoring a vote for a malformed event id");
        return;
    };
    let Some(selection) = items
        .message_of_event(&target)
        .and_then(|message| message.body.poll())
        .and_then(|poll| poll.next_selection(answer_id))
    else {
        tracing::debug!(event_id, "ignoring a vote that would change nothing");
        return;
    };
    let content = UnstablePollResponseEventContent::new(selection, target);
    queue(timeline, content.into(), PollAction::Vote, timeline_tx).await;
}

pub(super) async fn end(
    timeline: &Timeline,
    items: &TimelineItems,
    event_id: &str,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) {
    let Ok(target) = OwnedEventId::try_from(event_id) else {
        tracing::warn!(event_id, "ignoring a poll end for a malformed event id");
        return;
    };
    let Some(poll) = items
        .message_of_event(&target)
        .filter(|message| message.is_own)
        .and_then(|message| message.body.poll())
        .filter(|poll| poll.is_open())
    else {
        tracing::warn!(
            event_id,
            "ignoring a poll end for anything but an own open poll"
        );
        return;
    };
    let content = UnstablePollEndEventContent::new(end_fallback(poll), target);
    queue(timeline, content.into(), PollAction::End, timeline_tx).await;
}

async fn queue(
    timeline: &Timeline,
    content: AnyMessageLikeEventContent,
    action: PollAction,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) {
    if let Err(e) = timeline.send(content).await {
        tracing::warn!(?action, "the send queue refused a poll send: {e}");
        drop(
            timeline_tx
                .send(TimelineUpdate::PollSendFailed(action))
                .await,
        );
    }
}

fn end_fallback(poll: &Poll) -> String {
    let leaders: Vec<&str> = poll.leaders().map(|answer| answer.text.as_str()).collect();
    match leaders.as_slice() {
        [] => "The poll has closed with no top answer".to_owned(),
        [only] => format!("The poll has closed. Top answer: {only}"),
        several => format!("The poll has closed. Top answers: {}", several.join(", ")),
    }
}
