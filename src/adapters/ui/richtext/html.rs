use ruma::html::NodeRef;

use super::writer::FIRST_LIST_NUMBER;
use super::{MAX_DEPTH, SPOILER_ATTRIBUTE};

pub(super) fn collapse_whitespace(text: &str) -> String {
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

pub(super) fn longest_backtick_run(text: &str) -> usize {
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

pub(super) fn is_hex_colour(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 3 | 4 | 6 | 8) && digits.chars().all(|ch| ch.is_ascii_hexdigit())
}

pub(super) fn list_start(node: &NodeRef) -> usize {
    attribute(node, "start")
        .and_then(|start| start.trim().parse().ok())
        .unwrap_or(FIRST_LIST_NUMBER)
}

pub(super) fn has_name(node: &NodeRef, name: &str) -> bool {
    node.as_element()
        .is_some_and(|element| element.name.local.as_ref() == name)
}

pub(super) fn is_spoiler(node: &NodeRef) -> bool {
    has_name(node, "span") && has_attribute(node, SPOILER_ATTRIBUTE)
}

pub(super) fn first_spoiler(node: &NodeRef, depth: usize) -> Option<NodeRef> {
    if depth > MAX_DEPTH {
        return None;
    }
    node.children().find_map(|child| {
        if is_spoiler(&child) {
            Some(child)
        } else {
            first_spoiler(&child, depth + 1)
        }
    })
}

pub(super) fn spoiler_reason(spoiler: &NodeRef) -> Option<String> {
    attribute(spoiler, SPOILER_ATTRIBUTE)
        .map(|reason| collapse_whitespace(&reason).trim().to_owned())
        .filter(|reason| !reason.is_empty())
}

pub(super) fn node_text(node: &NodeRef) -> String {
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

pub(super) fn image_text(node: &NodeRef) -> String {
    attribute(node, "alt")
        .or_else(|| attribute(node, "title"))
        .unwrap_or_default()
}

fn has_attribute(node: &NodeRef, name: &str) -> bool {
    node.as_element().is_some_and(|element| {
        element
            .attrs
            .borrow()
            .iter()
            .any(|attr| attr.name.local.as_ref() == name)
    })
}

pub(super) fn attribute(node: &NodeRef, name: &str) -> Option<String> {
    let element = node.as_element()?;
    let attrs = element.attrs.borrow();
    attrs
        .iter()
        .find(|attr| attr.name.local.as_ref() == name)
        .map(|attr| attr.value.to_string())
        .filter(|value| !value.is_empty())
}
