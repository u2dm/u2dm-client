use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use super::catalog::{Flag, Scenarios};
use crate::domain::message::{MessageBody, TimelineMessage};
use crate::domain::poll::{Poll, PollAnswer, PollStatus};

const ENV_VAR: &str = "U2DM_DEMO_POLLS";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "reproduces how votes and poll ends actually arrive from a homeserver",
    combinable: true,
    flags: &[
        Flag {
            value: "late",
            effect: "every tally and end arrives as a Set rather than inside the opening Reset",
            note: "include in ANY poll test; authored votes ride the opening Reset, where the Set path that production always uses never runs once",
        },
        Flag {
            value: "live",
            effect: "another member votes in the newest open poll every 1.5s, twelve times",
            note: "",
        },
        Flag {
            value: "ended",
            effect: "the newest open poll is ended by its creator 3s after the room opens",
            note: "",
        },
        Flag {
            value: "many",
            effect: "twenty long answers on the newest poll",
            note: "",
        },
        Flag {
            value: "vote-fails",
            effect: "an own vote is refused a second after it shows, so it reverts and a toast explains",
            note: "excluded from `all`",
        },
        Flag {
            value: "end-fails",
            effect: "ending a poll is refused a second later; the row stays ended, as the SDK cannot undo an end",
            note: "excluded from `all`",
        },
        Flag {
            value: "all",
            effect: "late, live, ended and many",
            note: "excludes vote-fails and end-fails",
        },
    ],
    notes: &[
        "the newest poll is the one widened, voted in or ended, because a room opens at the bottom",
    ],
};

pub const LATE_INTERVAL: Duration = Duration::from_millis(350);
pub const LIVE_INTERVAL: Duration = Duration::from_millis(1500);
pub const LIVE_ROUNDS: usize = 12;
pub const ENDS_AFTER: Duration = Duration::from_secs(3);
pub const REFUSAL_DELAY: Duration = Duration::from_secs(1);

const MANY_ANSWERS: usize = 20;
const MANY_VOTES_EVERY: usize = 3;

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub tallies_arrive_late: bool,
    pub members_keep_voting: bool,
    pub newest_poll_ends: bool,
    pub answers_are_many: bool,
    pub votes_fail: bool,
    pub ends_fail: bool,
}

pub fn scenario() -> Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    *SCENARIO.get_or_init(from_env)
}

fn from_env() -> Scenario {
    let Ok(raw) = env::var(ENV_VAR) else {
        return Scenario::default();
    };
    let mut scenario = Scenario::default();
    for flag in raw.split(',').map(str::trim) {
        apply(&mut scenario, flag);
    }
    tracing::info!(
        tallies_arrive_late = scenario.tallies_arrive_late,
        members_keep_voting = scenario.members_keep_voting,
        newest_poll_ends = scenario.newest_poll_ends,
        answers_are_many = scenario.answers_are_many,
        votes_fail = scenario.votes_fail,
        ends_fail = scenario.ends_fail,
        "demo mode: reproducing real-account poll timing"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "late" => scenario.tallies_arrive_late = true,
        "live" => scenario.members_keep_voting = true,
        "ended" => scenario.newest_poll_ends = true,
        "many" => scenario.answers_are_many = true,
        "vote-fails" => scenario.votes_fail = true,
        "end-fails" => scenario.ends_fail = true,
        "all" => {
            scenario.tallies_arrive_late = true;
            scenario.members_keep_voting = true;
            scenario.newest_poll_ends = true;
            scenario.answers_are_many = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

fn poll_mut(message: &mut TimelineMessage) -> Option<&mut Poll> {
    match &mut message.body {
        MessageBody::Poll(poll) => Some(poll),
        _ => None,
    }
}

pub fn apply_scenario(messages: &mut [TimelineMessage]) {
    if !scenario().answers_are_many {
        return;
    }
    let Some(poll) = messages.iter_mut().rev().find_map(poll_mut) else {
        return;
    };
    poll.answers = (0..MANY_ANSWERS)
        .map(|n| PollAnswer {
            id: format!("many-{n}"),
            text: format!("answer {n}, which runs long enough to wrap onto a second line"),
            votes: usize::from(n % MANY_VOTES_EVERY == 0),
            mine: false,
        })
        .collect();
    poll.voters = poll.answers.iter().map(|answer| answer.votes).sum();
}

pub fn strip_tallies(messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    messages
        .iter()
        .map(|message| {
            let mut message = message.clone();
            if let Some(poll) = poll_mut(&mut message) {
                for answer in &mut poll.answers {
                    answer.votes = 0;
                    answer.mine = false;
                }
                poll.voters = 0;
                poll.status = PollStatus::Open;
            }
            message
        })
        .collect()
}

pub fn poll_indices(messages: &[TimelineMessage]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.body.poll().is_some())
        .map(|(index, _)| index)
        .collect()
}

pub fn newest_open_poll(messages: &[TimelineMessage]) -> Option<usize> {
    messages
        .iter()
        .rposition(|message| message.body.poll().is_some_and(Poll::is_open))
}

fn selection_of(poll: &Poll) -> Vec<String> {
    poll.answers
        .iter()
        .filter(|answer| answer.mine)
        .map(|answer| answer.id.clone())
        .collect()
}

fn select(poll: &mut Poll, selection: &[String]) {
    let voted_before = poll.answers.iter().any(|answer| answer.mine);
    for answer in &mut poll.answers {
        let mine = selection.contains(&answer.id);
        if mine != answer.mine {
            answer.votes = if mine {
                answer.votes.saturating_add(1)
            } else {
                answer.votes.saturating_sub(1)
            };
            answer.mine = mine;
        }
    }
    let voted_after = poll.answers.iter().any(|answer| answer.mine);
    if voted_after && !voted_before {
        poll.voters = poll.voters.saturating_add(1);
    }
    if voted_before && !voted_after {
        poll.voters = poll.voters.saturating_sub(1);
    }
}

pub fn cast(message: &mut TimelineMessage, answer_id: &str) -> Option<Vec<String>> {
    let poll = poll_mut(message)?;
    let selection = poll.next_selection(answer_id)?;
    let previous = selection_of(poll);
    select(poll, &selection);
    Some(previous)
}

pub fn restore(message: &mut TimelineMessage, selection: &[String]) -> bool {
    let Some(poll) = poll_mut(message) else {
        return false;
    };
    select(poll, selection);
    true
}

pub fn close(message: &mut TimelineMessage) -> bool {
    let Some(poll) = poll_mut(message).filter(|poll| poll.is_open()) else {
        return false;
    };
    poll.status = PollStatus::Ended;
    true
}

pub fn end_own(message: &mut TimelineMessage) -> bool {
    message.is_own && close(message)
}

pub fn member_votes(message: &mut TimelineMessage, round: usize) -> bool {
    let Some(poll) = poll_mut(message).filter(|poll| poll.is_open()) else {
        return false;
    };
    let Some(pick) = round.checked_rem(poll.answers.len()) else {
        return false;
    };
    let Some(answer) = poll.answers.get_mut(pick) else {
        return false;
    };
    answer.votes = answer.votes.saturating_add(1);
    poll.voters = poll.voters.saturating_add(1);
    true
}
