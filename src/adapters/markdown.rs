use std::collections::BTreeSet;
use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};
use ruma::events::Mentions;
use ruma::events::room::message::{FormattedBody, TextMessageEventContent};
use ruma::html::{Html, ListBehavior, PropertiesNames, RemoveReplyFallback, SanitizerConfig};
use ruma::{OwnedUserId, UserId};

use super::body_emoji::{self, EMOTICON_ATTRIBUTE};
use crate::domain::emote::Emote;
use crate::domain::mention::{is_user_tag, mentions_room, user_tags};
use crate::domain::message::RichText;

const MARKDOWN_OPTIONS: Options = Options::ENABLE_TABLES.union(Options::ENABLE_STRIKETHROUGH);
const ESCAPE: char = '\\';
const LINKING_PASSES: usize = 4;
const LABEL_OPEN: char = '[';
const LABEL_CLOSE: char = ']';
const DESTINATION_OPEN: char = '(';
const DESTINATION_CLOSE: char = ')';
const TOKEN_DELIMITER: char = ':';
const PLACEHOLDER_OPEN: char = '\u{E000}';
const PLACEHOLDER_CLOSE: char = '\u{E001}';
const DEFAULT_EMOTE_HEIGHT: &str = "32";
const KEPT_ATTRIBUTES: [PropertiesNames<'static>; 1] = [PropertiesNames {
    parent: "img",
    properties: &[EMOTICON_ATTRIBUTE],
}];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomMentions {
    Allowed,
    Denied,
}

impl RoomMentions {
    pub fn when(allowed: bool) -> Self {
        if allowed { Self::Allowed } else { Self::Denied }
    }
}

pub struct Composed {
    pub markdown: String,
    pub html: Option<String>,
    pub mentions: Mentions,
}

impl Composed {
    pub fn content(&self) -> TextMessageEventContent {
        match &self.html {
            Some(html) => TextMessageEventContent::html(self.markdown.clone(), html.clone()),
            None => TextMessageEventContent::plain(self.markdown.clone()),
        }
    }
}

pub fn compose(text: &str, own_user: &str, room: RoomMentions, emotes: &[Emote]) -> Composed {
    let placed = Placed::in_text(text, emotes);
    let linked = Linked::from_text(&placed.text);
    let mut mentions = Mentions::with_user_ids(
        linked
            .users
            .into_iter()
            .filter(|user| user.as_str() != own_user),
    );
    mentions.room = room == RoomMentions::Allowed && linked.room;
    let (markdown, html) = placed.rendered(&linked.markdown);
    Composed {
        markdown,
        html,
        mentions,
    }
}

pub fn sanitized(html: &str) -> String {
    sanitized_with(html, RemoveReplyFallback::No)
}

pub fn sanitized_with(html: &str, reply_fallback: RemoveReplyFallback) -> String {
    let mut config = SanitizerConfig::compat().allow_attributes(KEPT_ATTRIBUTES, ListBehavior::Add);
    if reply_fallback == RemoveReplyFallback::Yes {
        config = config.remove_reply_fallback();
    }
    let parsed = Html::parse(html);
    parsed.sanitize_with(&config);
    parsed.to_string()
}

pub fn composer_text(body: &str, sanitized_html: Option<&str>) -> Option<String> {
    let Some(html) = sanitized_html else {
        return Some(body.to_owned());
    };
    let emotes = body_emoji::emotes_in(html);
    let placed = Placed::in_text(body, &emotes);
    let (_, rendered) = placed.rendered(&placed.text);
    (sanitized(&rendered?) == html).then(|| unlinked(body))
}

pub fn own_text(body: &str, sanitized_html: Option<&str>) -> RichText {
    let Some(html) = sanitized_html else {
        return RichText::plain(body.to_owned());
    };
    match composer_text(body, Some(html)) {
        Some(composer) => RichText::authored(body.to_owned(), html.to_owned(), composer),
        None => RichText::formatted(body.to_owned(), html.to_owned()),
    }
}

