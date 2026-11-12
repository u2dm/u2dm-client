use slint::SharedString;
use slint::platform::Key;
use slint::winit_030::winit::event::KeyEvent;
use slint::winit_030::winit::keyboard::{Key as WinitKey, NamedKey};

pub fn slint_text(event: &KeyEvent) -> Option<SharedString> {
    match &event.logical_key {
        WinitKey::Named(named) => named_key(*named).map(SharedString::from),
        WinitKey::Character(text) => Some(SharedString::from(text.as_str())),
        _ => None,
    }
}

fn named_key(named: NamedKey) -> Option<Key> {
    Some(match named {
        NamedKey::Backspace => Key::Backspace,
        NamedKey::Tab => Key::Tab,
        NamedKey::Enter => Key::Return,
        NamedKey::Escape => Key::Escape,
        NamedKey::Delete => Key::Delete,
        NamedKey::Space => Key::Space,
        NamedKey::ArrowUp => Key::UpArrow,
        NamedKey::ArrowDown => Key::DownArrow,
        NamedKey::ArrowLeft => Key::LeftArrow,
        NamedKey::ArrowRight => Key::RightArrow,
        NamedKey::Home => Key::Home,
        NamedKey::End => Key::End,
        NamedKey::PageUp => Key::PageUp,
        NamedKey::PageDown => Key::PageDown,
        NamedKey::Insert => Key::Insert,
        NamedKey::ContextMenu => Key::Menu,
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
        _ => return None,
    })
}
