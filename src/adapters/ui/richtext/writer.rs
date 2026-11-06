use std::collections::BTreeSet;
use std::mem;

use slint::{StyledText, StyledTextFromMarkdownError};

use super::html::collapse_whitespace;
use super::{
    CoverLabels, InlineContent, InlineLine, InlinePiece, MAX_INLINE_PIECES, MAX_MARKDOWN_LEN,
    Reading, RoomWord, Spoilers, emote_lead,
};
use crate::adapters::ui::autolink;
use crate::domain::mention;

pub(super) const CELL_GAP: &str = "   ";

pub(super) const TAB_AS_SPACES: &str = "    ";

pub(super) const LIST_INDENT: &str = "    ";

pub(super) const FIRST_LIST_NUMBER: usize = 1;

pub(super) const BULLET: &str = "\u{2022} ";

const QUOTE_MARKER: &str = "> ";

const DELIMITER_GUARD: &str = "<u></u>";

const EMPTY_ITEM_CONTENT: &str = "<u></u>";

pub(super) const NBSP: &str = "\u{a0}";

#[derive(PartialEq, Eq)]
pub(super) enum InlineStyle {
    Strong,
    Italic,
    Struck,
    Underlined,
    Coloured(String),
}

impl InlineStyle {
    pub(super) fn opener(&self) -> String {
        match self {
            Self::Strong => guarded("**"),
            Self::Italic => guarded("*"),
            Self::Struck => guarded("~~"),
            Self::Underlined => "<u>".to_owned(),
            Self::Coloured(colour) => format!("<font color=\"{colour}\">"),
        }
    }

