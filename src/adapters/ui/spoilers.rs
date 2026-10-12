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

pub fn reveal_row(unique_id: &str) -> Option<CoveredBody> {
    with_session(|session| {
        let rows = &mut session.spoilers;
        let body = rows.covered.get(unique_id)?.clone();
        rows.revealed.insert(unique_id.to_owned()).then_some(body)
    })
}

pub fn forget_spoiler_rows() {
    with_session(|session| session.spoilers = SpoilerRows::default());
}
