use std::collections::BTreeSet;

use ruma::MxcUri;
use ruma::html::{Attribute, Html, LocalName, Namespace, NodeRef, QualName, StrTendril};

use crate::domain::emote::Emote;

const MAX_DEPTH: usize = 16;
const IMAGE_TAG: &[u8] = b"<img";
const REPLY_FALLBACK: &str = "mx-reply";
pub const EMOTICON_ATTRIBUTE: &str = "data-mx-emoticon";
const TITLE_ATTRIBUTE: &str = "title";
const ALT_ATTRIBUTE: &str = "alt";
const HEIGHT_ATTRIBUTE: &str = "height";
const WIDTH_ATTRIBUTE: &str = "width";

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

pub fn emotes_in(html: &str) -> Vec<Emote> {
    let mut emotes: Vec<Emote> = Vec::new();
    if !mentions_an_image(html) {
        return emotes;
    }
    for node in images(&Html::parse(html)) {
        if let Some(emote) = emote_of(&node)
            && !emotes.iter().any(|known| known.token == emote.token)
        {
            emotes.push(emote);
        }
    }
    emotes
}

pub fn marked_as_emoticons(html: &str) -> String {
    let parsed = Html::parse(html);
    for node in images(&parsed) {
        if let Some(element) = node.as_element()
            && !is_emoticon(&node)
        {
            element.attrs.borrow_mut().insert(Attribute {
                name: QualName::new(
                    None,
                    Namespace::from(""),
                    LocalName::from(EMOTICON_ATTRIBUTE),
                ),
                value: StrTendril::new(),
            });
        }
    }
    parsed.to_string()
}

fn sources(html: &str, wanted: Wanted) -> BTreeSet<String> {
    if !mentions_an_image(html) {
        return BTreeSet::new();
    }
    images(&Html::parse(html))
        .iter()
        .filter(|image| wanted.admits(image))
        .filter_map(source)
        .collect()
}

fn images(html: &Html) -> Vec<NodeRef> {
    let mut found = Vec::new();
    for node in html.children() {
        collect(&node, 0, &mut found);
    }
    found
}

fn mentions_an_image(html: &str) -> bool {
    html.as_bytes()
        .windows(IMAGE_TAG.len())
        .any(|window| window.eq_ignore_ascii_case(IMAGE_TAG))
}

fn collect(node: &NodeRef, depth: usize, found: &mut Vec<NodeRef>) {
    if depth > MAX_DEPTH || is_reply_fallback(node) {
        return;
    }
    if source(node).is_some() {
        found.push(node.clone());
        return;
    }
    for child in node.children() {
        collect(&child, depth + 1, found);
    }
}

fn emote_of(node: &NodeRef) -> Option<Emote> {
    if !is_emoticon(node) {
        return None;
    }
    let mxc = source(node)?;
    let title = attribute(node, TITLE_ATTRIBUTE)?;
    Some(Emote {
        token: Emote::token_for_title(&title),
        mxc,
        title,
        alt: attribute(node, ALT_ATTRIBUTE),
        height: attribute(node, HEIGHT_ATTRIBUTE),
        width: attribute(node, WIDTH_ATTRIBUTE),
    })
}

fn attribute(node: &NodeRef, name: &str) -> Option<String> {
    let element = node.as_element()?;
    let attrs = element.attrs.borrow();
    attrs
        .iter()
        .find(|attr| attr.name.local.as_ref() == name)
        .map(|attr| attr.value.to_string())
}

pub fn is_emoticon(image: &NodeRef) -> bool {
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
