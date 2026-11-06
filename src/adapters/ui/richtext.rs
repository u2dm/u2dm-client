mod html;
mod nodes;
mod writer;

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;

use slint::{SharedString, StyledText};

use self::writer::{Writer, render_line};
use super::session::with_session;

const MAX_DEPTH: usize = 16;

const MAX_NODES: usize = 4096;

const MAX_MARKDOWN_LEN: usize = 64 * 1024;

const MAX_MEMO_ENTRIES: usize = 512;

const MAX_INLINE_PIECES: usize = 512;

const SPOILER_ATTRIBUTE: &str = "data-mx-spoiler";

const REVEAL_LINK: &str = "";

const EMOTE_MARK: &str = "* ";

#[derive(Clone)]
pub struct StyledBody {
    pub styled: StyledText,
    pub plain: SharedString,
    pub has_links: bool,
    pub mentions_you: bool,
    pub has_spoilers: bool,
}

#[derive(Clone)]
pub enum InlineContent {
    Text(StyledText),
    Emoji(String),
}

#[derive(Clone)]
pub struct InlinePiece {
    pub content: InlineContent,
    pub spaced: bool,
}

#[derive(Clone)]
pub struct InlineLine {
    pub pieces: Vec<InlinePiece>,
    pub words: StyledText,
    pub gaps: usize,
}

