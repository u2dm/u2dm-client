use std::fmt::Write;

use matrix_sdk::room::edit::EditedContent;
use matrix_sdk::ruma::events::AnyMessageLikeEventContent;
use matrix_sdk::ruma::events::poll::start::PollKind;
use matrix_sdk::ruma::events::poll::unstable_end::UnstablePollEndEventContent;
use matrix_sdk::ruma::events::poll::unstable_response::UnstablePollResponseEventContent;
use matrix_sdk::ruma::events::poll::unstable_start::{
    NewUnstablePollStartEventContent, UnstablePollAnswer, UnstablePollAnswers,
    UnstablePollStartContentBlock,
};
use matrix_sdk::ruma::{EventId, OwnedEventId, UInt};
use matrix_sdk_ui::timeline::{Timeline, TimelineEventItemId};
use tokio::sync::mpsc;

use super::filter::TimelineItems;
use crate::adapters::matrix::permissions::poll_permissions;
use crate::domain::poll::{
    Poll, PollAction, PollChoice, PollDisclosure, PollDraft, PollRevision, RevisedAnswer,
};
use crate::domain::timeline::TimelineUpdate;
use crate::error::{AppError, Result};
use crate::util::random_hex;

const ANSWER_ID_BYTES: usize = 8;

pub(super) fn start_content(draft: &PollDraft) -> Result<AnyMessageLikeEventContent> {
    let answers = draft
        .answers()
        .iter()
        .map(|text| UnstablePollAnswer::new(random_hex(ANSWER_ID_BYTES), text))
        .collect();
    let block = start_block(
        draft.question(),
        answers,
        draft.choice(),
        draft.disclosure(),
    )?;
    let fallback = fallback_text(draft.question(), draft.answers().iter().map(String::as_str));
    let content = NewUnstablePollStartEventContent::plain_text(fallback, block);
    Ok(AnyMessageLikeEventContent::UnstablePollStart(
        content.into(),
    ))
}

fn start_block(
    question: &str,
    answers: Vec<UnstablePollAnswer>,
    choice: PollChoice,
    disclosure: PollDisclosure,
) -> Result<UnstablePollStartContentBlock> {
    let answers =
        UnstablePollAnswers::try_from(answers).map_err(|e| AppError::Other(e.to_string()))?;
    let mut block = UnstablePollStartContentBlock::new(question, answers);
    block.kind = match disclosure {
        PollDisclosure::Disclosed => PollKind::Disclosed,
        PollDisclosure::Undisclosed => PollKind::Undisclosed,
    };
    block.max_selections = UInt::try_from(choice.max()).unwrap_or(UInt::MAX);
    Ok(block)
}

fn fallback_text<'a>(question: &str, answers: impl Iterator<Item = &'a str>) -> String {
    let mut text = question.to_owned();
    for (number, answer) in (1..).zip(answers) {
        write!(text, "\n{number}. {answer}").ok();
    }
    text
}

fn edited_content(revision: PollRevision) -> Result<EditedContent> {
    let PollRevision {
        question,
        answers,
        choice,
        disclosure,
    } = revision;
    let fallback_text = fallback_text(&question, answers.iter().map(|answer| answer.text.as_str()));
    let answers = answers
        .into_iter()
        .map(|RevisedAnswer { id, text }| {
            UnstablePollAnswer::new(id.unwrap_or_else(|| random_hex(ANSWER_ID_BYTES)), text)
        })
        .collect();
    Ok(EditedContent::PollStart {
        fallback_text,
        new_content: start_block(&question, answers, choice, disclosure)?,
    })
}

fn own_editable_poll<'a>(items: &'a TimelineItems, target: &EventId) -> Option<&'a Poll> {
    items
        .message_of_event(target)
        .filter(|message| message.is_own)
        .and_then(|message| message.body.poll())
        .filter(|poll| poll.editable)
}

async fn apply_revision(
    timeline: &Timeline,
    target: OwnedEventId,
    revision: PollRevision,
) -> Result<()> {
    if !poll_permissions(timeline.room()).await.start {
        return Err(AppError::Other(
            "the room's power levels refuse poll edits".to_owned(),
        ));
    }
    let content = edited_content(revision)?;
    timeline
        .edit(&TimelineEventItemId::EventId(target), content)
        .await
        .map_err(|e| AppError::Other(e.to_string()))
}

fn revision_of(
    items: &TimelineItems,
    event_id: &str,
    draft: &PollDraft,
) -> Result<Option<(OwnedEventId, PollRevision)>> {
    let target = OwnedEventId::try_from(event_id).map_err(|e| AppError::Other(e.to_string()))?;
    let poll = own_editable_poll(items, &target).ok_or_else(|| {
        AppError::Other("only an own poll nobody has answered can be edited".to_owned())
    })?;
    Ok(poll.revise(draft).map(|revision| (target, revision)))
}

pub(super) async fn edit(
    timeline: &Timeline,
    items: &TimelineItems,
    event_id: &str,
    draft: &PollDraft,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) {
    let applied = match revision_of(items, event_id, draft) {
        Ok(Some((target, revision))) => apply_revision(timeline, target, revision).await,
        Ok(None) => {
            tracing::debug!(event_id, "ignoring a poll edit that changes nothing");
            Ok(())
        }
        Err(e) => Err(e),
    };
    if let Err(e) = applied {
        tracing::warn!(event_id, "the poll edit was not sent: {e}");
        report(PollAction::Edit, timeline_tx).await;
    }
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
    if !poll_permissions(timeline.room()).await.vote {
        tracing::debug!(event_id, "the room's power levels refuse this vote");
        report(PollAction::Vote, timeline_tx).await;
        return;
    }
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
    if !poll_permissions(timeline.room()).await.end {
        tracing::debug!(event_id, "the room's power levels refuse this poll end");
        report(PollAction::End, timeline_tx).await;
        return;
    }
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
        report(action, timeline_tx).await;
    }
}

async fn report(action: PollAction, timeline_tx: &mpsc::Sender<TimelineUpdate>) {
    drop(
        timeline_tx
            .send(TimelineUpdate::PollSendFailed(action))
            .await,
    );
}

fn end_fallback(poll: &Poll) -> String {
    let leaders: Vec<&str> = poll.leaders().map(|answer| answer.text.as_str()).collect();
    match leaders.as_slice() {
        [] => "The poll has closed with no top answer".to_owned(),
        [only] => format!("The poll has closed. Top answer: {only}"),
        several => format!("The poll has closed. Top answers: {}", several.join(", ")),
    }
}
