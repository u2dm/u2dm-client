use std::collections::BTreeSet;

const SHARE_SCALE: u16 = 1000;

pub const VOTERS_NAMED: usize = 10;

const POLL_ANSWERS_MIN: usize = 2;
const POLL_ANSWERS_MAX: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollDisclosure {
    Disclosed,
    Undisclosed,
}

impl PollDisclosure {
    pub fn from_hidden(results_hidden: bool) -> Self {
        if results_hidden {
            Self::Undisclosed
        } else {
            Self::Disclosed
        }
    }
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
    Edit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollPermissions {
    pub vote: bool,
    pub end: bool,
    pub start: bool,
}

impl PollPermissions {
    pub const UNRESTRICTED: Self = Self {
        vote: true,
        end: true,
        start: true,
    };
}

impl Default for PollPermissions {
    fn default() -> Self {
        Self::UNRESTRICTED
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollStatus {
    Open,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voter {
    pub user_id: String,
    pub name: Option<String>,
    pub is_own: bool,
}

impl Voter {
    pub fn new(user_id: String, own_user_id: Option<&str>) -> Self {
        Self {
            is_own: own_user_id == Some(user_id.as_str()),
            user_id,
            name: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollAnswer {
    pub id: String,
    pub text: String,
    pub voters: Vec<Voter>,
}

impl PollAnswer {
    pub fn votes(&self) -> usize {
        self.voters.len()
    }

    pub fn mine(&self) -> bool {
        self.voters.iter().any(|voter| voter.is_own)
    }

    pub fn others(&self) -> usize {
        self.voters.iter().filter(|voter| !voter.is_own).count()
    }

    pub fn named(&self) -> impl Iterator<Item = &Voter> {
        self.voters
            .iter()
            .filter(|voter| !voter.is_own)
            .take(VOTERS_NAMED)
    }

    fn named_mut(&mut self) -> impl Iterator<Item = &mut Voter> {
        self.voters
            .iter_mut()
            .filter(|voter| !voter.is_own)
            .take(VOTERS_NAMED)
    }

    pub fn share(&self, voters: usize) -> f32 {
        if voters == 0 {
            return 0.0;
        }
        let scaled = self
            .votes()
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
    pub status: PollStatus,
    pub editable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisedAnswer {
    pub id: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollRevision {
    pub question: String,
    pub answers: Vec<RevisedAnswer>,
    pub choice: PollChoice,
    pub disclosure: PollDisclosure,
}

impl Poll {
    pub fn is_open(&self) -> bool {
        self.status == PollStatus::Open
    }

    pub fn reveals_results(&self) -> bool {
        self.disclosure == PollDisclosure::Disclosed || !self.is_open()
    }

    pub fn voter_count(&self) -> usize {
        self.answers
            .iter()
            .flat_map(|answer| &answer.voters)
            .map(|voter| voter.user_id.as_str())
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn named_voters(&self) -> impl Iterator<Item = &Voter> {
        let reveals = self.reveals_results();
        self.answers
            .iter()
            .filter(move |_| reveals)
            .flat_map(PollAnswer::named)
    }

    pub fn named_voters_mut(&mut self) -> impl Iterator<Item = &mut Voter> {
        let reveals = self.reveals_results();
        self.answers
            .iter_mut()
            .filter(move |_| reveals)
            .flat_map(PollAnswer::named_mut)
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
                .filter(|candidate| (candidate.id == answer_id) != candidate.mine())
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
                PollChoice::Single => !answer.mine(),
                PollChoice::Multiple { .. } => answer.mine() || self.accepts_another(),
            }
    }

    fn accepts_another(&self) -> bool {
        self.answers.iter().filter(|answer| answer.mine()).count() < self.choice.max()
    }

    pub fn is_leading(&self, answer: &PollAnswer) -> bool {
        answer.votes() > 0 && answer.votes() == self.leading_votes()
    }

    fn leading_votes(&self) -> usize {
        self.answers
            .iter()
            .map(PollAnswer::votes)
            .max()
            .unwrap_or_default()
    }

    pub fn revise(&self, draft: &PollDraft) -> Option<PollRevision> {
        let choice = self.revised_choice(draft);
        let restated = self.question == draft.question()
            && self.disclosure == draft.disclosure()
            && self.choice == choice
            && self
                .answers
                .iter()
                .map(|answer| answer.text.as_str())
                .eq(draft.answers().iter().map(String::as_str));
        if restated {
            return None;
        }
        let mut unclaimed: Vec<&PollAnswer> = self.answers.iter().collect();
        let answers = draft
            .answers()
            .iter()
            .map(|text| RevisedAnswer {
                id: unclaimed
                    .iter()
                    .position(|answer| &answer.text == text)
                    .map(|index| unclaimed.remove(index).id.clone()),
                text: text.clone(),
            })
            .collect();
        Some(PollRevision {
            question: draft.question().to_owned(),
            answers,
            choice,
            disclosure: draft.disclosure(),
        })
    }

    fn revised_choice(&self, draft: &PollDraft) -> PollChoice {
        match (self.choice, draft.choice()) {
            (PollChoice::Multiple { max }, PollChoice::Multiple { max: offered })
                if max < self.answers.len() =>
            {
                PollChoice::up_to(max.min(offered))
            }
            (_, choice) => choice,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceMode {
    Single,
    Multiple,
}

impl ChoiceMode {
    pub fn from_multiple(multiple: bool) -> Self {
        if multiple {
            Self::Multiple
        } else {
            Self::Single
        }
    }
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
