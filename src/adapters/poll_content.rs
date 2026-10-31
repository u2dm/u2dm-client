use std::fmt::Write;

use ruma::UInt;
use ruma::events::AnyMessageLikeEventContent;
use ruma::events::poll::start::PollKind;
use ruma::events::poll::unstable_start::{
    NewUnstablePollStartEventContent, UnstablePollAnswer, UnstablePollAnswers,
    UnstablePollStartContentBlock,
};

use crate::domain::poll::{PollChoice, PollDisclosure, PollDraft};
use crate::error::{AppError, Result};
use crate::util::random_hex;

pub const ANSWER_ID_BYTES: usize = 8;

pub fn start_content(draft: &PollDraft) -> Result<AnyMessageLikeEventContent> {
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

pub fn start_block(
    question: &str,
    answers: Vec<UnstablePollAnswer>,
    choice: PollChoice,
    disclosure: PollDisclosure,
) -> Result<UnstablePollStartContentBlock> {
    let answers = UnstablePollAnswers::try_from(answers).map_err(AppError::other)?;
    let mut block = UnstablePollStartContentBlock::new(question, answers);
    block.kind = match disclosure {
        PollDisclosure::Disclosed => PollKind::Disclosed,
        PollDisclosure::Undisclosed => PollKind::Undisclosed,
    };
    block.max_selections = UInt::try_from(choice.max()).unwrap_or(UInt::MAX);
    Ok(block)
}

pub fn fallback_text<'a>(question: &str, answers: impl Iterator<Item = &'a str>) -> String {
    let mut text = question.to_owned();
    for (number, answer) in (1..).zip(answers) {
        write!(text, "\n{number}. {answer}").ok();
    }
    text
}
