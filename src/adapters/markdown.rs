use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ruma::events::Mentions;
use ruma::events::room::message::{FormattedBody, TextMessageEventContent};
use ruma::html::{HtmlSanitizerMode, RemoveReplyFallback, sanitize_html};
use ruma::{OwnedUserId, UserId};

use crate::domain::mention::{is_user_tag, mentions_room, user_tags};
use crate::domain::message::RichText;

const MARKDOWN_OPTIONS: Options = Options::ENABLE_TABLES.union(Options::ENABLE_STRIKETHROUGH);
const SANITIZER_MODE: HtmlSanitizerMode = HtmlSanitizerMode::Compat;
const ESCAPE: char = '\\';
const LINKING_PASSES: usize = 4;
const LABEL_OPEN: char = '[';
const LABEL_CLOSE: char = ']';
const DESTINATION_OPEN: char = '(';
const DESTINATION_CLOSE: char = ')';

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

pub fn compose(text: &str, own_user: &str, room: RoomMentions) -> Composed {
    let linked = Linked::from_text(text);
    let html = FormattedBody::markdown(&linked.markdown).map(|formatted| formatted.body);
    let mut mentions = Mentions::with_user_ids(
        linked
            .users
            .into_iter()
            .filter(|user| user.as_str() != own_user),
    );
    mentions.room = room == RoomMentions::Allowed && linked.room;
    Composed {
        markdown: linked.markdown,
        html,
        mentions,
    }
}

pub fn sanitized(html: &str) -> String {
    sanitize_html(html, SANITIZER_MODE, RemoveReplyFallback::No)
}

pub fn composer_text(body: &str, sanitized_html: Option<&str>) -> Option<String> {
    let Some(html) = sanitized_html else {
        return Some(body.to_owned());
    };
    let rendered = FormattedBody::markdown(body)?;
    (sanitized(&rendered.body) == html).then(|| unlinked(body))
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
                            tags.scan(text, run.take());
                            run = verbatim.then_some(range);
                        }
                    }
                }
                Event::Start(tag) => {
                    tags.scan(text, run.take());
                    if hides_text(&tag) {
                        hidden += 1;
                    }
                }
                Event::End(end) => {
                    tags.scan(text, run.take());
                    if ends_hidden_text(end) {
                        hidden = hidden.saturating_sub(1);
                    }
                }
                _ => tags.scan(text, run.take()),
            }
        }
        tags.scan(text, run);
        tags
    }

    fn scan(&mut self, text: &str, run: Option<Range<usize>>) {
        let Some(source) = run else {
            return;
        };
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
