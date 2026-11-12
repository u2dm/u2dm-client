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
