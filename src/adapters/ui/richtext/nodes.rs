use std::fmt::Write;

use ruma::html::{Html, NodeRef};

use super::html::{
    attribute, collapse_whitespace, first_spoiler, has_name, image_text, is_hex_colour, is_spoiler,
    list_start, longest_backtick_run, node_text, spoiler_reason,
};
use super::writer::{
    BULLET, CELL_GAP, Content, InlineStyle, LIST_INDENT, Line, NBSP, OpenStyle, Piece,
    TAB_AS_SPACES, Writer,
};
use super::{Cover, MAX_DEPTH, MAX_NODES, REVEAL_LINK, Spoilers};
use crate::adapters::body_emoji;
use crate::adapters::ui::{autolink, permalink};

impl Writer<'_> {
    pub(super) fn document(&mut self, html: &str) {
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
            "span" if is_spoiler(node) => self.spoiler(node, depth),
            "span" | "font" => self.coloured(node, depth),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.block(node, depth, |w, n, d| w.styled(InlineStyle::Strong, n, d));
            }
            "code" => self.code(node),
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
            body_emoji::is_emoticon(node)
                && self
                    .inline
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

    fn coloured(&mut self, node: &NodeRef, depth: usize) {
        let colour = attribute(node, "data-mx-color")
            .or_else(|| attribute(node, "color"))
            .filter(|value| is_hex_colour(value));
        match colour {
            Some(colour) => self.styled(InlineStyle::Coloured(colour), node, depth),
            None => self.children(node, depth),
        }
    }

    fn spoiler(&mut self, node: &NodeRef, depth: usize) {
        self.has_spoilers = true;
        match self.spoilers {
            Spoilers::Shown => self.coloured(node, depth),
            Spoilers::Covered(cover) => self.cover(spoiler_reason(node).as_deref(), cover),
        }
    }

    fn covers_spoiler_in(&mut self, node: &NodeRef) -> bool {
        let Some(spoiler) = first_spoiler(node, 0) else {
            return false;
        };
        self.has_spoilers = true;
        match self.spoilers {
            Spoilers::Shown => false,
            Spoilers::Covered(cover) => {
                self.cover(spoiler_reason(&spoiler).as_deref(), cover);
                true
            }
        }
    }

    fn cover(&mut self, reason: Option<&str>, cover: Cover) {
        let label = self.covers.text_for(reason);
        match cover {
            Cover::Link => {
                self.write_pending_prefix();
                self.link_around(REVEAL_LINK, |w| w.code_span(&label));
            }
            Cover::Text => self.code_span(&label),
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

    fn code(&mut self, node: &NodeRef) {
        if !self.covers_spoiler_in(node) {
            self.code_span(collapse_whitespace(&node_text(node)).trim());
        }
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
        if self.covers_spoiler_in(node) {
            return;
        }
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
        if self.covers_spoiler_in(node) {
            return;
        }
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
        self.link_around(href, |w| w.text(label));
    }

    fn link_around(&mut self, href: &str, write_label: impl FnOnce(&mut Self)) {
        self.link_open = true;
        let bracket_at = self.markdown.len();
        self.markdown.push('[');
        write_label(self);
        self.close_link(href);
        self.link_open = false;

        if self.spans_one_line(bracket_at + 1) {
            self.has_links = true;
        } else {
            self.markdown.replace_range(bracket_at..=bracket_at, "");
        }
    }

    pub(super) fn link_to(&mut self, destination: &str, write_link: impl FnOnce(&mut Self)) {
        if self.names_you(destination) {
            self.bold_mention(write_link);
        } else {
            write_link(self);
        }
    }

    pub(super) fn bold_mention(&mut self, write_mention: impl FnOnce(&mut Self)) {
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
}
