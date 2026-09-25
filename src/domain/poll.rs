const SHARE_SCALE: u16 = 1000;

const POLL_ANSWERS_MIN: usize = 2;
const POLL_ANSWERS_MAX: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollDisclosure {
    Disclosed,
    Undisclosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollChoice {
    Single,
    Multiple { max: usize },
}

impl PollChoice {
    pub fn up_to(max_selections: usize) -> Self {
        if max_selections > 1 {
            Self::Multiple {
                max: max_selections,
            }
        } else {
            Self::Single
        }
    }

    pub fn max(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Multiple { max } => max,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollAction {
    Vote,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollStatus {
    Open,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollAnswer {
    pub id: String,
    pub text: String,
    pub votes: usize,
    pub mine: bool,
}

impl PollAnswer {
    pub fn share(&self, voters: usize) -> f32 {
        if voters == 0 {
            return 0.0;
        }
        let scaled = self
            .votes
            .min(voters)
            .saturating_mul(usize::from(SHARE_SCALE))
            / voters;
        f32::from(u16::try_from(scaled).unwrap_or(SHARE_SCALE)) / f32::from(SHARE_SCALE)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Poll {
    pub question: String,
    pub disclosure: PollDisclosure,
    pub choice: PollChoice,
    pub answers: Vec<PollAnswer>,
    pub voters: usize,
    pub status: PollStatus,
}

impl Poll {
    pub fn is_open(&self) -> bool {
        self.status == PollStatus::Open
    }

    pub fn reveals_results(&self) -> bool {
        self.disclosure == PollDisclosure::Disclosed || !self.is_open()
    }

    pub fn next_selection(&self, answer_id: &str) -> Option<Vec<String>> {
        let answer = self.answers.iter().find(|answer| answer.id == answer_id)?;
        if !self.can_toggle(answer) {
            return None;
        }
        Some(match self.choice {
            PollChoice::Single => vec![answer.id.clone()],
            PollChoice::Multiple { .. } => self
                .answers
                .iter()
                .filter(|candidate| (candidate.id == answer_id) != candidate.mine)
                .map(|candidate| candidate.id.clone())
                .collect(),
        })
    }

    pub fn leaders(&self) -> impl Iterator<Item = &PollAnswer> {
        self.answers.iter().filter(|answer| self.is_leading(answer))
    }

    pub fn can_toggle(&self, answer: &PollAnswer) -> bool {
        self.is_open()
            && match self.choice {
                PollChoice::Single => !answer.mine,
                PollChoice::Multiple { .. } => answer.mine || self.accepts_another(),
            }
    }

    fn accepts_another(&self) -> bool {
        self.answers.iter().filter(|answer| answer.mine).count() < self.choice.max()
    }

    pub fn is_leading(&self, answer: &PollAnswer) -> bool {
        answer.votes > 0 && answer.votes == self.leading_votes()
    }

    fn leading_votes(&self) -> usize {
        self.answers
            .iter()
            .map(|answer| answer.votes)
            .max()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceMode {
    Single,
    Multiple,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollDraftError {
    NoQuestion,
    TooFewAnswers,
    TooManyAnswers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollDraft {
    question: String,
    answers: Vec<String>,
    choice: PollChoice,
    disclosure: PollDisclosure,
}

impl PollDraft {
    pub fn parse(
        question: String,
        answers: Vec<String>,
        mode: ChoiceMode,
        disclosure: PollDisclosure,
    ) -> Result<Self, PollDraftError> {
        let question = trimmed(question);
        if question.is_empty() {
            return Err(PollDraftError::NoQuestion);
        }
        let answers: Vec<String> = answers
            .into_iter()
            .map(trimmed)
            .filter(|answer| !answer.is_empty())
            .collect();
        if answers.len() < POLL_ANSWERS_MIN {
            return Err(PollDraftError::TooFewAnswers);
        }
        if answers.len() > POLL_ANSWERS_MAX {
            return Err(PollDraftError::TooManyAnswers);
        }
        let choice = match mode {
            ChoiceMode::Single => PollChoice::Single,
            ChoiceMode::Multiple => PollChoice::Multiple { max: answers.len() },
        };
        Ok(Self {
            question,
            answers,
            choice,
            disclosure,
        })
    }

    pub fn question(&self) -> &str {
        &self.question
    }

    pub fn answers(&self) -> &[String] {
        &self.answers
    }

    pub fn choice(&self) -> PollChoice {
        self.choice
    }

    pub fn disclosure(&self) -> PollDisclosure {
        self.disclosure
    }
}

fn trimmed(text: String) -> String {
    let kept = text.trim();
    if kept.len() == text.len() {
        text
    } else {
        kept.to_owned()
    }
}
