use crate::domain::room_search::TitleQuery;

const WORD_SEPARATOR: char = ' ';

pub(super) fn matching_entries<S: AsRef<str>>(
    query: &str,
    entries: impl IntoIterator<Item = [S; 3]>,
) -> Vec<i32> {
    let query = TitleQuery::new(query);
    entries
        .into_iter()
        .enumerate()
        .filter(|(_, words)| query.matches(&joined(words)))
        .filter_map(|(index, _)| i32::try_from(index).ok())
        .collect()
}

fn joined<S: AsRef<str>>(words: &[S]) -> String {
    words.iter().fold(String::new(), |mut text, word| {
        if !text.is_empty() {
            text.push(WORD_SEPARATOR);
        }
        text.push_str(word.as_ref());
        text
    })
}
