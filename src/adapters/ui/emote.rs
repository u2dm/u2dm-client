use std::collections::{HashMap, VecDeque};
use std::mem;
use std::sync::Arc;

use super::session::with_session;
use crate::domain::emote::Emote;
use crate::domain::image_pack::{ImagePack, PackId, PackImage, PackTitle};
use crate::util::saturating_i32;

pub const NO_WORD: i32 = -1;

const MAX_EMOTE_BINDINGS: usize = 256;
const TOKEN_DELIMITER: char = ':';
const PACK_DELIMITER: char = '/';
const SLUG_SEPARATOR: char = '-';
const PERSONAL_SLUG: &str = "personal";
const UNNAMED_SLUG: &str = "pack";
const MIN_EMOTE_QUERY: usize = 2;
const MAX_QUERY_BYTES: usize = 255;
const SPACER: &str = " ";
const EMOTE_HEIGHT: &str = "32";

#[derive(Default)]
pub struct EmoteBindings {
    room: String,
    bound: VecDeque<Emote>,
    other_rooms: HashMap<String, VecDeque<Emote>>,
}

pub fn bound_in(text: &str) -> Vec<Emote> {
    with_session(|session| session.emotes.used_in(text))
}

pub fn rebind(room_id: &str, emotes: &[Emote]) {
    with_session(|session| session.emotes.restore(room_id, emotes));
}

pub fn follow_room(room_id: &str) {
    with_session(|session| session.emotes.follow_room(room_id));
}

pub fn place_picked(
    text: &str,
    caret: i32,
    pack_id: &str,
    shortcode: &str,
    replaces_word: bool,
) -> (String, i32) {
    let token = with_session(|session| {
        let packs = session
            .snapshot
            .as_ref()
            .map(|view| Arc::clone(&view.image_packs.packs))?;
        session
            .emotes
            .bind(&packs, &PackId::new(pack_id), shortcode, text)
    });
    match token {
        Some(token) => place(text, caret, &token, replaces_word),
        None => (text.to_owned(), saturating_i32(clamped(text, caret))),
    }
}

impl EmoteBindings {
    pub fn bind(
        &mut self,
        packs: &[ImagePack],
        pack_id: &PackId,
        shortcode: &str,
        text: &str,
    ) -> Option<String> {
        let pack = packs.iter().find(|pack| pack.id == *pack_id)?;
        let image = pack
            .images
            .iter()
            .find(|image| image.shortcode == shortcode && image.usage.offers_emoticon())?;
        let token = self.token_for(packs, pack, image, text);
        let emote = Emote {
            token: token.clone(),
            mxc: image.mxc.clone(),
            title: image.shortcode.clone(),
            alt: Some(image.body.clone()),
            height: Some(EMOTE_HEIGHT.to_owned()),
            width: None,
        };
        self.bound.retain(|bound| bound.token != token);
        self.bound.push_back(emote);
        while self.bound.len() > MAX_EMOTE_BINDINGS {
            self.bound.pop_front();
        }
        Some(token)
    }

    pub fn used_in(&self, text: &str) -> Vec<Emote> {
        self.bound
            .iter()
            .filter(|emote| text.contains(&emote.token))
            .cloned()
            .collect()
    }

    pub fn restore(&mut self, room_id: &str, emotes: &[Emote]) {
        self.follow_room(room_id);
        for emote in emotes {
            if !self.bound.iter().any(|bound| bound.token == emote.token) {
                self.bound.push_back(emote.clone());
            }
        }
        while self.bound.len() > MAX_EMOTE_BINDINGS {
            self.bound.pop_front();
        }
    }

    pub fn follow_room(&mut self, room_id: &str) {
        if self.room != room_id {
            let entered = self.other_rooms.remove(room_id).unwrap_or_default();
            let left_room = mem::replace(&mut self.room, room_id.to_owned());
            let left_bound = mem::replace(&mut self.bound, entered);
            if !left_bound.is_empty() {
                self.other_rooms.insert(left_room, left_bound);
            }
        }
    }