#[derive(Clone)]
pub struct InlineBody {
    pub lines: Vec<InlineLine>,
    pub emote_only: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RoomWord {
    Tag,
    #[default]
    Text,
}

impl RoomWord {
    pub fn when(tagged: bool) -> Self {
        if tagged { Self::Tag } else { Self::Text }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cover {
    Link,
    Text,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Spoilers {
    Covered(Cover),
    Shown,
}

impl Default for Spoilers {
    fn default() -> Self {
        Self::Covered(Cover::Text)
    }
}

#[derive(Clone, Default)]
pub struct CoverLabels {
    pub label: String,
    pub reason_label: String,
    pub reason_slot: String,
}

impl CoverLabels {
    fn text_for(&self, reason: Option<&str>) -> String {
        match reason {
            Some(reason) if self.shows_reasons() => {
                self.reason_label.replacen(&self.reason_slot, reason, 1)
            }
            _ => self.label.clone(),
        }
    }

    fn shows_reasons(&self) -> bool {
        !self.reason_slot.is_empty() && self.reason_label.contains(&self.reason_slot)
    }
}

#[derive(PartialEq, Eq, Hash)]
enum Source {
    Formatted {
        html: String,
        actor: Option<String>,
        room_word: RoomWord,
        spoilers: Spoilers,
    },
    Unformatted {
        text: String,
        actor: Option<String>,
        room_word: RoomWord,
    },
}

#[derive(PartialEq, Eq, Hash)]
struct InlineSource {
    html: String,
    actor: Option<String>,
    room_word: RoomWord,
    spoilers: Spoilers,
    drawn: BTreeSet<String>,
}

type Memo<K, T> = HashMap<K, Option<T>>;

#[derive(Clone, Default)]
struct Reading {
    own_user_id: String,
    covers: CoverLabels,
}

#[derive(Default)]
pub struct StyledBodies {
    reading: Reading,
    rendered: Memo<Source, StyledBody>,
    inline: Memo<InlineSource, InlineBody>,
}

impl StyledBodies {
    fn forget(&mut self) {
        self.rendered.clear();
        self.inline.clear();
    }
}

pub fn forget_styled_bodies() {
    with_session(|session| session.bodies.forget());
}

pub fn recognise_own_user(user_id: &str) {
    with_session(|session| {
        let bodies = &mut session.bodies;
        if bodies.reading.own_user_id != user_id {
            user_id.clone_into(&mut bodies.reading.own_user_id);
            bodies.forget();
        }
    });
}

pub fn recognise_cover_labels(covers: CoverLabels) {
    with_session(|session| {
        let bodies = &mut session.bodies;
        bodies.reading.covers = covers;
        bodies.forget();
    });
}

pub fn styled_body(
    html: &str,
    plain_fallback: &str,
    actor: Option<&str>,
    room_word: RoomWord,
    spoilers: Spoilers,
) -> StyledBody {
    memoised(
        Source::Formatted {
            html: html.to_owned(),
            actor: actor.map(str::to_owned),
            room_word,
            spoilers,
        },
        |bodies| &mut bodies.rendered,
        |reading, _| {
            build(
                html,
                Writer::reading_as(reading, room_word, spoilers).acted_by(actor),
            )
        },
    )
    .unwrap_or_else(|| unstyled_body(&acted(actor, plain_fallback)))
}

pub fn plain_body(text: &str, actor: Option<&str>, room_word: RoomWord) -> StyledBody {
    memoised(
        Source::Unformatted {
            text: text.to_owned(),
            actor: actor.map(str::to_owned),
            room_word,
        },
        |bodies| &mut bodies.rendered,
        |reading, _| {
            build_plain(
                text,
                Writer::reading_as(reading, room_word, Spoilers::Shown).acted_by(actor),
            )
        },
    )
    .unwrap_or_else(|| unstyled_body(&acted(actor, text)))
}

pub fn inline_body(
    html: &str,
    actor: Option<&str>,
    room_word: RoomWord,
    spoilers: Spoilers,
    drawn: BTreeSet<String>,
) -> Option<InlineBody> {
    let source = InlineSource {
        html: html.to_owned(),
        actor: actor.map(str::to_owned),
        room_word,
        spoilers,
        drawn,
    };
    memoised(
        source,
        |bodies| &mut bodies.inline,
        |reading, source| {
            let writer = Writer::inline_as(reading, room_word, spoilers, source.drawn.clone())
                .acted_by(actor);
            build_inline(html, writer)
        },
    )
}

fn emote_lead(actor: &str) -> String {
    format!("{EMOTE_MARK}{actor} ")
}

fn acted<'a>(actor: Option<&str>, text: &'a str) -> Cow<'a, str> {
    match actor {
        Some(actor) => Cow::Owned(emote_lead(actor) + text),
        None => Cow::Borrowed(text),
    }
}

fn memoised<K: Hash + Eq, T: Clone>(
    source: K,
    memo: fn(&mut StyledBodies) -> &mut Memo<K, T>,
    render: impl FnOnce(&Reading, &K) -> Option<T>,
) -> Option<T> {
    let lookup = with_session(|session| {
        let bodies = &mut session.bodies;
        memo(bodies)
            .get(&source)
            .cloned()
            .ok_or_else(|| bodies.reading.clone())
    });
    let reading = match lookup {
        Ok(hit) => return hit,
        Err(reading) => reading,
    };

    let built = render(&reading, &source);
    with_session(|session| {
        let memo = memo(&mut session.bodies);
        if memo.len() >= MAX_MEMO_ENTRIES {
            memo.clear();
        }
        memo.insert(source, built.clone());
    });
    built
}

fn unstyled_body(text: &str) -> StyledBody {
    StyledBody {
        styled: StyledText::from_plain_text(text),
        plain: SharedString::from(text),
        has_links: false,
        mentions_you: false,
        has_spoilers: false,
    }
}

fn build(html: &str, mut writer: Writer<'_>) -> Option<StyledBody> {
    writer.document(html);

    if writer.exceeded_limits() {
        tracing::debug!("a formatted message exceeded the rich-text limits, showing it plain");
        return None;
    }

    let markdown = writer.markdown.trim();
    let plain = writer.plain.trim();
    if markdown.is_empty() {
        return None;
    }

    match StyledText::from_markdown(markdown) {
        Ok(styled) => Some(StyledBody {
            styled,
            plain: SharedString::from(plain),
            has_links: writer.has_links,
            mentions_you: writer.mentions_you,
            has_spoilers: writer.has_spoilers,
        }),
        Err(e) => {
            tracing::debug!("a formatted message did not render, showing it plain: {e}");
            (!plain.is_empty()).then(|| StyledBody {
                has_spoilers: writer.has_spoilers,
                ..unstyled_body(plain)
            })
        }
    }
}

fn build_inline(html: &str, mut writer: Writer<'_>) -> Option<InlineBody> {
    writer.document(html);

    if writer.exceeded_limits() {
        tracing::debug!("a body with custom emoji is over the inline limits, showing shortcodes");
        return None;
    }

    let emote_only = writer.plain.trim().is_empty();
    let inline = writer.inline?;
    if !inline.has_emoji {
        return None;
    }
    let lines = inline
        .lines
        .into_iter()
        .map(render_line)
        .collect::<Result<Vec<_>, _>>();
    match lines {
        Ok(lines) => Some(InlineBody { lines, emote_only }),
        Err(e) => {
            tracing::debug!("a word beside a custom emoji did not render, showing shortcodes: {e}");
            None
        }
    }
}

fn build_plain(text: &str, mut writer: Writer<'_>) -> Option<StyledBody> {
    if text.len() > MAX_MARKDOWN_LEN {
        return None;
    }

    for (index, line) in text.lines().enumerate() {
        if index > 0 {
            writer.markdown.push('\n');
        }
        writer.push_linkified(line);
    }
    if !(writer.has_links || writer.tags_room) || writer.exceeded_limits() {
        return None;
    }
    writer.plain.push_str(text);

    match StyledText::from_markdown(&writer.markdown) {
        Ok(styled) => Some(StyledBody {
            styled,
            plain: SharedString::from(writer.plain.as_str()),
            has_links: writer.has_links,
            mentions_you: writer.mentions_you,
            has_spoilers: false,
        }),
        Err(e) => {
            tracing::debug!("a message with links did not render, showing it plain: {e}");
            None
        }
    }
}