pub fn own_caption(caption: RichText) -> RichText {
    let Some(html) = caption.html() else {
        return caption;
    };
    match composer_text(&caption.plain, Some(&sanitized(html))) {
        Some(composer) => RichText::authored(caption.plain.clone(), html.to_owned(), composer),
        None => caption,
    }
}

pub fn merged(original: Option<&Mentions>, added: &Mentions) -> Mentions {
    let mut merged = original.cloned().unwrap_or_default();
    merged.user_ids.extend(added.user_ids.iter().cloned());
    merged.room |= added.room;
    merged
}

struct Placed<'a> {
    text: String,
    emotes: Vec<&'a Emote>,
}

impl<'a> Placed<'a> {
    fn in_text(text: &str, emotes: &'a [Emote]) -> Self {
        let mut placed = Self {
            text: String::with_capacity(text.len()),
            emotes: Vec::new(),
        };
        if emotes.is_empty() || text.contains([PLACEHOLDER_OPEN, PLACEHOLDER_CLOSE]) {
            text.clone_into(&mut placed.text);
            return placed;
        }
        let mut rest = text;
        while let Some(at) = rest.find(TOKEN_DELIMITER) {
            placed.text.push_str(rest.get(..at).unwrap_or_default());
            let from = rest.get(at..).unwrap_or_default();
            let token = emotes
                .iter()
                .filter(|emote| !emote.token.is_empty() && from.starts_with(&emote.token))
                .max_by_key(|emote| emote.token.len());
            if let Some(emote) = token {
                placed.text.push_str(&placeholder(placed.emotes.len()));
                placed.emotes.push(emote);
                rest = from.get(emote.token.len()..).unwrap_or_default();
            } else {
                placed.text.push(TOKEN_DELIMITER);
                rest = from.get(TOKEN_DELIMITER.len_utf8()..).unwrap_or_default();
            }
        }
        placed.text.push_str(rest);
        placed
    }

    fn rendered(&self, linked: &str) -> (String, Option<String>) {
        if self.emotes.is_empty() {
            let html = FormattedBody::markdown(linked).map(|formatted| formatted.body);
            return (linked.to_owned(), html);
        }
        let eligible = eligible_placeholders(linked);
        let body = self.restored(linked, |index, emote| {
            if eligible.contains(&index) {
                emote.body_token()
            } else {
                emote.token.clone()
            }
        });
        if eligible.is_empty() {
            let html = FormattedBody::markdown(&body).map(|formatted| formatted.body);
            return (body, html);
        }
        let source = self.restored(linked, |index, emote| {
            if eligible.contains(&index) {
                placeholder(index)
            } else {
                emote.token.clone()
            }
        });
        let html = self.restored(&render_markdown(&source), |index, emote| {
            if eligible.contains(&index) {
                image_tag(emote)
            } else {
                emote.token.clone()
            }
        });
        (body, Some(html))
    }

    fn restored(&self, text: &str, mut with: impl FnMut(usize, &Emote) -> String) -> String {
        let mut restored = String::with_capacity(text.len());
        let mut copied = 0;
        for (span, index) in placeholders(text) {
            let Some(emote) = self.emotes.get(index) else {
                continue;
            };
            restored.push_str(text.get(copied..span.start).unwrap_or_default());
            restored.push_str(&with(index, emote));
            copied = span.end;
        }
        restored.push_str(text.get(copied..).unwrap_or_default());
        restored
    }
}

fn eligible_placeholders(linked: &str) -> BTreeSet<usize> {
    let runs = visible_runs(linked);
    placeholders(linked)
        .into_iter()
        .filter(|(span, _)| {
            runs.iter()
                .any(|run| run.start <= span.start && span.end <= run.end)
        })
        .map(|(_, index)| index)
        .collect()
}

fn placeholder(index: usize) -> String {
    format!("{TOKEN_DELIMITER}{PLACEHOLDER_OPEN}{index}{PLACEHOLDER_CLOSE}{TOKEN_DELIMITER}")
}

