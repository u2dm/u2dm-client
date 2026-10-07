use std::ops::Range;
use std::sync::Arc;

use crate::domain::room_info::RosterMember;
use crate::domain::room_search::folded;
use crate::domain::user_info::localpart;

pub const ROOM_MENTION: &str = "@room";

const OPENING: &[char] = &[
    '(', '[', '{', '<', '"', '\'', '\u{201c}', '\u{2018}', '\u{00ab}',
];
const TRAILING: &[char] = &[
    '.', ',', ';', ':', '!', '?', '"', '\'', '>', '\u{201d}', '\u{2019}', '\u{00bb}',
];
const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
const USER_SIGIL: char = '@';
const USER_LOCALPART_PUNCTUATION: &[char] = &['.', '_', '=', '-', '/', '+'];
const SERVER_SEPARATOR: char = ':';
const PORT_SEPARATOR: char = ':';

pub fn candidates(text: &str) -> Vec<(Range<usize>, &str)> {
    words(text)
        .into_iter()
        .map(|(offset, word)| {
            let (span, candidate) = candidate_in(word);
            (offset + span.start..offset + span.end, candidate)
        })
        .collect()
}

pub fn user_tags(text: &str) -> Vec<(Range<usize>, &str)> {
    candidates(text)
        .into_iter()
        .filter(|(_, candidate)| is_user_tag(candidate))
        .collect()
}

pub fn mentions_room(text: &str) -> bool {
    !room_mentions(text).is_empty()
}

pub fn room_mentions(text: &str) -> Vec<Range<usize>> {
    candidates(text)
        .into_iter()
        .filter(|(_, candidate)| *candidate == ROOM_MENTION)
        .map(|(span, _)| span)
        .collect()
}

pub fn is_user_tag(candidate: &str) -> bool {
    let Some((localpart, server)) = candidate
        .strip_prefix(USER_SIGIL)
        .and_then(|rest| rest.split_once(SERVER_SEPARATOR))
    else {
        return false;
    };
    !localpart.is_empty()
        && localpart
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || USER_LOCALPART_PUNCTUATION.contains(&ch))
        && is_server_name(server)
}

pub fn is_named_host(host: &str) -> bool {
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

pub fn ends_tag(ch: char) -> bool {
    TRAILING.contains(&ch) || BRACKETS.iter().any(|&(_, close)| close == ch)
}

fn is_server_name(server: &str) -> bool {
    match server.split_once(PORT_SEPARATOR) {
        Some((host, port)) => is_named_host(host) && port.parse::<u16>().is_ok(),
        None => is_named_host(server),
    }
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

fn candidate_in(word: &str) -> (Range<usize>, &str) {
    let opened = word.trim_start_matches(OPENING);
    let start = word.len() - opened.len();
    let candidate = trim_trailing(opened);
    (start..start + candidate.len(), candidate)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionQuery {
    folded: String,
    names_server: bool,
}

impl MentionQuery {
    pub fn new(raw: &str) -> Self {
        Self {
            folded: folded(raw.trim()),
            names_server: raw.contains(SERVER_SEPARATOR),
        }
    }

    pub fn names_room(&self) -> bool {
        ROOM_MENTION
            .trim_start_matches(USER_SIGIL)
            .starts_with(self.folded.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchTier {
    NameStart,
    WordStart,
    IdStart,
    Anywhere,
}

#[derive(Debug, Clone)]
pub struct MentionCandidate {
    member: Arc<RosterMember>,
    label: String,
    id: String,
}

impl MentionCandidate {
    pub fn new(member: Arc<RosterMember>) -> Self {
        let label = folded(member.label());
        let id = folded(member.user_id.trim_start_matches(USER_SIGIL));
        Self { member, label, id }
    }

    fn tier(&self, query: &MentionQuery) -> Option<MatchTier> {
        let needle = query.folded.as_str();
        if self.label.starts_with(needle) {
            return Some(MatchTier::NameStart);
        }
        if self
            .label
            .split_whitespace()
            .skip(1)
            .any(|word| word.starts_with(needle))
        {
            return Some(MatchTier::WordStart);
        }
        let id_start = if query.names_server {
            self.id.as_str()
        } else {
            localpart(&self.id)
        };
        if id_start.starts_with(needle) {
            return Some(MatchTier::IdStart);
        }
        (self.label.contains(needle) || self.id.contains(needle)).then_some(MatchTier::Anywhere)
    }
}

pub fn rank(
    candidates: &[MentionCandidate],
    query: &MentionQuery,
    recent: &[String],
    limit: usize,
) -> Vec<Arc<RosterMember>> {
    let mut matches: Vec<(MatchTier, usize, &MentionCandidate)> = candidates
        .iter()
        .filter_map(|candidate| {
            let tier = candidate.tier(query)?;
            let recency = recent
                .iter()
                .position(|speaker| *speaker == candidate.member.user_id)
                .unwrap_or(usize::MAX);
            Some((tier, recency, candidate))
        })
        .collect();
    matches.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then_with(|| a.2.label.cmp(&b.2.label))
            .then_with(|| a.2.member.user_id.cmp(&b.2.member.user_id))
    });
    matches
        .into_iter()
        .take(limit)
        .map(|(_, _, candidate)| Arc::clone(&candidate.member))
        .collect()
}
