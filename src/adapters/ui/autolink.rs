use std::ops::Range;

use url::Url;

use super::permalink;
use crate::domain::mention::{candidates, is_named_host, is_user_tag};

const SCHEMES: &[&str] = &["https://", "http://", "mailto:"];
const HOST_PREFIX: &str = "www.";
const LOCAL_PART_PUNCTUATION: &[char] = &['.', '_', '%', '+', '-'];

pub fn find(text: &str) -> Vec<(Range<usize>, String)> {
    candidates(text)
        .into_iter()
        .filter_map(|(span, candidate)| Some((span, destination(candidate)?)))
        .collect()
}

pub fn is_safe_destination(href: &str) -> bool {
    !href.is_empty()
        && !href
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '<' | '>' | '\\'))
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
    user_mention(candidate).or_else(|| mailbox(candidate))
}

fn user_mention(candidate: &str) -> Option<String> {
    if !is_user_tag(candidate) {
        return None;
    }
    accepted(permalink::user_link(candidate)?, false)
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

fn starts_with_ignoring_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}
