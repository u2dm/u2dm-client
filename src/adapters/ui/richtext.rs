use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;
use std::hash::Hash;
use std::mem;

use ruma::html::{Html, NodeRef};
use slint::{SharedString, StyledText, StyledTextFromMarkdownError};

use super::session::with_session;
use super::{autolink, permalink};
use crate::adapters::body_emoji;
use crate::domain::mention;

const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 4096;
const MAX_MARKDOWN_LEN: usize = 64 * 1024;
const MAX_MEMO_ENTRIES: usize = 512;
const MAX_INLINE_PIECES: usize = 512;
const CELL_GAP: &str = "   ";
const TAB_AS_SPACES: &str = "    ";
const LIST_INDENT: &str = "    ";
const FIRST_LIST_NUMBER: usize = 1;
const BULLET: &str = "\u{2022} ";
const QUOTE_MARKER: &str = "> ";
const DELIMITER_GUARD: &str = "<u></u>";
const EMPTY_ITEM_CONTENT: &str = "<u></u>";
const NBSP: &str = "\u{a0}";

#[derive(Clone)]
pub struct StyledBody {
    pub styled: StyledText,
    pub plain: SharedString,
    pub has_links: bool,
    pub mentions_you: bool,
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
    pub emoji: usize,
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

#[derive(PartialEq, Eq, Hash)]
enum Source {
    Formatted(String, RoomWord),
    Unformatted(String, RoomWord),
}

#[derive(PartialEq, Eq, Hash)]
struct InlineSource {
    html: String,
    room_word: RoomWord,
    drawn: BTreeSet<String>,
}

type Memo<K, T> = HashMap<K, Option<T>>;

#[derive(Default)]
pub struct StyledBodies {
    own_user_id: String,
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
        if bodies.own_user_id != user_id {
            user_id.clone_into(&mut bodies.own_user_id);
            bodies.forget();
        }
    });
}

pub fn styled_body(html: &str, plain_fallback: &str, room_word: RoomWord) -> StyledBody {
    memoised(
        Source::Formatted(html.to_owned(), room_word),
        |bodies| &mut bodies.rendered,
        |own_user_id, _| build(html, Writer::reading_as(own_user_id, room_word)),
    )
    .unwrap_or_else(|| unstyled_body(plain_fallback))
}

pub fn plain_body(text: &str, room_word: RoomWord) -> StyledBody {
    memoised(
        Source::Unformatted(text.to_owned(), room_word),
        |bodies| &mut bodies.rendered,
        |own_user_id, _| build_plain(text, Writer::reading_as(own_user_id, room_word)),
    )
    .unwrap_or_else(|| unstyled_body(text))
}

pub fn inline_body(html: &str, room_word: RoomWord, drawn: BTreeSet<String>) -> Option<InlineBody> {
    let source = InlineSource {
        html: html.to_owned(),
        room_word,
        drawn,
    };
    memoised(
        source,
        |bodies| &mut bodies.inline,
        |own_user_id, source| {
            let writer = Writer::inline_as(own_user_id, room_word, source.drawn.clone());
            build_inline(html, writer)
        },
    )
}

fn memoised<K: Hash + Eq, T: Clone>(
    source: K,
    memo: fn(&mut StyledBodies) -> &mut Memo<K, T>,
    render: impl FnOnce(&str, &K) -> Option<T>,
) -> Option<T> {
    let lookup = with_session(|session| {
        let bodies = &mut session.bodies;
        memo(bodies)
            .get(&source)
            .cloned()
            .ok_or_else(|| bodies.own_user_id.clone())
    });
    let own_user_id = match lookup {
        Ok(hit) => return hit,
        Err(own_user_id) => own_user_id,
    };

    let built = render(&own_user_id, &source);
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
        }),
        Err(e) => {
            tracing::debug!("a formatted message did not render, showing it plain: {e}");
            (!plain.is_empty()).then(|| unstyled_body(plain))
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

    match StyledText::from_markdown(&writer.markdown) {
        Ok(styled) => Some(StyledBody {
            styled,
            plain: SharedString::from(text),
            has_links: writer.has_links,
            mentions_you: writer.mentions_you,
        }),
        Err(e) => {
            tracing::debug!("a message with links did not render, showing it plain: {e}");
            None
        }
    }
}

