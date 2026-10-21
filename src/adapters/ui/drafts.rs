use super::backend::UiBackend;
use super::session::{ActiveBackend, with_session};

type Draft = <ActiveBackend as UiBackend>::Draft;

pub fn keep(room_id: String, draft: Draft) {
    with_session(|session| session.drafts.insert(room_id, draft));
}

pub fn take(room_id: &str) -> Option<Draft> {
    with_session(|session| session.drafts.remove(room_id))
}
