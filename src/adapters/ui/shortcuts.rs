use slint::SharedString;
use slint::platform::Key;

use crate::domain::shortcut::{ChordKey, KeyChord, Modifiers, NamedKey};

pub(super) fn key_text(chord: Option<KeyChord>) -> SharedString {
    match chord.map(|chord| chord.key) {
        Some(ChordKey::Character(character)) => character.to_lowercase().collect::<String>().into(),
        Some(ChordKey::Named(named)) => SharedString::from(slint_key(named)),
        None => SharedString::new(),
    }
}

pub(super) fn held_with(chord: Option<KeyChord>) -> Modifiers {
    chord.map(|chord| chord.modifiers).unwrap_or_default()
}

pub(super) fn ignores_shift(chord: Option<KeyChord>) -> bool {
    chord.is_some_and(KeyChord::ignores_shift)
}

fn slint_key(key: NamedKey) -> Key {
    match key {
        NamedKey::F6 => Key::F6,
        NamedKey::Up => Key::UpArrow,
        NamedKey::Down => Key::DownArrow,
        NamedKey::PageUp => Key::PageUp,
    }
}