    pub(super) fn closer(&self) -> String {
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
pub(super) enum ListMarkers {
    #[default]
    DashAndDot,
    StarAndParen,
}

impl ListMarkers {
    pub(super) fn other(self) -> Self {
        match self {
            Self::DashAndDot => Self::StarAndParen,
            Self::StarAndParen => Self::DashAndDot,
        }
    }

    pub(super) fn marker(self, number: Option<usize>) -> String {
        match (self, number) {
            (Self::DashAndDot, None) => "- ".to_owned(),
            (Self::StarAndParen, None) => "* ".to_owned(),
            (Self::DashAndDot, Some(n)) => format!("{n}. "),
            (Self::StarAndParen, Some(n)) => format!("{n}) "),
        }
    }
}

pub(super) struct OpenStyle {
    pub(super) style: InlineStyle,
    pub(super) written_on_this_line: bool,
}

pub(super) enum Content {
    Markdown(String),
    Emoji(String),
}

pub(super) struct Piece {
    pub(super) content: Content,
    pub(super) spaced: bool,
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

pub(super) fn render_line(pieces: Vec<Piece>) -> Result<InlineLine, StyledTextFromMarkdownError> {
    let mut words = String::new();
    let mut gaps = 0;
    for piece in &pieces {
        if let Content::Markdown(markdown) = &piece.content {
            words.push_str(DELIMITER_GUARD);
            words.push_str(markdown);
        }
        gaps += usize::from(piece.spaced);
    }
    Ok(InlineLine {
        words: StyledText::from_markdown(&words)?,
        pieces: pieces
            .into_iter()
            .map(Piece::render)
            .collect::<Result<_, _>>()?,
        gaps,
    })
}

#[derive(Default)]
pub(super) struct InlineLines {
    pub(super) drawn: BTreeSet<String>,
    pub(super) lines: Vec<Vec<Piece>>,
    pub(super) pieces: Vec<Piece>,
    pub(super) pieces_written: usize,
    has_content: bool,
    word_break: bool,
    pub(super) has_emoji: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Line {
    #[default]
    Empty,
    MarkersOnly,
    HasContent,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
pub(super) struct Writer<'own> {
    pub(super) own_user_id: &'own str,
    pub(super) covers: CoverLabels,
    room_word: RoomWord,
    pub(super) spoilers: Spoilers,
    pub(super) markdown: String,
    pub(super) plain: String,
    pub(super) has_links: bool,
    pub(super) mentions_you: bool,
    pub(super) has_spoilers: bool,
    pub(super) tags_room: bool,
    pub(super) overflowed: bool,
    pub(super) nodes: usize,
    pub(super) line: Line,
    pub(super) link_open: bool,
    pub(super) list_depth: usize,
    pub(super) item_content_column: usize,
    pub(super) list_markers: ListMarkers,
    pub(super) quote_depth: usize,
    pub(super) quotes_on_line: usize,
    pub(super) open_styles: Vec<OpenStyle>,
    pub(super) inline: Option<InlineLines>,
}

impl<'own> Writer<'own> {
    pub(super) fn reading_as(
        reading: &'own Reading,
        room_word: RoomWord,
        spoilers: Spoilers,
    ) -> Self {
        Self {
            own_user_id: &reading.own_user_id,
            covers: reading.covers.clone(),
            room_word,
            spoilers,
            ..Self::default()
        }
    }

    pub(super) fn inline_as(
        reading: &'own Reading,
        room_word: RoomWord,
        spoilers: Spoilers,
        drawn: BTreeSet<String>,
    ) -> Self {
        Self {
            inline: Some(InlineLines {
                drawn,
                ..InlineLines::default()
            }),
            ..Self::reading_as(reading, room_word, spoilers)
        }
    }

    pub(super) fn acted_by(mut self, actor: Option<&str>) -> Self {
        if let Some(actor) = actor {
            let lead = emote_lead(actor);
            self.push_spaced(&lead);
            self.plain.push_str(&lead);
            self.line = Line::MarkersOnly;
        }
        self
    }

    pub(super) fn exceeded_limits(&mut self) -> bool {
        let pieces = self
            .inline
            .as_ref()
            .map_or(0, |inline| inline.pieces_written);
        self.overflowed |= self.markdown.len() > MAX_MARKDOWN_LEN || pieces > MAX_INLINE_PIECES;
        self.overflowed
    }

    pub(super) fn end_piece(&mut self) {
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

    pub(super) fn note_content(&mut self) {
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

    pub(super) fn break_pending_word(&mut self) {
        let pending = self.inline.as_ref().is_some_and(|inline| inline.word_break);
        if pending {
            self.end_piece();
        }
    }

    fn start_word(&mut self) {
        self.break_pending_word();
        self.write_pending_openers();
    }

    pub(super) fn is_open(&self, style: &InlineStyle) -> bool {
        self.open_styles.iter().any(|open| open.style == *style)
    }

    pub(super) fn write_pending_prefix(&mut self) {
        self.write_pending_quote_markers();
        self.start_word();
    }

    pub(super) fn write_pending_quote_markers(&mut self) {
        for _ in self.quotes_on_line..self.quote_depth {
            self.push_marker(QUOTE_MARKER);
            self.plain.push_str(QUOTE_MARKER);
            self.line = Line::MarkersOnly;
        }
        self.quotes_on_line = self.quote_depth;
    }

    pub(super) fn push_marker(&mut self, marker: &str) {
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

    pub(super) fn spans_one_line(&self, from: usize) -> bool {
        let written = self.markdown.get(from..).unwrap_or_default();
        !written.is_empty() && !written.contains('\n')
    }

    pub(super) fn leave_blank_line(&mut self) {
        if self.inline.is_none() && !self.markdown.is_empty() && !self.markdown.ends_with("\n\n") {
            self.markdown.push('\n');
        }
    }

    pub(super) fn text(&mut self, raw: &str) {
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

    pub(super) fn push_linkified(&mut self, text: &str) {
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

    pub(super) fn close_link(&mut self, destination: &str) {
        self.markdown.push_str("](<");
        self.push_escaped(destination);
        self.markdown.push_str(">)");
    }

    pub(super) fn push_escaped(&mut self, text: &str) {
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

    pub(super) fn finish_content_line(&mut self) {
        if self.line == Line::HasContent {
            self.finish_line();
        }
    }

    pub(super) fn finish_line(&mut self) {
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
