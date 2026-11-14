use std::collections::HashMap;
use std::fmt;
use std::ops::RangeInclusive;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutCommand {
    OpenSettings,
    FindRoom,
    ShowShortcuts,
    NextRegion,
    PreviousRegion,
    PreviousRoom,
    NextRoom,
    PreviousUnreadRoom,
    NextUnreadRoom,
    MoveSpaceUp,
    MoveSpaceDown,
    ToggleMicrophone,
    ToggleCamera,
    Reply,
    Edit,
    FirstUnread,
}

impl ShortcutCommand {
    pub const ALL: [Self; 16] = [
        Self::OpenSettings,
        Self::FindRoom,
        Self::ShowShortcuts,
        Self::NextRegion,
        Self::PreviousRegion,
        Self::PreviousRoom,
        Self::NextRoom,
        Self::PreviousUnreadRoom,
        Self::NextUnreadRoom,
        Self::MoveSpaceUp,
        Self::MoveSpaceDown,
        Self::ToggleMicrophone,
        Self::ToggleCamera,
        Self::Reply,
        Self::Edit,
        Self::FirstUnread,
    ];

    pub const fn default_chord(self) -> KeyChord {
        match self {
            Self::OpenSettings => KeyChord::of(Modifiers::CONTROL, ChordKey::Character(',')),
            Self::FindRoom => KeyChord::of(Modifiers::CONTROL, ChordKey::Character('k')),
            Self::ShowShortcuts => KeyChord::of(Modifiers::CONTROL, ChordKey::Character('/')),
            Self::NextRegion => KeyChord::of(Modifiers::NONE, ChordKey::Named(NamedKey::F6)),
            Self::PreviousRegion => KeyChord::of(Modifiers::SHIFT, ChordKey::Named(NamedKey::F6)),
            Self::PreviousRoom => KeyChord::of(Modifiers::ALT, ChordKey::Named(NamedKey::Up)),
            Self::NextRoom => KeyChord::of(Modifiers::ALT, ChordKey::Named(NamedKey::Down)),
            Self::PreviousUnreadRoom => {
                KeyChord::of(Modifiers::ALT_SHIFT, ChordKey::Named(NamedKey::Up))
            }
            Self::NextUnreadRoom => {
                KeyChord::of(Modifiers::ALT_SHIFT, ChordKey::Named(NamedKey::Down))
            }
            Self::MoveSpaceUp => {
                KeyChord::of(Modifiers::CONTROL_SHIFT, ChordKey::Named(NamedKey::Up))
            }
            Self::MoveSpaceDown => {
                KeyChord::of(Modifiers::CONTROL_SHIFT, ChordKey::Named(NamedKey::Down))
            }
            Self::ToggleMicrophone => KeyChord::of(Modifiers::CONTROL, ChordKey::Character('d')),
            Self::ToggleCamera => KeyChord::of(Modifiers::CONTROL, ChordKey::Character('e')),
            Self::Reply => KeyChord::of(Modifiers::NONE, ChordKey::Character('r')),
            Self::Edit => KeyChord::of(Modifiers::NONE, ChordKey::Character('e')),
            Self::FirstUnread => KeyChord::of(Modifiers::SHIFT, ChordKey::Named(NamedKey::PageUp)),
        }
    }