fn placeholders(text: &str) -> Vec<(Range<usize>, usize)> {
    let opener = format!("{TOKEN_DELIMITER}{PLACEHOLDER_OPEN}");
    let closer = format!("{PLACEHOLDER_CLOSE}{TOKEN_DELIMITER}");
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(relative) = text.get(from..).and_then(|rest| rest.find(&opener)) {
        let start = from + relative;
        let digits_from = start + opener.len();
        let Some(close) = text.get(digits_from..).and_then(|rest| rest.find(&closer)) else {
            break;
        };
        let end = digits_from + close + closer.len();
        if let Some(index) = text
            .get(digits_from..digits_from + close)
            .and_then(|digits| digits.parse().ok())
        {
            found.push((start..end, index));
        }
        from = end;
    }
    found
}

fn image_tag(emote: &Emote) -> String {
    let height = emote.height.as_deref().unwrap_or(DEFAULT_EMOTE_HEIGHT);
    let alt = emote
        .alt
        .as_deref()
        .map(|alt| format!(" alt=\"{}\"", attribute(alt)))
        .unwrap_or_default();
    let width = emote
        .width
        .as_deref()
        .map(|width| format!(" width=\"{}\"", attribute(width)))
        .unwrap_or_default();
    format!(
        "<img {EMOTICON_ATTRIBUTE} src=\"{}\"{alt} title=\"{}\" height=\"{}\"{width} />",
        attribute(&emote.mxc),
        attribute(&emote.title),
        attribute(height),
    )
}

fn attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn render_markdown(text: &str) -> String {
    let events: Vec<Event<'_>> = Parser::new_ext(text, MARKDOWN_OPTIONS)
        .map(|event| match event {
            Event::SoftBreak => Event::HardBreak,
            event => event,
        })
        .collect();
    let inline = matches!(events.first(), Some(Event::Start(Tag::Paragraph)))
        && matches!(events.last(), Some(Event::End(TagEnd::Paragraph)))
        && !events
            .iter()
            .skip(1)
            .any(|event| matches!(event, Event::Start(tag) if is_block_tag(tag)));
    let count = events.len();
    let mut rendered = String::new();
    if inline {
        html::push_html(
            &mut rendered,
            events.into_iter().skip(1).take(count.saturating_sub(2)),
        );
    } else {
        html::push_html(&mut rendered, events.into_iter());
    }
    rendered
}

fn is_block_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::List(_)
            | Tag::FootnoteDefinition(_)
            | Tag::Table(_)
    )
}

struct Linked {
    markdown: String,
    users: Vec<OwnedUserId>,
    room: bool,
}

impl Linked {
    fn from_text(text: &str) -> Self {
        let mut linked = Self {
            markdown: text.to_owned(),
            users: Vec::new(),
            room: false,
        };
        for _ in 0..LINKING_PASSES {
            let tags = Tags::in_text(&linked.markdown);
            linked.room |= tags.room;
            if tags.found.is_empty() {
                break;
            }
            linked.markdown = tags.linked(&linked.markdown);
            linked
                .users
                .extend(tags.found.into_iter().map(|(_, user)| user));
        }
        linked
    }
}

#[derive(Default)]
struct Tags {
    found: Vec<(Range<usize>, OwnedUserId)>,
    room: bool,
}

impl Tags {
    fn in_text(text: &str) -> Self {
        let mut tags = Self::default();
        for run in visible_runs(text) {
            tags.scan(text, run);
        }
        tags
    }

    fn scan(&mut self, text: &str, source: Range<usize>) {
        let Some(shown) = text.get(source.clone()) else {
            return;
        };
        self.room |= mentions_room(shown);
        for (span, candidate) in user_tags(shown) {
            let start = source.start + span.start;
            let escaped = text
                .get(..start)
                .is_some_and(|before| before.ends_with(ESCAPE));
            if let (false, Ok(user)) = (escaped, UserId::parse(candidate)) {
                self.found.push((start..source.start + span.end, user));
            }
        }
    }

    fn linked(&self, text: &str) -> String {
        let mut markdown = String::with_capacity(text.len());
        let mut copied = 0;
        for (span, user) in &self.found {
            markdown.push_str(text.get(copied..span.start).unwrap_or_default());
            push_link(&mut markdown, user);
            copied = span.end;
        }
        markdown.push_str(text.get(copied..).unwrap_or_default());
        markdown
    }
}

