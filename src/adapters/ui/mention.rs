use crate::domain::mention::ends_tag;

pub const NO_WORD: i32 = -1;

const SIGIL: char = '@';
const SPACER: &str = " ";
const MAX_QUERY_BYTES: usize = 255;
const ID_PUNCTUATION: &[char] = &['.', '_', '=', '-', '/', '+', ':'];

pub struct MentionWord {
    pub start: usize,
    pub query: String,
}

pub fn mention_word(text: &str, caret: i32) -> Option<MentionWord> {
    let caret = clamped(text, caret);
    let before = text.get(..caret)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let query = before.get(start..)?.strip_prefix(SIGIL)?;
    if query.contains(SIGIL) || query.len() > MAX_QUERY_BYTES {
        return None;
    }
    Some(MentionWord {
        start,
        query: query.to_owned(),
    })
}

pub fn word_at(text: &str, caret: i32) -> (i32, String) {
    match mention_word(text, caret) {
        Some(word) => (i32::try_from(word.start).unwrap_or(NO_WORD), word.query),
        None => (NO_WORD, String::new()),
    }
}

pub fn complete(text: &str, caret: i32, insert: &str) -> (String, i32) {
    let caret = clamped(text, caret);
    let Some(word) = mention_word(text, offset(caret)) else {
        return (text.to_owned(), offset(caret));
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
    (completed, offset(caret))
}

fn is_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ID_PUNCTUATION.contains(&ch)
}

fn clamped(text: &str, caret: i32) -> usize {
    let mut caret = usize::try_from(caret).unwrap_or(0).min(text.len());
    while caret > 0 && !text.is_char_boundary(caret) {
        caret -= 1;
    }
    caret
}

fn offset(caret: usize) -> i32 {
    i32::try_from(caret).unwrap_or(i32::MAX)
}
