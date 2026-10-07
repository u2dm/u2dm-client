use matrix_sdk::ruma::{OwnedUserId, UserId as MatrixUserId};
use percent_encoding::percent_decode_str;
use url::Url;

use crate::domain::user_info::UserId;

const MATRIX_TO_PREFIX: &str = "https://matrix.to/#/";
const MATRIX_SCHEME: &str = "matrix";
const USER_PATH_KINDS: [&str; 2] = ["u", "user"];
const USER_SIGIL: char = '@';
const QUERY_START: char = '?';
const PATH_SEPARATOR: char = '/';

pub fn linked_user(destination: &str) -> Option<UserId> {
    matrix_to_user(destination).or_else(|| matrix_uri_user(destination))
}

pub fn user_link(candidate: &str) -> Option<String> {
    Some(parsed_user(candidate)?.matrix_to_uri().to_string())
}

fn matrix_to_user(destination: &str) -> Option<UserId> {
    let route = destination.strip_prefix(MATRIX_TO_PREFIX)?;
    let identifier = route.split_once(QUERY_START).map_or(route, |(id, _)| id);
    let identifier = decoded_segment(identifier)?;
    if !identifier.starts_with(USER_SIGIL) {
        return None;
    }
    valid_user(&identifier)
}

fn matrix_uri_user(destination: &str) -> Option<UserId> {
    let uri = Url::parse(destination).ok()?;
    if uri.scheme() != MATRIX_SCHEME {
        return None;
    }
    let (kind, identifier) = uri.path().split_once(PATH_SEPARATOR)?;
    if !USER_PATH_KINDS.contains(&kind) {
        return None;
    }
    let identifier = decoded_segment(identifier)?;
    valid_user(&format!("{USER_SIGIL}{identifier}"))
}

fn decoded_segment(raw: &str) -> Option<String> {
    let segment = raw.strip_suffix(PATH_SEPARATOR).unwrap_or(raw);
    if segment.contains(PATH_SEPARATOR) {
        return None;
    }
    percent_decode_str(segment)
        .decode_utf8()
        .ok()
        .map(String::from)
}

fn valid_user(candidate: &str) -> Option<UserId> {
    parsed_user(candidate).map(|user| UserId::new(user.as_str()))
}

fn parsed_user(candidate: &str) -> Option<OwnedUserId> {
    MatrixUserId::parse(candidate)
        .ok()
        .filter(|user| !user.localpart().is_empty())
}
