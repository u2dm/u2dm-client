use std::cell::RefCell;

use slint::SharedString;
use slint::platform::Key;

use crate::domain::shortcut::{
    Binding, ChordKey, KeyChord, Modifiers, NamedKey, ShortcutChange, ShortcutCommand,
    ShortcutRefusal, ShortcutTable,
};

thread_local! {
    static IN_USE: RefCell<ShortcutTable> = RefCell::new(ShortcutTable::default());
}

pub(super) struct ShownBinding {
    pub(super) keys: KeyChord,
    pub(super) on: bool,
    pub(super) custom: bool,
    pub(super) standard: KeyChord,
}

pub(super) fn shown_binding(table: &ShortcutTable, command: ShortcutCommand) -> ShownBinding {
    let binding = table.binding(command);
    ShownBinding {
        keys: binding.chord(),
        on: matches!(binding, Binding::On(_)),
        custom: table.is_custom(command),
        standard: command.default_chord(),
    }
}

pub(super) fn use_shortcuts(table: &ShortcutTable) {
    IN_USE.with_borrow_mut(|in_use| in_use.clone_from(table));
}

pub(super) fn checked(change: ShortcutChange) -> Result<(), ShortcutRefusal> {
    IN_USE.with_borrow(|in_use| in_use.check(change))
}

pub(super) fn binding_in_use(command: ShortcutCommand) -> Binding {
    IN_USE.with_borrow(|in_use| in_use.binding(command))
}

pub(super) fn pressed_chord(text: &str, held: Modifiers) -> Option<KeyChord> {
    let key = NamedKey::ALL
        .into_iter()
        .find(|named| slint_text(*named) == text)
        .map_or_else(
            || ChordKey::typed(text),
            |named| Some(ChordKey::Named(named)),
        )?;
    Some(KeyChord::pressed(held, key))
}

pub(super) fn key_text(chord: KeyChord) -> SharedString {
    match chord.key {
        ChordKey::Character(character) => character.to_lowercase().collect::<String>().into(),
        ChordKey::Named(named) => slint_text(named),
    }
}

fn slint_text(key: NamedKey) -> SharedString {
    SharedString::from(slint_key(key))
}

const fn slint_key(key: NamedKey) -> Key {
    match key {
        NamedKey::F1 => Key::F1,
        NamedKey::F2 => Key::F2,
        NamedKey::F3 => Key::F3,
        NamedKey::F4 => Key::F4,
        NamedKey::F5 => Key::F5,
        NamedKey::F6 => Key::F6,
        NamedKey::F7 => Key::F7,
        NamedKey::F8 => Key::F8,
        NamedKey::F9 => Key::F9,
        NamedKey::F10 => Key::F10,
        NamedKey::F11 => Key::F11,
        NamedKey::F12 => Key::F12,
        NamedKey::Up => Key::UpArrow,
        NamedKey::Down => Key::DownArrow,
        NamedKey::Left => Key::LeftArrow,
        NamedKey::Right => Key::RightArrow,
        NamedKey::Home => Key::Home,
        NamedKey::End => Key::End,
        NamedKey::PageUp => Key::PageUp,
        NamedKey::PageDown => Key::PageDown,
        NamedKey::Insert => Key::Insert,
        NamedKey::Delete => Key::Delete,
        NamedKey::Backspace => Key::Backspace,
        NamedKey::Enter => Key::Return,
        NamedKey::Escape => Key::Escape,
        NamedKey::Tab => Key::Tab,
        NamedKey::Space => Key::Space,
        NamedKey::Menu => Key::Menu,
    }
}