    fn token_for(
        &self,
        packs: &[ImagePack],
        pack: &ImagePack,
        image: &PackImage,
        text: &str,
    ) -> String {
        let plain = wrapped(&image.shortcode);
        if !shared_shortcode(packs, pack, image) && self.free_for(&plain, image, text) {
            return plain;
        }
        let qualified = format!("{}{PACK_DELIMITER}{}", image.shortcode, slug(pack));
        let first = wrapped(&qualified);
        if self.free_for(&first, image, text) {
            return first;
        }
        (2..=MAX_EMOTE_BINDINGS + 2)
            .map(|n| wrapped(&format!("{qualified}{SLUG_SEPARATOR}{n}")))
            .find(|candidate| self.free_for(candidate, image, text))
            .unwrap_or(first)
    }

    fn free_for(&self, token: &str, image: &PackImage, text: &str) -> bool {
        match self.bound.iter().find(|bound| bound.token == token) {
            Some(bound) => bound.mxc == image.mxc && bound.title == image.shortcode,
            None => !text.contains(token),
        }
    }
}

fn shared_shortcode(packs: &[ImagePack], pack: &ImagePack, image: &PackImage) -> bool {
    packs.iter().any(|other| {
        other.id != pack.id
            && other.images.iter().any(|candidate| {
                candidate.shortcode == image.shortcode && candidate.usage.offers_emoticon()
            })
    })
}

fn wrapped(name: &str) -> String {
    format!("{TOKEN_DELIMITER}{name}{TOKEN_DELIMITER}")
}

pub fn slug(pack: &ImagePack) -> String {
    let PackTitle::Named(title) = &pack.title else {
        return PERSONAL_SLUG.to_owned();
    };
    let mut slug = String::with_capacity(title.len());
    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with(SLUG_SEPARATOR) {
            slug.push(SLUG_SEPARATOR);
        }
    }
    let slug = slug.trim_matches(SLUG_SEPARATOR);
    if slug.is_empty() {
        UNNAMED_SLUG.to_owned()
    } else {
        slug.to_owned()
    }
}

pub struct EmoteWord {
    pub start: usize,
    pub query: String,
}

pub fn emote_word(text: &str, caret: i32) -> Option<EmoteWord> {
    let caret = clamped(text, caret);
    let before = text.get(..caret)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let query = before.get(start..)?.strip_prefix(TOKEN_DELIMITER)?;
    if query.len() < MIN_EMOTE_QUERY
        || query.len() > MAX_QUERY_BYTES
        || !query.chars().all(is_query_char)
    {
        return None;
    }
    Some(EmoteWord {
        start,
        query: query.to_owned(),
    })
}

pub fn word_at(text: &str, caret: i32) -> (i32, String) {
    match emote_word(text, caret) {
        Some(word) => (i32::try_from(word.start).unwrap_or(NO_WORD), word.query),
        None => (NO_WORD, String::new()),
    }
}

pub fn place(text: &str, caret: i32, token: &str, replaces_word: bool) -> (String, i32) {
    let caret = clamped(text, caret);
    let (start, end) = match emote_word(text, saturating_i32(caret)).filter(|_| replaces_word) {
        Some(word) => {
            let after = text.get(caret..).unwrap_or_default();
            let run = after
                .find(|ch: char| !is_query_char(ch))
                .unwrap_or(after.len());
            (word.start, caret + run)
        }
        None => (caret, caret),
    };
    let before = text.get(..start).unwrap_or_default();
    let rest = text.get(end..).unwrap_or_default();
    let lead = match before.chars().next_back() {
        Some(previous) if !previous.is_whitespace() => SPACER,
        _ => "",
    };
    let (trail, skipped) = match rest.chars().next() {
        Some(next) if next.is_whitespace() => ("", next.len_utf8()),
        _ => (SPACER, 0),
    };
    let mut placed = String::with_capacity(text.len() + token.len() + 2);
    placed.push_str(before);
    placed.push_str(lead);
    placed.push_str(token);
    placed.push_str(trail);
    placed.push_str(rest);
    let caret = before.len() + lead.len() + token.len() + trail.len() + skipped;
    (placed, saturating_i32(caret))
}

fn is_query_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | PACK_DELIMITER)
}

fn clamped(text: &str, caret: i32) -> usize {
    let mut caret = usize::try_from(caret).unwrap_or(0).min(text.len());
    while caret > 0 && !text.is_char_boundary(caret) {
        caret -= 1;
    }
    caret
}