    pub const fn scope(self) -> ShortcutScope {
        match self {
            Self::Reply | Self::Edit | Self::FirstUnread => ShortcutScope::Message,
            Self::MoveSpaceUp | Self::MoveSpaceDown => ShortcutScope::SpaceTile,
            Self::OpenSettings
            | Self::FindRoom
            | Self::ShowShortcuts
            | Self::NextRegion
            | Self::PreviousRegion
            | Self::PreviousRoom
            | Self::NextRoom
            | Self::PreviousUnreadRoom
            | Self::NextUnreadRoom
            | Self::ToggleMicrophone
            | Self::ToggleCamera => ShortcutScope::Screen,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutScope {
    Screen,
    Message,
    SpaceTile,
}

impl ShortcutScope {
    const fn overlaps(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Screen, _)
                | (_, Self::Screen)
                | (Self::Message, Self::Message)
                | (Self::SpaceTile, Self::SpaceTile)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Backspace,
    Enter,
    Escape,
    Tab,
    Space,
    Menu,
}

impl NamedKey {
    pub const ALL: [Self; 28] = [
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::F6,
        Self::F7,
        Self::F8,
        Self::F9,
        Self::F10,
        Self::F11,
        Self::F12,
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::Home,
        Self::End,
        Self::PageUp,
        Self::PageDown,
        Self::Insert,
        Self::Delete,
        Self::Backspace,
        Self::Enter,
        Self::Escape,
        Self::Tab,
        Self::Space,
        Self::Menu,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
            Self::Insert => "Insert",
            Self::Delete => "Delete",
            Self::Backspace => "Backspace",
            Self::Enter => "Enter",
            Self::Escape => "Escape",
            Self::Tab => "Tab",
            Self::Space => "Space",
            Self::Menu => "Menu",
        }
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|key| key.name().eq_ignore_ascii_case(name))
    }

    const fn is_function_key(self) -> bool {
        matches!(
            self,
            Self::F1
                | Self::F2
                | Self::F3
                | Self::F4
                | Self::F5
                | Self::F6
                | Self::F7
                | Self::F8
                | Self::F9
                | Self::F10
                | Self::F11
                | Self::F12
        )
    }

    const fn moves(self) -> bool {
        matches!(
            self,
            Self::Up
                | Self::Down
                | Self::Left
                | Self::Right
                | Self::Home
                | Self::End
                | Self::PageUp
                | Self::PageDown
        )
    }

    const fn always_kept(self) -> bool {
        matches!(
            self,
            Self::Insert
                | Self::Delete
                | Self::Backspace
                | Self::Enter
                | Self::Escape
                | Self::Tab
                | Self::Space
                | Self::Menu
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChordKey {
    Character(char),
    Named(NamedKey),
}

const PRIVATE_USE: RangeInclusive<char> = '\u{E000}'..='\u{F8FF}';

impl ChordKey {
    pub fn typed(text: &str) -> Option<Self> {
        let mut characters = text.chars();
        match (characters.next(), characters.next()) {
            (Some(character), None)
                if !character.is_control() && !PRIVATE_USE.contains(&character) =>
            {
                Some(Self::Character(lowercase(character)))
            }
            _ => None,
        }
    }

    fn named(name: &str) -> Option<Self> {
        NamedKey::named(name).map_or_else(|| Self::typed(name), |named| Some(Self::Named(named)))
    }

    const fn is_function_key(self) -> bool {
        matches!(self, Self::Named(key) if key.is_function_key())
    }
}

fn lowercase(character: char) -> char {
    let mut lower = character.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(single), None) => single,
        _ => character,
    }
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
    const CONTROL_SHIFT: Self = Self {
        control: true,
        shift: true,
        ..Self::NONE
    };

    const fn commands(self) -> bool {
        self.control || self.alt || self.meta
    }

    const fn edits_text(self) -> bool {
        self.control && !self.alt && !self.meta
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub key: ChordKey,
    pub modifiers: Modifiers,
}

const TEXT_EDITING_LETTERS: &str = "acvxyz";

impl KeyChord {
    const fn of(modifiers: Modifiers, key: ChordKey) -> Self {
        Self { key, modifiers }
    }

    pub fn pressed(modifiers: Modifiers, key: ChordKey) -> Self {
        let key = match key {
            ChordKey::Character(character) => ChordKey::Character(lowercase(character)),
            ChordKey::Named(_) => key,
        };
        let held = Self::of(modifiers, key);
        if held.ignores_shift() {
            Self::of(
                Modifiers {
                    shift: false,
                    ..modifiers
                },
                key,
            )
        } else {
            held
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let (held, key) = split_chord(text.trim());
        let mut modifiers = Modifiers::NONE;
        for name in held.split('+') {
            match name.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.control = true,
                "alt" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "super" | "meta" => modifiers.meta = true,
                "" => {}
                _ => return None,
            }
        }
        Some(Self::pressed(modifiers, ChordKey::named(key.trim())?))
    }

    pub fn ignores_shift(self) -> bool {
        matches!(self.key, ChordKey::Character(c) if !c.is_alphabetic())
    }

    fn refusal_in(self, scope: ShortcutScope) -> Option<ShortcutRefusal> {
        if self.is_kept(scope) {
            return Some(ShortcutRefusal::Reserved);
        }
        if scope != ShortcutScope::Message
            && !self.modifiers.commands()
            && !self.key.is_function_key()
        {
            return Some(ShortcutRefusal::NeedsModifier);
        }
        None
    }

    fn is_kept(self, scope: ShortcutScope) -> bool {
        let held = self.modifiers;
        match self.key {
            ChordKey::Named(key) if key.always_kept() => true,
            ChordKey::Named(NamedKey::F10) => held == Modifiers::SHIFT,
            ChordKey::Named(key) if key.moves() => {
                held == Modifiers::NONE
                    || (scope == ShortcutScope::Screen && !held.alt && !held.meta)
            }
            ChordKey::Named(_) => false,
            ChordKey::Character(character) => {
                held.edits_text() && TEXT_EDITING_LETTERS.contains(character)
            }
        }
    }
}

fn split_chord(text: &str) -> (&str, &str) {
    if text == "+" {
        return ("", text);
    }
    if let Some(held) = text.strip_suffix("++") {
        return (held, "+");
    }
    text.rsplit_once('+').unwrap_or(("", text))
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Modifiers {
            control,
            alt,
            shift,
            meta,
        } = self.modifiers;
        for (held, name) in [
            (meta, "Super"),
            (control, "Ctrl"),
            (alt, "Alt"),
            (shift, "Shift"),
        ] {
            if held {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            ChordKey::Named(key) => f.write_str(key.name()),
            ChordKey::Character(character) => write!(f, "{}", character.to_uppercase()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    On(KeyChord),
    Off(KeyChord),
}

impl Binding {
    pub const fn chord(self) -> KeyChord {
        match self {
            Self::On(chord) | Self::Off(chord) => chord,
        }
    }

    pub const fn active(self) -> Option<KeyChord> {
        match self {
            Self::On(chord) => Some(chord),
            Self::Off(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutChange {
    Bind(ShortcutCommand, KeyChord),
    TurnOff(ShortcutCommand),
    TurnOn(ShortcutCommand),
    Reset(ShortcutCommand),
    ResetAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutRefusal {
    Reserved,
    NeedsModifier,
    Taken(ShortcutCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutTable {
    bindings: HashMap<ShortcutCommand, Binding>,
}

impl Default for ShortcutTable {
    fn default() -> Self {
        Self {
            bindings: ShortcutCommand::ALL
                .into_iter()
                .map(|command| (command, Binding::On(command.default_chord())))
                .collect(),
        }
    }
}

impl ShortcutTable {
    pub fn from_bindings(read: &HashMap<ShortcutCommand, Binding>) -> Self {
        let mut table = Self {
            bindings: HashMap::new(),
        };
        for command in ShortcutCommand::ALL {
            let wanted = read
                .get(&command)
                .copied()
                .unwrap_or(Binding::On(command.default_chord()));
            let kept = table.fitting(command, wanted);
            table.bindings.insert(command, kept);
        }
        table
    }

    pub fn binding(&self, command: ShortcutCommand) -> Binding {
        self.bindings
            .get(&command)
            .copied()
            .unwrap_or(Binding::On(command.default_chord()))
    }

    pub fn chord(&self, command: ShortcutCommand) -> Option<KeyChord> {
        self.binding(command).active()
    }

    pub fn is_custom(&self, command: ShortcutCommand) -> bool {
        self.binding(command) != Binding::On(command.default_chord())
    }

    pub fn custom_count(&self) -> usize {
        ShortcutCommand::ALL
            .into_iter()
            .filter(|command| self.is_custom(*command))
            .count()
    }

    pub fn check(&self, change: ShortcutChange) -> Result<(), ShortcutRefusal> {
        match self.proposed(change) {
            Some((command, Binding::On(chord))) => self.admits(command, chord),
            Some((_, Binding::Off(_))) | None => Ok(()),
        }
    }

    pub fn apply(&mut self, change: ShortcutChange) -> Result<bool, ShortcutRefusal> {
        self.check(change)?;
        let Some((command, binding)) = self.proposed(change) else {
            let shipped = Self::default();
            let moved = *self != shipped;
            *self = shipped;
            return Ok(moved);
        };
        Ok(self.bindings.insert(command, binding) != Some(binding))
    }

    fn proposed(&self, change: ShortcutChange) -> Option<(ShortcutCommand, Binding)> {
        match change {
            ShortcutChange::Bind(command, chord) => Some((command, Binding::On(chord))),
            ShortcutChange::TurnOff(command) => {
                Some((command, Binding::Off(self.binding(command).chord())))
            }
            ShortcutChange::TurnOn(command) => {
                Some((command, Binding::On(self.binding(command).chord())))
            }
            ShortcutChange::Reset(command) => Some((command, Binding::On(command.default_chord()))),
            ShortcutChange::ResetAll => None,
        }
    }

    fn admits(&self, command: ShortcutCommand, chord: KeyChord) -> Result<(), ShortcutRefusal> {
        if let Some(refusal) = chord.refusal_in(command.scope()) {
            return Err(refusal);
        }
        self.owner_of(chord, command)
            .map_or(Ok(()), |owner| Err(ShortcutRefusal::Taken(owner)))
    }

    fn owner_of(&self, chord: KeyChord, asking: ShortcutCommand) -> Option<ShortcutCommand> {
        ShortcutCommand::ALL.into_iter().find(|command| {
            *command != asking
                && command.scope().overlaps(asking.scope())
                && self.bindings.get(command).and_then(|held| held.active()) == Some(chord)
        })
    }

    fn fitting(&self, command: ShortcutCommand, wanted: Binding) -> Binding {
        let shipped = command.default_chord();
        match wanted {
            Binding::On(chord) if self.admits(command, chord).is_ok() => wanted,
            Binding::On(_) if self.admits(command, shipped).is_ok() => Binding::On(shipped),
            Binding::Off(chord) if chord.refusal_in(command.scope()).is_none() => wanted,
            Binding::On(_) | Binding::Off(_) => Binding::Off(shipped),
        }
    }
}