#[derive(PartialEq, Eq)]
enum InlineStyle {
    Strong,
    Italic,
    Struck,
    Underlined,
    Coloured(String),
}

impl InlineStyle {
    fn opener(&self) -> String {
        match self {
            Self::Strong => guarded("**"),
            Self::Italic => guarded("*"),
            Self::Struck => guarded("~~"),
            Self::Underlined => "<u>".to_owned(),
            Self::Coloured(colour) => format!("<font color=\"{colour}\">"),
        }
    }

    fn closer(&self) -> String {
        match self {
            Self::Strong | Self::Italic | Self::Struck => self.opener(),
            Self::Underlined => "</u>".to_owned(),
            Self::Coloured(_) => "</font>".to_owned(),
        }
    }
}

fn guarded(delimiter: &str) -> String {
    format!("{DELIMITER_GUARD}{delimiter}{DELIMITER_GUARD}")
}

#[derive(Clone, Copy, Default)]
enum ListMarkers {
    #[default]
    DashAndDot,
    StarAndParen,
}

impl ListMarkers {
    fn other(self) -> Self {
        match self {
            Self::DashAndDot => Self::StarAndParen,
            Self::StarAndParen => Self::DashAndDot,
        }
    }

    fn marker(self, number: Option<usize>) -> String {
        match (self, number) {
            (Self::DashAndDot, None) => "- ".to_owned(),
            (Self::StarAndParen, None) => "* ".to_owned(),
            (Self::DashAndDot, Some(n)) => format!("{n}. "),
            (Self::StarAndParen, Some(n)) => format!("{n}) "),
        }
    }
}

struct OpenStyle {
    style: InlineStyle,
    written_on_this_line: bool,
}

enum Content {
    Markdown(String),
    Emoji(String),
}

struct Piece {
    content: Content,
    spaced: bool,
}

impl Piece {
    fn render(self) -> Result<InlinePiece, StyledTextFromMarkdownError> {
        let content = match self.content {
            Content::Markdown(markdown) => {
                InlineContent::Text(StyledText::from_markdown(&markdown)?)
            }
            Content::Emoji(mxc) => InlineContent::Emoji(mxc),
        };
        Ok(InlinePiece {
            content,
            spaced: self.spaced,
        })
    }
}

fn render_line(pieces: Vec<Piece>) -> Result<InlineLine, StyledTextFromMarkdownError> {
    let mut words = String::new();
    let mut emoji = 0;
    let mut gaps = 0;
    for piece in &pieces {
        match &piece.content {
            Content::Markdown(markdown) => {
                words.push_str(DELIMITER_GUARD);
                words.push_str(markdown);
            }
            Content::Emoji(_) => emoji += 1,
        }
        gaps += usize::from(piece.spaced);
    }
    Ok(InlineLine {
        words: StyledText::from_markdown(&words)?,
        pieces: pieces
            .into_iter()
            .map(Piece::render)
            .collect::<Result<_, _>>()?,
        emoji,
        gaps,
    })
}

#[derive(Default)]
struct InlineLines {
    drawn: BTreeSet<String>,
    lines: Vec<Vec<Piece>>,
    pieces: Vec<Piece>,
    pieces_written: usize,
    has_content: bool,
    word_break: bool,
    has_emoji: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Line {
    #[default]
    Empty,
    MarkersOnly,
    HasContent,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
struct Writer<'own> {
    own_user_id: &'own str,
    room_word: RoomWord,
    markdown: String,
    plain: String,
    has_links: bool,
    mentions_you: bool,
    tags_room: bool,
    overflowed: bool,
    nodes: usize,
    line: Line,
    link_open: bool,
    list_depth: usize,
    item_content_column: usize,
    list_markers: ListMarkers,
    quote_depth: usize,
    quotes_on_line: usize,
    open_styles: Vec<OpenStyle>,
    inline: Option<InlineLines>,
}

impl<'own> Writer<'own> {
    fn reading_as(own_user_id: &'own str, room_word: RoomWord) -> Self {
        Self {
            own_user_id,
            room_word,
            ..Self::default()
        }
    }

    fn inline_as(own_user_id: &'own str, room_word: RoomWord, drawn: BTreeSet<String>) -> Self {
        Self {
            inline: Some(InlineLines {
                drawn,
                ..InlineLines::default()
            }),
            ..Self::reading_as(own_user_id, room_word)
        }
    }

