use std::collections::HashMap;
use std::fmt::Write;

use matrix_sdk::ruma::html::{Html, NodeRef};
use slint::{SharedString, StyledText};

use super::autolink;
use super::session::with_session;

const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 4096;
const MAX_MARKDOWN_LEN: usize = 64 * 1024;
const MAX_MEMO_ENTRIES: usize = 512;
const CELL_GAP: &str = "   ";
const TAB_AS_SPACES: &str = "    ";
const LIST_INDENT: &str = "    ";
const BULLET: &str = "\u{2022} ";
const DELIMITER_GUARD: &str = "<u></u>";

#[derive(Clone)]
pub struct StyledBody {
    pub styled: StyledText,
    pub plain: SharedString,
    pub has_links: bool,
}

#[derive(PartialEq, Eq, Hash)]
enum Source {
    Formatted(String),
    Unformatted(String),
}

#[derive(Default)]
pub struct StyledBodies(HashMap<Source, Option<StyledBody>>);

pub fn forget_styled_bodies() {
    with_session(|session| session.bodies.0.clear());
}

pub fn styled_body(html: &str, plain_fallback: &str) -> StyledBody {
    rendered(Source::Formatted(html.to_owned()), || build(html))
        .unwrap_or_else(|| unstyled_body(plain_fallback))
}

pub fn plain_body(text: &str) -> StyledBody {
    rendered(Source::Unformatted(text.to_owned()), || build_plain(text))
        .unwrap_or_else(|| unstyled_body(text))
}

fn rendered(source: Source, render: impl FnOnce() -> Option<StyledBody>) -> Option<StyledBody> {
    if let Some(hit) = with_session(|session| session.bodies.0.get(&source).cloned()) {
        return hit;
    }

    let built = render();
    remember(source, built.clone());
    built
}

fn unstyled_body(text: &str) -> StyledBody {
    StyledBody {
        styled: StyledText::from_plain_text(text),
        plain: SharedString::from(text),
        has_links: false,
    }
}

fn remember(source: Source, built: Option<StyledBody>) {
    with_session(|session| {
        let memo = &mut session.bodies.0;
        if memo.len() >= MAX_MEMO_ENTRIES {
            memo.clear();
        }
        memo.insert(source, built);
    });
}

fn build(html: &str) -> Option<StyledBody> {
    let mut writer = Writer::default();
    let document = Html::parse(html);
    for node in document.children() {
        writer.node(&node, 0);
    }
    writer.finish_line();

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
        }),
        Err(e) => {
            tracing::debug!("a formatted message did not render, showing it plain: {e}");
            (!plain.is_empty()).then(|| unstyled_body(plain))
        }
    }
}

