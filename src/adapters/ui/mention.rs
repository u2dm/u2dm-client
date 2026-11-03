use super::typed_word::{MAX_QUERY_BYTES, SPACER, TypedWord, clamped, located, sigil_word};
use crate::domain::mention::ends_tag;
use crate::util::saturating_i32;

const SIGIL: char = '@';
const ID_PUNCTUATION: &[char] = &['.', '_', '=', '-', '/', '+', ':'];

pub fn mention_word(text: &str, caret: i32) -> Option<TypedWord> {
    let (start, query) = sigil_word(text, caret, SIGIL)?;
    if query.contains(SIGIL) || query.len() > MAX_QUERY_BYTES {
        return None;
    }
    Some(TypedWord {
        start,
        query: query.to_owned(),
    })
}

pub fn word_at(text: &str, caret: i32) -> (i32, String) {
    located(mention_word(text, caret))
}

pub fn complete(text: &str, caret: i32, insert: &str) -> (String, i32) {
    let caret = clamped(text, caret);
    let Some(word) = mention_word(text, saturating_i32(caret)) else {
        return (text.to_owned(), saturating_i32(caret));
    };
    let after = text.get(caret..).unwrap_or_default();
    let run = after
        .find(|ch: char| !is_id_char(ch))
        .map_or(after, |end| after.get(..end).unwrap_or_default())
        .trim_end_matches(ends_tag);
    let rest = after.get(run.len()..).unwrap_or_default();
    let (spacer, skipped) = match rest.chars().next() {
        Some(next) if next.is_whitespace() => ("", next.len_utf8()),
        Some(next) if ends_tag(next) => ("", 0),
        _ => (SPACER, 0),
    };
    let mut completed = String::with_capacity(text.len() + insert.len() + spacer.len());
    completed.push_str(text.get(..word.start).unwrap_or_default());
    completed.push_str(insert);
    completed.push_str(spacer);
    completed.push_str(rest);
    let caret = word.start + insert.len() + spacer.len() + skipped;
    (completed, saturating_i32(caret))
}

fn is_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ID_PUNCTUATION.contains(&ch)
}