    fn exceeded_limits(&mut self) -> bool {
        let pieces = self
            .inline
            .as_ref()
            .map_or(0, |inline| inline.pieces_written);
        self.overflowed |= self.markdown.len() > MAX_MARKDOWN_LEN || pieces > MAX_INLINE_PIECES;
        self.overflowed
    }

    fn document(&mut self, html: &str) {
        for node in Html::parse(html).children() {
            self.node(&node, 0);
        }
        self.finish_line();
    }

    fn node(&mut self, node: &NodeRef, depth: usize) {
        self.nodes += 1;
        if self.nodes > MAX_NODES || depth > MAX_DEPTH {
            self.overflowed = true;
        }
        if self.exceeded_limits() {
            return;
        }

        if let Some(text) = node.as_text() {
            self.text(&text.borrow());
            return;
        }
        let Some(element) = node.as_element() else {
            return;
        };

        match element.name.local.as_ref() {
            "mx-reply" => (),
            "br" => self.finish_line(),
            "hr" => self.block(node, depth, |w, _, _| w.text("---")),
            "img" => self.image(node),
            "b" | "strong" => self.styled(InlineStyle::Strong, node, depth),
            "i" | "em" => self.styled(InlineStyle::Italic, node, depth),
            "del" | "s" | "strike" => self.styled(InlineStyle::Struck, node, depth),
            "u" | "ins" => self.styled(InlineStyle::Underlined, node, depth),
            "span" | "font" => self.coloured(node, depth),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.block(node, depth, |w, n, d| w.styled(InlineStyle::Strong, n, d));
            }
            "code" => self.code_span(collapse_whitespace(&node_text(node)).trim()),
            "pre" => self.block(node, depth, |w, n, _| w.preformatted(n)),
            "a" => self.link(node, depth),
            "blockquote" => self.block(node, depth, Self::quote),
            "ul" | "ol" => self.list(node, depth),
            "th" | "td" => self.cell(node, depth),
            "p" | "div" | "li" | "tr" | "table" | "thead" | "tbody" | "caption" | "details"
            | "summary" => self.block(node, depth, Self::children),
            _ => self.children(node, depth),
        }
    }

    fn children(&mut self, node: &NodeRef, depth: usize) {
        for child in node.children() {
            self.node(&child, depth + 1);
        }
    }

    fn image(&mut self, node: &NodeRef) {
        let emoji = body_emoji::source(node).filter(|mxc| {
            self.inline
                .as_ref()
                .is_some_and(|inline| inline.drawn.contains(mxc))
        });
        match emoji {
            Some(mxc) => self.emoji(mxc),
            None => self.text(&image_text(node)),
        }
    }

    fn emoji(&mut self, mxc: String) {
        self.write_pending_quote_markers();
        self.end_piece();
        self.line = Line::HasContent;
        if let Some(inline) = self.inline.as_mut() {
            inline.pieces.push(Piece {
                content: Content::Emoji(mxc),
                spaced: false,
            });
            inline.pieces_written += 1;
            inline.has_emoji = true;
        }
    }

    fn end_piece(&mut self) {
        if self.inline.is_none() {
            return;
        }
        self.write_closers_before_line_end();
        let markdown = mem::take(&mut self.markdown);
        let Some(inline) = self.inline.as_mut() else {
            return;
        };
        let spaced = mem::take(&mut inline.word_break);
        if mem::take(&mut inline.has_content) {
            inline.pieces.push(Piece {
                content: Content::Markdown(markdown),
                spaced,
            });
            inline.pieces_written += 1;
        }
    }

    fn note_content(&mut self) {
        if let Some(inline) = self.inline.as_mut() {
            inline.has_content = true;
        }
    }

    fn mark_word_break(&mut self) {
        let Some(inline) = self.inline.as_mut() else {
            return;
        };
        if inline.has_content {
            inline.word_break = true;
        } else if let Some(last) = inline.pieces.last_mut() {
            last.spaced = true;
        }
    }

    fn break_pending_word(&mut self) {
        let pending = self.inline.as_ref().is_some_and(|inline| inline.word_break);
        if pending {
            self.end_piece();
        }
    }

