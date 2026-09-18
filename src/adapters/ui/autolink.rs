use std::ops::Range;

use url::Url;

const OPENING: &[char] = &[
    '(', '[', '{', '<', '"', '\'', '\u{201c}', '\u{2018}', '\u{00ab}',
];
const TRAILING: &[char] = &[
    '.', ',', ';', ':', '!', '?', '"', '\'', '>', '\u{201d}', '\u{2019}', '\u{00bb}',
];
const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
const SCHEMES: &[&str] = &["https://", "http://", "mailto:"];
const HOST_PREFIX: &str = "www.";
const LOCAL_PART_PUNCTUATION: &[char] = &['.', '_', '%', '+', '-'];

pub fn find(text: &str) -> Vec<(Range<usize>, String)> {
    let mut links = Vec::new();
    for (offset, word) in words(text) {
        if let Some((span, destination)) = link_in(word) {
            links.push((offset + span.start..offset + span.end, destination));
        }
    }
    links
}

pub fn is_safe_destination(href: &str) -> bool {
    !href.is_empty()
        && !href
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '<' | '>' | '\\'))
}

fn words(text: &str) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(from) = start.take() {
                words.push((from, text.get(from..index).unwrap_or_default()));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(from) = start {
        words.push((from, text.get(from..).unwrap_or_default()));
    }
    words
}

fn link_in(word: &str) -> Option<(Range<usize>, String)> {
    let opened = word.trim_start_matches(OPENING);
    let start = word.len() - opened.len();
    let candidate = trim_trailing(opened);
    let destination = destination(candidate)?;
    Some((start..start + candidate.len(), destination))
}

fn trim_trailing(candidate: &str) -> &str {
    let mut unmatched =
        BRACKETS.map(|(open, close)| (close, unmatched_closers(candidate, open, close)));
    let mut kept = candidate;
    while let Some(last) = kept.chars().last() {
        let droppable = TRAILING.contains(&last) || take_unmatched_closer(&mut unmatched, last);
        if !droppable {
            break;
        }
        match kept.get(..kept.len() - last.len_utf8()) {
            Some(shorter) => kept = shorter,
            None => break,
        }
    }
    kept
}

fn unmatched_closers(text: &str, open: char, close: char) -> usize {
    let opened = text.chars().filter(|&ch| ch == open).count();
    let closed = text.chars().filter(|&ch| ch == close).count();
    closed.saturating_sub(opened)
}

fn take_unmatched_closer(unmatched: &mut [(char, usize)], last: char) -> bool {
    match unmatched
        .iter_mut()
        .find(|(close, count)| *close == last && *count > 0)
    {
        Some((_, count)) => {
            *count -= 1;
            true
        }
        None => false,
    }
}

fn destination(candidate: &str) -> Option<String> {
    if SCHEMES
        .iter()
        .any(|scheme| starts_with_ignoring_case(candidate, scheme))
    {
        return accepted(candidate.to_owned(), false);
    }
    if starts_with_ignoring_case(candidate, HOST_PREFIX) {
        return accepted(format!("https://{candidate}"), true);
    }
    mailbox(candidate)
}

fn mailbox(candidate: &str) -> Option<String> {
    let (local, domain) = candidate.split_once('@')?;
    let addressable = !local.is_empty()
        && local
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || LOCAL_PART_PUNCTUATION.contains(&ch));
    if !addressable || !is_named_host(domain) {
        return None;
    }
    accepted(format!("mailto:{candidate}"), false)
}

fn accepted(destination: String, needs_named_host: bool) -> Option<String> {
    if !is_safe_destination(&destination) {
        return None;
    }
    let url = Url::parse(&destination).ok()?;
    match url.scheme() {
        "http" | "https" => {
            let host = url.host_str()?;
            if host.is_empty() || (needs_named_host && !is_named_host(host)) {
                return None;
            }
        }
        "mailto" => (),
        _ => return None,
    }
    Some(destination)
}

fn is_named_host(host: &str) -> bool {
    let Some((labels, top_level)) = host.rsplit_once('.') else {
        return false;
    };
    !labels.is_empty()
        && top_level.len() >= 2
        && top_level.chars().all(|ch| ch.is_ascii_alphabetic())
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
}

fn starts_with_ignoring_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}
