use super::rows::row_index;
use crate::domain::room_search::next_starting_with;

pub(super) fn row_with_id<S: AsRef<str>>(ids: impl IntoIterator<Item = S>, id: &str) -> i32 {
    row_index(ids.into_iter().position(|row_id| row_id.as_ref() == id))
}

pub(super) fn row_starting_with<S: AsRef<str>>(names: &[S], prefix: &str, after: i32) -> i32 {
    row_index(next_starting_with(
        names,
        prefix,
        usize::try_from(after).ok(),
    ))
}

pub(super) struct ListedRoom<S> {
    pub id: S,
    pub joined: bool,
    pub unread: bool,
}

pub(super) fn wants_reading(alert: bool, hint: bool, muted: bool) -> bool {
    alert || (hint && !muted)
}

pub(super) fn room_beside<S: AsRef<str>>(
    rows: &[ListedRoom<S>],
    from: &str,
    forward: bool,
    unread_only: bool,
) -> i32 {
    let len = rows.len();
    if len == 0 {
        return -1;
    }
    let (origin, steps) = match rows.iter().position(|row| row.id.as_ref() == from) {
        Some(at) => (at, len - 1),
        None if forward => (len - 1, len),
        None => (0, len),
    };
    let found = (1..=steps)
        .map(|offset| {
            if forward {
                (origin + offset) % len
            } else {
                (origin + len - offset) % len
            }
        })
        .find(|&index| {
            rows.get(index)
                .is_some_and(|row| row.joined && (row.unread || !unread_only))
        });
    row_index(found)
}