    fn start_word(&mut self) {
        self.break_pending_word();
        self.write_pending_openers();
    }

    fn block(&mut self, node: &NodeRef, depth: usize, body: impl Fn(&mut Self, &NodeRef, usize)) {
        self.finish_content_line();
        body(self, node, depth);
        self.finish_content_line();
    }

    fn styled(&mut self, style: InlineStyle, node: &NodeRef, depth: usize) {
        if self.is_open(&style) {
            self.children(node, depth);
            return;
        }

        self.open_styles.push(OpenStyle {
            style,
            written_on_this_line: false,
        });
        self.children(node, depth);
        if let Some(open) = self.open_styles.pop()
            && open.written_on_this_line
        {
            self.markdown.push_str(&open.style.closer());
        }
    }

    fn is_open(&self, style: &InlineStyle) -> bool {
        self.open_styles.iter().any(|open| open.style == *style)
    }

    fn write_pending_prefix(&mut self) {
        self.write_pending_quote_markers();
        self.start_word();
    }

    fn write_pending_quote_markers(&mut self) {
        for _ in self.quotes_on_line..self.quote_depth {
            self.push_marker(QUOTE_MARKER);
            self.plain.push_str(QUOTE_MARKER);
            self.line = Line::MarkersOnly;
        }
        self.quotes_on_line = self.quote_depth;
    }

    fn push_marker(&mut self, marker: &str) {
        if self.inline.is_none() {
            self.push_escaped(marker);
            return;
        }
        let glyphs = marker.trim_end_matches(' ');
        self.break_pending_word();
        self.push_escaped(glyphs);
        if glyphs.len() < marker.len() {
            self.mark_word_break();
        }
    }

    fn write_pending_openers(&mut self) {
        for open in &mut self.open_styles {
            if !open.written_on_this_line {
                self.markdown.push_str(&open.style.opener());
                open.written_on_this_line = true;
            }
        }
    }

    fn write_closers_before_line_end(&mut self) {
        for open in self.open_styles.iter_mut().rev() {
            if open.written_on_this_line {
                self.markdown.push_str(&open.style.closer());
                open.written_on_this_line = false;
            }
        }
    }

    fn spans_one_line(&self, from: usize) -> bool {
        let written = self.markdown.get(from..).unwrap_or_default();
        !written.is_empty() && !written.contains('\n')
    }

    fn coloured(&mut self, node: &NodeRef, depth: usize) {
        let colour = attribute(node, "data-mx-color")
            .or_else(|| attribute(node, "color"))
            .filter(|value| is_hex_colour(value));
        match colour {
            Some(colour) => self.styled(InlineStyle::Coloured(colour), node, depth),
            None => self.children(node, depth),
        }
    }

    fn quote(&mut self, node: &NodeRef, depth: usize) {
        self.quote_depth += 1;
        self.children(node, depth);
        self.quote_depth -= 1;
    }

    fn cell(&mut self, node: &NodeRef, depth: usize) {
        if self.line == Line::HasContent {
            self.markdown.push_str(CELL_GAP);
            self.plain.push_str(CELL_GAP);
        }
        self.children(node, depth);
    }

    fn code_span(&mut self, text: &str) {
        if text.is_empty() || self.exceeded_limits() {
            return;
        }
        self.write_pending_prefix();
        let fence = "`".repeat(longest_backtick_run(text) + 1);
        let padding = if text.starts_with('`') || text.ends_with('`') {
            " "
        } else {
            ""
        };
        write!(self.markdown, "{fence}{padding}{text}{padding}{fence}").ok();
        self.note_content();
        self.plain.push_str(text);
        self.line = Line::HasContent;
    }

    fn preformatted(&mut self, node: &NodeRef) {
        for line in node_text(node).lines() {
            self.code_span(line.replace('\t', TAB_AS_SPACES).trim_end());
            self.finish_content_line();
        }
    }

    fn link(&mut self, node: &NodeRef, depth: usize) {
        let Some(href) = attribute(node, "href").filter(|href| autolink::is_safe_destination(href))
        else {
            self.children(node, depth);
            return;
        };
        let plain_label = collapse_whitespace(&node_text(node)).trim().to_owned();
        if plain_label.is_empty() {
            return;
        }
        if self.link_open {
            self.text(&plain_label);
            return;
        }
        self.write_pending_prefix();
        self.link_to(&href, |w| w.labelled_link(&plain_label, &href));
    }

