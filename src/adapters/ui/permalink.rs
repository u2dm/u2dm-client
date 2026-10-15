use percent_encoding::percent_decode_str;
use ruma::{
    EventId, OwnedUserId, RoomAliasId, RoomId as MatrixRoomId, ServerName, UserId as MatrixUserId,
};
use url::{Url, form_urlencoded};

use crate::domain::matrix_link::{AddressEntry, MatrixLink, RoomAddress, RoomLink};
use crate::domain::room::RoomId;
use crate::domain::user_info::UserId;

const MATRIX_TO_PREFIX: &str = "https://matrix.to/#/";
const MATRIX_SCHEME: &str = "matrix";
const MATRIX_SCHEME_PREFIX: &str = "matrix:";
const USER_PATH_KINDS: [&str; 2] = ["u", "user"];
const ALIAS_PATH_KINDS: [&str; 2] = ["r", "room"];
const ROOM_ID_PATH_KIND: &str = "roomid";
const EVENT_PATH_KINDS: [&str; 2] = ["e", "event"];
const USER_SIGIL: char = '@';
const ROOM_ID_SIGIL: char = '!';
const ALIAS_SIGIL: char = '#';
const EVENT_SIGIL: char = '$';
const QUERY_START: char = '?';
const PATH_SEPARATOR: char = '/';
const VIA_KEY: &str = "via";
const LINK_VIA_LIMIT: usize = 8;

pub fn matrix_link(destination: &str) -> Option<MatrixLink> {
    matrix_to_link(destination).or_else(|| matrix_uri_link(destination))
}

pub fn linked_user(destination: &str) -> Option<UserId> {
    matrix_link(destination)?.into_user()
}

pub fn user_link(candidate: &str) -> Option<String> {
    Some(parsed_user(candidate)?.matrix_to_uri().to_string())
}

pub fn alias_link(candidate: &str) -> Option<String> {
    Some(
        RoomAliasId::parse(candidate)
            .ok()?
            .matrix_to_uri()
            .to_string(),
    )
}

pub fn room_address_entry(text: impl AsRef<str>) -> AddressEntry {
    let text = text.as_ref().trim();
    if let Some(MatrixLink::Room(link)) = matrix_link(text) {
        return AddressEntry::Room(link);
    }
    room_address(text).map_or(AddressEntry::Unrecognized, |room| {
        AddressEntry::Room(RoomLink::to_room(room))
    })
}

fn matrix_to_link(destination: &str) -> Option<MatrixLink> {
    let route = destination.strip_prefix(MATRIX_TO_PREFIX)?;
    let (path, query) = route.split_once(QUERY_START).unwrap_or((route, ""));
    let path = path.trim_end_matches(PATH_SEPARATOR);
    let (identifier, event) = match path.split_once(PATH_SEPARATOR) {
        Some((identifier, event)) => (identifier, Some(decoded(event)?)),
        None => (path, None),
    };
    let identifier = decoded(identifier)?;
    match identifier.chars().next()? {
        USER_SIGIL if event.is_none() => valid_user(&identifier).map(MatrixLink::User),
        ROOM_ID_SIGIL | ALIAS_SIGIL => room_link(&identifier, event, query).map(MatrixLink::Room),
        _ => None,
    }
}

fn matrix_uri_link(destination: &str) -> Option<MatrixLink> {
    let scheme = destination.get(..MATRIX_SCHEME_PREFIX.len())?;
    if !scheme.eq_ignore_ascii_case(MATRIX_SCHEME_PREFIX) {
        return None;
    }
    let uri = Url::parse(destination).ok()?;
    if uri.scheme() != MATRIX_SCHEME {
        return None;
    }
    let mut segments = uri
        .path()
        .trim_matches(PATH_SEPARATOR)
        .split(PATH_SEPARATOR);
    let kind = segments.next()?;
    let identifier = decoded(segments.next()?)?;
    let event = match (segments.next(), segments.next()) {
        (None, _) => None,
        (Some(event_kind), Some(event)) if EVENT_PATH_KINDS.contains(&event_kind) => {
            Some(format!("{EVENT_SIGIL}{}", decoded(event)?))
        }
        _ => return None,
    };
    if segments.next().is_some() {
        return None;
    }
    let query = uri.query().unwrap_or_default();
    if USER_PATH_KINDS.contains(&kind) {
        if event.is_some() {
            return None;
        }
        return valid_user(&format!("{USER_SIGIL}{identifier}")).map(MatrixLink::User);
    }
    let sigil = if ALIAS_PATH_KINDS.contains(&kind) {
        ALIAS_SIGIL
    } else if kind == ROOM_ID_PATH_KIND {
        ROOM_ID_SIGIL
    } else {
        return None;
    };
    room_link(&format!("{sigil}{identifier}"), event, query).map(MatrixLink::Room)
}

fn room_link(room: &str, event: Option<String>, query: &str) -> Option<RoomLink> {
    let room = room_address(room)?;
    let event = match event {
        Some(event) => Some(event_id(&event)?),
        None => None,
    };
    Some(RoomLink {
        room,
        event,
        via: via_servers(query),
    })
}

fn room_address(candidate: &str) -> Option<RoomAddress> {
    if !is_printable(candidate) {
        return None;
    }
    if candidate.starts_with(ALIAS_SIGIL) {
        let alias = RoomAliasId::parse(candidate).ok()?;
        return Some(RoomAddress::Alias(alias.to_string()));
    }
    let room_id = MatrixRoomId::parse(candidate).ok()?;
    Some(RoomAddress::Id(RoomId::new(room_id.as_str())))
}

fn event_id(candidate: &str) -> Option<String> {
    if !is_printable(candidate) {
        return None;
    }
    EventId::parse(candidate)
        .ok()
        .map(|event_id| event_id.to_string())
}

fn via_servers(query: &str) -> Vec<String> {
    form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key == VIA_KEY)
        .filter_map(|(_, server)| ServerName::parse(server.as_ref()).ok())
        .map(|server| server.to_string())
        .take(LINK_VIA_LIMIT)
        .collect()
}

fn is_printable(identifier: &str) -> bool {
    !identifier
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
}

fn decoded(raw: &str) -> Option<String> {
    percent_decode_str(raw).decode_utf8().ok().map(String::from)
}

fn valid_user(candidate: &str) -> Option<UserId> {
    parsed_user(candidate).map(|user| UserId::new(user.as_str()))
}

fn parsed_user(candidate: &str) -> Option<OwnedUserId> {
    MatrixUserId::parse(candidate)
        .ok()
        .filter(|user| !user.localpart().is_empty())
}
