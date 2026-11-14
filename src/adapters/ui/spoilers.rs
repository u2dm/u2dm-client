use std::collections::{HashMap, HashSet};

use super::dto::{CoveredBody, REVEALABLE};
use super::richtext::Spoilers;
use super::session::with_session;

#[derive(Default)]
pub struct SpoilerRows {
    covered: HashMap<String, CoveredBody>,
    revealed: HashSet<String>,
}

pub fn spoilers_on(unique_id: &str) -> Spoilers {
    with_session(|session| {
        if session.spoilers.revealed.contains(unique_id) {
            Spoilers::Shown
        } else {
            REVEALABLE
        }
    })
}

pub fn keep_covered_body(unique_id: &str, body: CoveredBody) {
    with_session(|session| {
        session.spoilers.covered.insert(unique_id.to_owned(), body);
    });
}

pub fn mark_row(unique_id: &str, spoilers: Spoilers) -> Option<CoveredBody> {
    with_session(|session| {
        let rows = &mut session.spoilers;
        let body = rows.covered.get(unique_id)?.clone();
        let changed = match spoilers {
            Spoilers::Shown => rows.revealed.insert(unique_id.to_owned()),
            Spoilers::Covered(_) => rows.revealed.remove(unique_id),
        };
        changed.then_some(body)
    })
}

pub fn forget_spoiler_rows() {
    with_session(|session| session.spoilers = SpoilerRows::default());
}