    fn labelled_link(&mut self, label: &str, href: &str) {
        self.link_open = true;
        let bracket_at = self.markdown.len();
        self.markdown.push('[');
        self.text(label);
        self.close_link(href);
        self.link_open = false;

        if self.spans_one_line(bracket_at + 1) {
            self.has_links = true;
        } else {
            self.markdown.replace_range(bracket_at..=bracket_at, "");
        }
    }

    fn link_to(&mut self, destination: &str, write_link: impl FnOnce(&mut Self)) {
        if self.names_you(destination) {
            self.bold_mention(write_link);
        } else {
            write_link(self);
        }
    }

    fn bold_mention(&mut self, write_mention: impl FnOnce(&mut Self)) {
        self.mentions_you = true;
        if self.is_open(&InlineStyle::Strong) {
            write_mention(self);
            return;
        }
        self.markdown.push_str(&InlineStyle::Strong.opener());
        write_mention(self);
        self.markdown.push_str(&InlineStyle::Strong.closer());
    }

    fn names_you(&self, destination: &str) -> bool {
        !self.own_user_id.is_empty()
            && permalink::linked_user(destination).is_some_and(|user| *user == *self.own_user_id)
    }

    fn list(&mut self, node: &NodeRef, depth: usize) {
        if self.list_depth >= MAX_DEPTH {
            self.children(node, depth);
            return;
        }
        let mut number = has_name(node, "ol").then(|| list_start(node));
        let top_level = self.list_depth == 0;

        self.finish_line();
        if top_level {
            self.leave_blank_line();
        }
        self.list_depth += 1;
        for child in node.children() {
            if !has_name(&child, "li") {
                continue;
            }
            self.item(&child, depth + 1, number);
            number = number.map(|n| n.saturating_add(1));
        }
        self.list_depth -= 1;
        self.finish_line();
        if top_level {
            self.leave_blank_line();
            self.list_markers = self.list_markers.other();
        }
    }

    fn leave_blank_line(&mut self) {
        if self.inline.is_none() && !self.markdown.is_empty() && !self.markdown.ends_with("\n\n") {
            self.markdown.push('\n');
        }
    }

    fn item(&mut self, node: &NodeRef, depth: usize, number: Option<usize>) {
        self.finish_line();
        let parent_content_column = self.item_content_column;
        self.write_item_marker(number);
        self.children(node, depth);
        self.item_content_column = parent_content_column;
        self.finish_line();
    }

    fn write_item_marker(&mut self, number: Option<usize>) {
        self.write_pending_quote_markers();
        let plain_indent = LIST_INDENT.repeat(self.list_depth.saturating_sub(1));
        let plain_marker = number.map_or_else(|| BULLET.to_owned(), |n| format!("{n}. "));
        if self.inline.is_some() {
            self.break_pending_word();
            self.push_escaped(&plain_indent.replace(' ', NBSP));
            self.push_marker(&plain_marker);
        } else if self.quotes_on_line > 0 {
            self.push_escaped(&plain_indent);
            self.push_escaped(&plain_marker);
        } else {
            let markdown_indent = " ".repeat(self.item_content_column);
            let markdown_marker = self.list_markers.marker(number);
            self.markdown.push_str(&markdown_indent);
            self.markdown.push_str(&markdown_marker);
            self.item_content_column = markdown_indent.len() + markdown_marker.len();
        }
        self.plain.push_str(&plain_indent);
        self.plain.push_str(&plain_marker);
        self.line = Line::MarkersOnly;
    }

    fn text(&mut self, raw: &str) {
        if self.exceeded_limits() {
            return;
        }
        let collapsed = collapse_whitespace(raw);
        let text = if self.line == Line::HasContent {
            collapsed.as_str()
        } else {
            collapsed.trim_start()
        };
        if text.is_empty() {
            return;
        }

        self.write_pending_prefix();
        if self.link_open {
            self.push_escaped(text);
        } else {
            self.push_linkified(text);
        }
        self.plain.push_str(text);
        self.line = Line::HasContent;
    }

