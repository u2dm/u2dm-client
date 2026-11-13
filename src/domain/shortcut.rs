use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutCommand {
    OpenSettings,
    FindRoom,
    NextRegion,
    PreviousRegion,
    PreviousRoom,
    NextRoom,
    PreviousUnreadRoom,
    NextUnreadRoom,
    ToggleMicrophone,
    ToggleCamera,
    Reply,
    Edit,
    FirstUnread,
}

impl ShortcutCommand {
    pub const ALL: [Self; 13] = [
        Self::OpenSettings,
        Self::FindRoom,
        Self::NextRegion,
        Self::PreviousRegion,
        Self::PreviousRoom,
        Self::NextRoom,
        Self::PreviousUnreadRoom,
        Self::NextUnreadRoom,
        Self::ToggleMicrophone,
        Self::ToggleCamera,
        Self::Reply,
        Self::Edit,
        Self::FirstUnread,
    ];

    const fn default_chord(self) -> KeyChord {
        match self {
            Self::OpenSettings => KeyChord::new(Modifiers::CONTROL, ChordKey::Character(',')),
            Self::FindRoom => KeyChord::new(Modifiers::CONTROL, ChordKey::Character('k')),
            Self::NextRegion => KeyChord::new(Modifiers::NONE, ChordKey::Named(NamedKey::F6)),
            Self::PreviousRegion => KeyChord::new(Modifiers::SHIFT, ChordKey::Named(NamedKey::F6)),
            Self::PreviousRoom => KeyChord::new(Modifiers::ALT, ChordKey::Named(NamedKey::Up)),
            Self::NextRoom => KeyChord::new(Modifiers::ALT, ChordKey::Named(NamedKey::Down)),
            Self::PreviousUnreadRoom => {
                KeyChord::new(Modifiers::ALT_SHIFT, ChordKey::Named(NamedKey::Up))
            }
            Self::NextUnreadRoom => {
                KeyChord::new(Modifiers::ALT_SHIFT, ChordKey::Named(NamedKey::Down))
            }
            Self::ToggleMicrophone => KeyChord::new(Modifiers::CONTROL, ChordKey::Character('d')),
            Self::ToggleCamera => KeyChord::new(Modifiers::CONTROL, ChordKey::Character('e')),
            Self::Reply => KeyChord::new(Modifiers::NONE, ChordKey::Character('r')),
            Self::Edit => KeyChord::new(Modifiers::NONE, ChordKey::Character('e')),
            Self::FirstUnread => KeyChord::new(Modifiers::SHIFT, ChordKey::Named(NamedKey::PageUp)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    F6,
    Up,
    Down,
    PageUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChordKey {
    Character(char),
    Named(NamedKey),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[allow(clippy::struct_excessive_bools)]
pub struct Modifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Modifiers {
    const NONE: Self = Self {
        control: false,
        alt: false,
        shift: false,
        meta: false,
    };
    const CONTROL: Self = Self {
        control: true,
        ..Self::NONE
    };
    const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };
    const SHIFT: Self = Self {
        shift: true,
        ..Self::NONE
    };
    const ALT_SHIFT: Self = Self {
        alt: true,
        shift: true,
        ..Self::NONE
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub key: ChordKey,
    pub modifiers: Modifiers,
}

impl KeyChord {
    const fn new(modifiers: Modifiers, key: ChordKey) -> Self {
        Self { key, modifiers }
    }

    pub fn ignores_shift(self) -> bool {
        matches!(self.key, ChordKey::Character(c) if !c.is_alphabetic())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutTable {
    chords: HashMap<ShortcutCommand, KeyChord>,
}

impl Default for ShortcutTable {
    fn default() -> Self {
        Self {
            chords: ShortcutCommand::ALL
                .into_iter()
                .map(|command| (command, command.default_chord()))
                .collect(),
        }
    }
}

impl ShortcutTable {
    pub fn chord(&self, command: ShortcutCommand) -> Option<KeyChord> {
        self.chords.get(&command).copied()
    }
}
