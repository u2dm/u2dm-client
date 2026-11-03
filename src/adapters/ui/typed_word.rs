pub const NO_WORD: i32 = -1;
pub const MAX_QUERY_BYTES: usize = 255;
pub const SPACER: &str = " ";

pub struct TypedWord {
    pub start: usize,
    pub query: String,
}

pub fn sigil_word(text: &str, caret: i32, sigil: char) -> Option<(usize, &str)> {
    let before = text.get(..clamped(text, caret))?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let query = before.get(start..)?.strip_prefix(sigil)?;
    Some((start, query))
}

pub fn located(word: Option<TypedWord>) -> (i32, String) {
    match word {
        Some(word) => (i32::try_from(word.start).unwrap_or(NO_WORD), word.query),
        None => (NO_WORD, String::new()),
    }
}

pub fn clamped(text: &str, caret: i32) -> usize {
    let mut caret = usize::try_from(caret).unwrap_or(0).min(text.len());
    while caret > 0 && !text.is_char_boundary(caret) {
        caret -= 1;
    }
    caret
}