    fn push_linkified(&mut self, text: &str) {
        let mut written = 0;
        for (span, destination) in autolink::find(text) {
            let (Some(before), Some(label)) =
                (text.get(written..span.start), text.get(span.clone()))
            else {
                continue;
            };
            self.push_words(before);
            self.start_word();
            self.link_to(&destination, |w| {
                w.markdown.push('[');
                w.push_escaped(label);
                w.close_link(&destination);
            });
            self.has_links = true;
            written = span.end;
        }
        self.push_words(text.get(written..).unwrap_or_default());
    }

    fn push_words(&mut self, text: &str) {
        if self.room_word == RoomWord::Text {
            self.push_spaced(text);
            return;
        }
        let mut written = 0;
        for span in mention::room_mentions(text) {
            let (Some(before), Some(tag)) = (text.get(written..span.start), text.get(span.clone()))
            else {
                continue;
            };
            self.push_spaced(before);
            self.tags_room = true;
            self.start_word();
            self.bold_mention(|w| w.push_escaped(tag));
            written = span.end;
        }
        self.push_spaced(text.get(written..).unwrap_or_default());
    }

    fn push_spaced(&mut self, text: &str) {
        if self.inline.is_none() {
            self.push_escaped(text);
            return;
        }
        for (index, word) in text.split(' ').enumerate() {
            if index > 0 {
                self.mark_word_break();
            }
            if !word.is_empty() {
                self.start_word();
                self.push_escaped(word);
            }
        }
    }

    fn close_link(&mut self, destination: &str) {
        self.markdown.push_str("](<");
        self.push_escaped(destination);
        self.markdown.push_str(">)");
    }

    fn push_escaped(&mut self, text: &str) {
        for ch in text.chars() {
            if ch.is_ascii_punctuation() {
                self.markdown.push('\\');
            }
            self.markdown.push(ch);
        }
        if !text.is_empty() {
            self.note_content();
        }
    }

    fn finish_inline_line(&mut self) {
        if let Some(inline) = self.inline.as_mut() {
            inline.word_break = false;
        }
        self.end_piece();
        if let Some(inline) = self.inline.as_mut() {
            let mut line = mem::take(&mut inline.pieces);
            if let Some(last) = line.last_mut() {
                last.spaced = false;
            }
            inline.lines.push(line);
        }
    }

    fn finish_content_line(&mut self) {
        if self.line == Line::HasContent {
            self.finish_line();
        }
    }

    fn finish_line(&mut self) {
        if self.line == Line::Empty {
            return;
        }
        if self.inline.is_some() {
            self.finish_inline_line();
        } else {
            if self.line == Line::MarkersOnly {
                self.markdown.push_str(EMPTY_ITEM_CONTENT);
            }
            self.write_closers_before_line_end();
            self.markdown.push('\n');
        }
        self.plain.push('\n');
        self.line = Line::Empty;
        self.quotes_on_line = 0;
    }
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    if pending_space {
        out.push(' ');
    }
    out
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

fn is_hex_colour(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 3 | 4 | 6 | 8) && digits.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn list_start(node: &NodeRef) -> usize {
    attribute(node, "start")
        .and_then(|start| start.trim().parse().ok())
        .unwrap_or(FIRST_LIST_NUMBER)
}

fn has_name(node: &NodeRef, name: &str) -> bool {
    node.as_element()
        .is_some_and(|element| element.name.local.as_ref() == name)
}

fn node_text(node: &NodeRef) -> String {
    let mut out = String::new();
    collect_text(node, &mut out, 0);
    out
}

fn collect_text(node: &NodeRef, out: &mut String, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Some(text) = node.as_text() {
        out.push_str(&text.borrow());
        return;
    }
    if has_name(node, "br") {
        out.push('\n');
        return;
    }
    for child in node.children() {
        collect_text(&child, out, depth + 1);
    }
}

fn image_text(node: &NodeRef) -> String {
    attribute(node, "alt")
        .or_else(|| attribute(node, "title"))
        .unwrap_or_default()
}

fn attribute(node: &NodeRef, name: &str) -> Option<String> {
    let element = node.as_element()?;
    let attrs = element.attrs.borrow();
    attrs
        .iter()
        .find(|attr| attr.name.local.as_ref() == name)
        .map(|attr| attr.value.to_string())
        .filter(|value| !value.is_empty())
}
