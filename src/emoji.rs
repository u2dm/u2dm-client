use std::sync::OnceLock;

use emojis::{Group, SkinTone};

pub struct EmojiEntry {
    pub base: String,
    pub tones: Vec<String>,
    pub name: String,
    keywords: String,
}

impl Clone for EmojiEntry {
    fn clone(&self) -> Self {
        Self {
            base: self.base.clone(),
            tones: self.tones.clone(),
            name: self.name.clone(),
            keywords: self.keywords.clone(),
        }
    }
}

const GROUP_ORDER: [Group; 9] = [
    Group::SmileysAndEmotion,
    Group::PeopleAndBody,
    Group::AnimalsAndNature,
    Group::FoodAndDrink,
    Group::TravelAndPlaces,
    Group::Activities,
    Group::Objects,
    Group::Symbols,
    Group::Flags,
];

const SINGLE_TONES: [SkinTone; 6] = [
    SkinTone::Default,
    SkinTone::Light,
    SkinTone::MediumLight,
    SkinTone::Medium,
    SkinTone::MediumDark,
    SkinTone::Dark,
];

const MAX_SEARCH_RESULTS: usize = 90;

static GROUPS: OnceLock<Vec<Vec<EmojiEntry>>> = OnceLock::new();

fn tone_variants(emoji: &emojis::Emoji) -> Vec<String> {
    if emoji.skin_tone() != Some(SkinTone::Default) {
        return Vec::new();
    }
    let variants: Vec<String> = SINGLE_TONES
        .iter()
        .filter_map(|tone| emoji.with_skin_tone(*tone))
        .map(|e| e.as_str().to_string())
        .collect();
    if variants.len() == SINGLE_TONES.len() {
        variants
    } else {
        Vec::new()
    }
}

fn to_entry(emoji: &emojis::Emoji) -> EmojiEntry {
    let mut keywords = emoji.name().to_lowercase();
    for shortcode in emoji.shortcodes() {
        keywords.push(' ');
        keywords.push_str(shortcode);
    }
    EmojiEntry {
        base: emoji.as_str().to_string(),
        tones: tone_variants(emoji),
        name: emoji.name().to_string(),
        keywords,
    }
}

fn build() -> Vec<Vec<EmojiEntry>> {
    GROUP_ORDER
        .iter()
        .map(|group| group.emojis().map(to_entry).collect())
        .collect()
}

pub fn groups() -> &'static [Vec<EmojiEntry>] {
    GROUPS.get_or_init(build)
}

pub fn insert_at(text: &str, offset: i32, glyph: &str) -> (String, i32) {
    let mut byte_offset = usize::try_from(offset).unwrap_or(0).min(text.len());
    while byte_offset > 0 && !text.is_char_boundary(byte_offset) {
        byte_offset -= 1;
    }
    let before = text.get(..byte_offset).unwrap_or(text);
    let after = text.get(byte_offset..).unwrap_or("");
    let mut result = String::with_capacity(before.len() + glyph.len() + after.len());
    result.push_str(before);
    result.push_str(glyph);
    result.push_str(after);
    let caret = i32::try_from(before.len() + glyph.len()).unwrap_or(i32::MAX);
    (result, caret)
}

pub fn search(query: &str) -> Vec<EmojiEntry> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    groups()
        .iter()
        .flatten()
        .filter(|entry| entry.keywords.contains(&needle))
        .take(MAX_SEARCH_RESULTS)
        .cloned()
        .collect()
}
