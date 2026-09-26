use std::collections::HashMap;
use std::env;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::catalog::{Flag, Scenarios};
use crate::domain::message::{MessageBody, TimelineMessage};
use crate::domain::poll::{Poll, PollAnswer, PollDraft, PollPermissions, PollStatus, Voter};

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
            value: "crowd",
            effect: "forty more voters on the newest poll's first answer, so its voter list names ten and counts the rest",
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
            value: "edit-fails",
            effect: "an own poll edit is refused a second later; the row keeps the edit, as the SDK cannot take one back",
            note: "excluded from `all`",
        },
        Flag {
            value: "barred",
            effect: "every room's power levels deny votes, poll ends and new polls, as in an announcement channel",
            note: "excluded from `all`",
        },
        Flag {
            value: "revoked",
            effect: "polls are open to vote in until 3s after a room opens, then a room list update bars them",
            note: "excluded from `all`; the change arrives through the room list, as a power-level change does",
        },
        Flag {
            value: "all",
            effect: "late, live, ended, many and crowd",
            note: "excludes vote-fails, end-fails, edit-fails, barred and revoked",
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
pub const REVOKED_AFTER: Duration = Duration::from_secs(3);

static REVOKED: AtomicBool = AtomicBool::new(false);

const MANY_ANSWERS: usize = 20;
const MANY_VOTES_EVERY: usize = 3;
const CROWD_VOTERS: usize = 40;
const CROWD_FIRST_GUEST: usize = 100;
const LIVE_FIRST_GUEST: usize = 200;

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub tallies_arrive_late: bool,
    pub members_keep_voting: bool,
    pub newest_poll_ends: bool,
    pub answers_are_many: bool,
    pub voters_are_crowded: bool,
    pub votes_fail: bool,
    pub ends_fail: bool,
    pub edits_fail: bool,
    pub rooms_are_barred: bool,
    pub permissions_get_revoked: bool,
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
        voters_are_crowded = scenario.voters_are_crowded,
        votes_fail = scenario.votes_fail,
        ends_fail = scenario.ends_fail,
        edits_fail = scenario.edits_fail,
        rooms_are_barred = scenario.rooms_are_barred,
        permissions_get_revoked = scenario.permissions_get_revoked,
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
        "crowd" => scenario.voters_are_crowded = true,
        "vote-fails" => scenario.votes_fail = true,
        "end-fails" => scenario.ends_fail = true,
        "edit-fails" => scenario.edits_fail = true,
        "barred" => scenario.rooms_are_barred = true,
        "revoked" => scenario.permissions_get_revoked = true,
        "all" => {
            scenario.tallies_arrive_late = true;
            scenario.members_keep_voting = true;
            scenario.newest_poll_ends = true;
            scenario.answers_are_many = true;
            scenario.voters_are_crowded = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub fn permissions() -> PollPermissions {
    let scenario = scenario();
    if scenario.rooms_are_barred
        || (scenario.permissions_get_revoked && REVOKED.load(Ordering::Relaxed))
    {
        PollPermissions {
            vote: false,
            end: false,
            start: false,
        }
    } else {
        PollPermissions::UNRESTRICTED
    }
}

pub fn revoke() -> bool {
    scenario().permissions_get_revoked && !REVOKED.swap(true, Ordering::Relaxed)
}

fn poll_mut(message: &mut TimelineMessage) -> Option<&mut Poll> {
    match &mut message.body {
        MessageBody::Poll(poll) => Some(poll),
        _ => None,
    }
}

fn guest(number: usize) -> Voter {
    Voter {
        user_id: format!("@guest-{number}:matrix.org"),
        name: Some(format!("Guest {number}")),
        is_own: false,
    }
}

fn own_voter() -> Voter {
    let own = super::data::own_user();
    Voter::new(own.to_owned(), Some(own))
}

pub fn apply_scenario(messages: &mut [TimelineMessage]) {
    name_voters(messages);
    let scenario = scenario();
    let Some(poll) = messages.iter_mut().rev().find_map(poll_mut) else {
        return;
    };
    if scenario.answers_are_many {
        poll.answers = (0..MANY_ANSWERS)
            .map(|n| PollAnswer {
                id: format!("many-{n}"),
                text: format!("answer {n}, which runs long enough to wrap onto a second line"),
                voters: if n % MANY_VOTES_EVERY == 0 {
                    vec![guest(n)]
                } else {
                    Vec::new()
                },
            })
            .collect();
    }
    if scenario.voters_are_crowded
        && let Some(answer) = poll.answers.first_mut()
    {
        answer
            .voters
            .extend((CROWD_FIRST_GUEST..CROWD_FIRST_GUEST + CROWD_VOTERS).map(guest));
    }
}

fn name_voters(messages: &mut [TimelineMessage]) {
    let names: HashMap<String, String> = messages
        .iter()
        .filter(|message| !message.is_own)
        .filter_map(|message| Some((message.sender.clone(), message.sender_display_name.clone()?)))
        .collect();
    for poll in messages.iter_mut().filter_map(poll_mut) {
        for voter in poll
            .answers
            .iter_mut()
            .flat_map(|answer| &mut answer.voters)
        {
            if voter.name.is_none() {
                voter.name = names.get(&voter.user_id).cloned();
            }
        }
    }
}

pub fn strip_tallies(messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    messages
        .iter()
        .map(|message| {
            let mut message = message.clone();
            if let Some(poll) = poll_mut(&mut message) {
                for answer in &mut poll.answers {
                    answer.voters.clear();
                }
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
        .filter(|answer| answer.mine())
        .map(|answer| answer.id.clone())
        .collect()
}

fn select(poll: &mut Poll, selection: &[String]) {
    poll.editable = false;
    for answer in &mut poll.answers {
        let chosen = selection.contains(&answer.id);
        if chosen && !answer.mine() {
            answer.voters.push(own_voter());
        }
        if !chosen {
            answer.voters.retain(|voter| !voter.is_own);
        }
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
    poll.editable = false;
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
    answer
        .voters
        .push(guest(LIVE_FIRST_GUEST.saturating_add(round)));
    poll.editable = false;
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Revised {
    Applied,
    Unchanged,
    Refused,
}

pub fn revise(message: &mut TimelineMessage, draft: &PollDraft, sequence: u64) -> Revised {
    if !message.is_own {
        return Revised::Refused;
    }
    let Some(poll) = poll_mut(message).filter(|poll| poll.editable) else {
        return Revised::Refused;
    };
    let Some(revision) = poll.revise(draft) else {
        return Revised::Unchanged;
    };
    poll.question = revision.question;
    poll.choice = revision.choice;
    poll.disclosure = revision.disclosure;
    poll.answers = revision
        .answers
        .into_iter()
        .enumerate()
        .map(|(index, answer)| PollAnswer {
            id: answer
                .id
                .unwrap_or_else(|| format!("demo-edit-{sequence}-answer-{index}")),
            text: answer.text,
            voters: Vec::new(),
        })
        .collect();
    message.edited = true;
    Revised::Applied
}