fn visible_runs(text: &str) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let mut run: Option<Range<usize>> = None;
    let mut hidden = 0_usize;
    for (event, range) in Parser::new_ext(text, MARKDOWN_OPTIONS).into_offset_iter() {
        match event {
            Event::Text(shown) if hidden == 0 => {
                let verbatim = text.get(range.clone()) == Some(&*shown);
                match run.as_mut() {
                    Some(current) if verbatim && current.end == range.start => {
                        current.end = range.end;
                    }
                    _ => {
                        runs.extend(run.take());
                        run = verbatim.then_some(range);
                    }
                }
            }
            Event::Start(tag) => {
                runs.extend(run.take());
                if hides_text(&tag) {
                    hidden += 1;
                }
            }
            Event::End(end) => {
                runs.extend(run.take());
                if ends_hidden_text(end) {
                    hidden = hidden.saturating_sub(1);
                }
            }
            _ => runs.extend(run.take()),
        }
    }
    runs.extend(run);
    runs
}

fn hides_text(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::CodeBlock(_) | Tag::Link { .. } | Tag::Image { .. } | Tag::HtmlBlock
    )
}

fn ends_hidden_text(end: TagEnd) -> bool {
    matches!(
        end,
        TagEnd::CodeBlock | TagEnd::Link | TagEnd::Image | TagEnd::HtmlBlock
    )
}

fn push_link(markdown: &mut String, user: &UserId) {
    markdown.push(LABEL_OPEN);
    markdown.push_str(user.as_str());
    markdown.push(LABEL_CLOSE);
    markdown.push(DESTINATION_OPEN);
    markdown.push_str(&user.matrix_to_uri().to_string());
    markdown.push(DESTINATION_CLOSE);
}

fn unlinked(body: &str) -> String {
    let target = Linked::from_text(body).markdown;
    let all = every_link_unlinked(body);
    if Linked::from_text(&all).markdown == target {
        return all;
    }
    let mut composer = body.to_owned();
    let mut from = 0;
    while let Some((link, label)) = next_user_link(&composer, from) {
        let candidate = replaced(&composer, &link, &label);
        if Linked::from_text(&candidate).markdown == target {
            from = link.start + label.len();
            composer = candidate;
        } else {
            from = link.end;
        }
    }
    composer
}

fn every_link_unlinked(body: &str) -> String {
    let mut composer = body.to_owned();
    let mut from = 0;
    while let Some((link, label)) = next_user_link(&composer, from) {
        from = link.start + label.len();
        composer = replaced(&composer, &link, &label);
    }
    composer
}

fn replaced(text: &str, range: &Range<usize>, with: &str) -> String {
    let mut replaced = String::with_capacity(text.len());
    replaced.push_str(text.get(..range.start).unwrap_or_default());
    replaced.push_str(with);
    replaced.push_str(text.get(range.end..).unwrap_or_default());
    replaced
}

fn next_user_link(text: &str, from: usize) -> Option<(Range<usize>, String)> {
    let mut at = from;
    loop {
        let open = at + text.get(at..)?.find(LABEL_OPEN)?;
        let from_label = text.get(open..)?;
        if let Some((label, after)) = linked_tag(from_label) {
            let end = text.len() - after.len();
            return Some((open..end, label.to_owned()));
        }
        at = open + LABEL_OPEN.len_utf8();
    }
}

fn linked_tag(from_label: &str) -> Option<(&str, &str)> {
    let (label, destination) = from_label
        .strip_prefix(LABEL_OPEN)?
        .split_once(LABEL_CLOSE)?;
    if !is_user_tag(label) {
        return None;
    }
    let user = UserId::parse(label).ok()?;
    let after = destination
        .strip_prefix(DESTINATION_OPEN)?
        .strip_prefix(user.matrix_to_uri().to_string().as_str())?
        .strip_prefix(DESTINATION_CLOSE)?;
    Some((label, after))
}
