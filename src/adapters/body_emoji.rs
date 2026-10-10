use std::collections::BTreeSet;

use ruma::MxcUri;
use ruma::html::{Html, NodeRef};

const MAX_DEPTH: usize = 16;
const IMAGE_TAG: &[u8] = b"<img";
const REPLY_FALLBACK: &str = "mx-reply";
const EMOTICON_ATTRIBUTE: &str = "data-mx-emoticon";

#[derive(Clone, Copy)]
enum Wanted {
    AnyImage,
    Emoticon,
}

impl Wanted {
    fn admits(self, image: &NodeRef) -> bool {
        match self {
            Self::AnyImage => true,
            Self::Emoticon => is_emoticon(image),
        }
    }
}

pub fn source(node: &NodeRef) -> Option<String> {
    let element = node.as_element()?;
    if element.name.local.as_ref() != "img" {
        return None;
    }
    let attrs = element.attrs.borrow();
    let src = attrs
        .iter()
        .find(|attr| attr.name.local.as_ref() == "src")?;
    let mxc: &str = &src.value;
    <&MxcUri>::from(mxc).is_valid().then(|| mxc.to_owned())
}

pub fn image_sources(html: &str) -> BTreeSet<String> {
    sources(html, Wanted::AnyImage)
}

pub fn emoticon_sources(html: &str) -> BTreeSet<String> {
    sources(html, Wanted::Emoticon)
}

fn sources(html: &str, wanted: Wanted) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    if !mentions_an_image(html) {
        return found;
    }
    for node in Html::parse(html).children() {
        collect(&node, 0, wanted, &mut found);
    }
    found
}

fn mentions_an_image(html: &str) -> bool {
    html.as_bytes()
        .windows(IMAGE_TAG.len())
        .any(|window| window.eq_ignore_ascii_case(IMAGE_TAG))
}

fn collect(node: &NodeRef, depth: usize, wanted: Wanted, found: &mut BTreeSet<String>) {
    if depth > MAX_DEPTH || is_reply_fallback(node) {
        return;
    }
    if let Some(mxc) = source(node) {
        if wanted.admits(node) {
            found.insert(mxc);
        }
        return;
    }
    for child in node.children() {
        collect(&child, depth + 1, wanted, found);
    }
}

fn is_emoticon(image: &NodeRef) -> bool {
    image.as_element().is_some_and(|element| {
        element
            .attrs
            .borrow()
            .iter()
            .any(|attr| attr.name.local.as_ref() == EMOTICON_ATTRIBUTE)
    })
}

fn is_reply_fallback(node: &NodeRef) -> bool {
    node.as_element()
        .is_some_and(|element| element.name.local.as_ref() == REPLY_FALLBACK)
}