fn build_plain(text: &str) -> Option<StyledBody> {
    if text.len() > MAX_MARKDOWN_LEN {
        return None;
    }

    let mut writer = Writer::default();
    for (index, line) in text.lines().enumerate() {
        if index > 0 {
            writer.markdown.push('\n');
        }
        writer.push_linkified(line);
    }
    if !writer.has_links || writer.exceeded_limits() {
        return None;
    }

    match StyledText::from_markdown(&writer.markdown) {
        Ok(styled) => Some(StyledBody {
            styled,
            plain: SharedString::from(text),
            has_links: true,
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

struct OpenStyle {
    style: InlineStyle,
    written_on_this_line: bool,
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct Writer {
    markdown: String,
    plain: String,
    has_links: bool,
    overflowed: bool,
    nodes: usize,
    line_has_content: bool,
    link_open: bool,
    list_depth: usize,
    open_styles: Vec<OpenStyle>,
}

impl Writer {
    fn exceeded_limits(&mut self) -> bool {
        self.overflowed |= self.markdown.len() > MAX_MARKDOWN_LEN;
        self.overflowed
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
            "img" => self.text(&image_text(node)),
            "b" | "strong" => self.styled(InlineStyle::Strong, node, depth),
            "i" | "em" => self.styled(InlineStyle::Italic, node, depth),
            "del" | "s" | "strike" => self.styled(InlineStyle::Struck, node, depth),
            "u" | "ins" => self.styled(InlineStyle::Underlined, node, depth),
            "span" | "font" => self.coloured(node, depth),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.block(node, depth, |w, n, d| w.styled(InlineStyle::Strong, n, d));
            }
            "code" => self.code_span(collapse_whitespace(&node_text(node)).trim()),
            "pre" => self.preformatted(node),
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

    fn block(&mut self, node: &NodeRef, depth: usize, body: impl Fn(&mut Self, &NodeRef, usize)) {
        self.finish_line();
        body(self, node, depth);
        self.finish_line();
    }

    fn styled(&mut self, style: InlineStyle, node: &NodeRef, depth: usize) {
        if self.open_styles.iter().any(|open| open.style == style) {
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
        self.text("> ");
        self.children(node, depth);
    }

    fn cell(&mut self, node: &NodeRef, depth: usize) {
        if self.line_has_content {
            self.markdown.push_str(CELL_GAP);
            self.plain.push_str(CELL_GAP);
        }
        self.children(node, depth);
    }

    fn code_span(&mut self, text: &str) {
        if text.is_empty() || self.exceeded_limits() {
            return;
        }
        self.write_pending_openers();
        let fence = "`".repeat(longest_backtick_run(text) + 1);
        let padding = if text.starts_with('`') || text.ends_with('`') {
            " "
        } else {
            ""
        };
        write!(self.markdown, "{fence}{padding}{text}{padding}{fence}").ok();
        self.plain.push_str(text);
        self.line_has_content = true;
    }

    fn preformatted(&mut self, node: &NodeRef) {
        self.finish_line();
        for line in node_text(node).lines() {
            self.code_span(line.replace('\t', TAB_AS_SPACES).trim_end());
            self.finish_line();
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
        self.write_pending_openers();

        self.link_open = true;
        let bracket_at = self.markdown.len();
        self.markdown.push('[');
        self.text(&plain_label);
        write!(self.markdown, "](<{href}>)").ok();
        self.link_open = false;

        if self.spans_one_line(bracket_at + 1) {
            self.has_links = true;
        } else {
            self.markdown.replace_range(bracket_at..=bracket_at, "");
        }
    }

    fn list(&mut self, node: &NodeRef, depth: usize) {
        if self.list_depth >= MAX_DEPTH {
            self.children(node, depth);
            return;
        }
        let ordered = has_name(node, "ol");

        self.finish_line();
        self.list_depth += 1;
        let mut number = 0;
        for child in node.children() {
            if !has_name(&child, "li") {
                continue;
            }
            number += 1;
            self.item(&child, depth + 1, ordered.then_some(number));
        }
        self.list_depth -= 1;
        self.finish_line();
    }

    fn item(&mut self, node: &NodeRef, depth: usize, number: Option<usize>) {
        self.finish_line();
        let indent = LIST_INDENT.repeat(self.list_depth.saturating_sub(1));
        let markdown_marker = number.map_or_else(|| "- ".to_owned(), |n| format!("{n}. "));
        let plain_marker = number.map_or_else(|| BULLET.to_owned(), |n| format!("{n}. "));

        self.markdown.push_str(&indent);
        self.markdown.push_str(&markdown_marker);
        self.plain.push_str(&indent);
        self.plain.push_str(&plain_marker);
        self.line_has_content = true;

        self.children(node, depth);
        self.finish_line();
    }

    fn text(&mut self, raw: &str) {
        if self.exceeded_limits() {
            return;
        }
        let collapsed = collapse_whitespace(raw);
        let text = if self.line_has_content {
            collapsed.as_str()
        } else {
            collapsed.trim_start()
        };
        if text.is_empty() {
            return;
        }

        self.write_pending_openers();
        if self.link_open {
            self.push_escaped(text);
        } else {
            self.push_linkified(text);
        }
        self.plain.push_str(text);
        self.line_has_content = true;
    }

    fn push_linkified(&mut self, text: &str) {
        let mut written = 0;
        for (span, destination) in autolink::find(text) {
            let (Some(before), Some(label)) =
                (text.get(written..span.start), text.get(span.clone()))
            else {
                continue;
            };
            self.push_escaped(before);
            self.markdown.push('[');
            self.push_escaped(label);
            write!(self.markdown, "](<{destination}>)").ok();
            self.has_links = true;
            written = span.end;
        }
        self.push_escaped(text.get(written..).unwrap_or_default());
    }

    fn push_escaped(&mut self, text: &str) {
        for ch in text.chars() {
            if ch.is_ascii_punctuation() {
                self.markdown.push('\\');
            }
            self.markdown.push(ch);
        }
    }

    fn finish_line(&mut self) {
        if !self.line_has_content {
            return;
        }
        self.write_closers_before_line_end();
        self.markdown.push('\n');
        self.plain.push('\n');
        self.line_has_content = false;
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
