const LEFT_TO_RIGHT_EMBEDDING: char = '\u{202a}';
const RIGHT_TO_LEFT_OVERRIDE: char = '\u{202e}';
const LEFT_TO_RIGHT_ISOLATE: char = '\u{2066}';
const FIRST_STRONG_ISOLATE: char = '\u{2068}';
const POP_DIRECTIONAL_ISOLATE: char = '\u{2069}';

fn steers_direction(c: char) -> bool {
    matches!(
        c,
        LEFT_TO_RIGHT_EMBEDDING..=RIGHT_TO_LEFT_OVERRIDE
            | LEFT_TO_RIGHT_ISOLATE..=POP_DIRECTIONAL_ISOLATE
    )
}

fn push_without_direction_controls(shown: &mut String, text: &str) {
    shown.extend(text.chars().filter(|c| !steers_direction(*c)));
}

fn push_isolated(shown: &mut String, text: &str) {
    shown.push(FIRST_STRONG_ISOLATE);
    push_without_direction_controls(shown, text);
    shown.push(POP_DIRECTIONAL_ISOLATE);
}

pub fn isolated(text: &str) -> String {
    let mut shown = String::with_capacity(text.len() + 2 * FIRST_STRONG_ISOLATE.len_utf8());
    push_isolated(&mut shown, text);
    shown
}

fn with_user_id(name: &str, user_id: &str) -> String {
    let mut shown = String::new();
    shown.push(FIRST_STRONG_ISOLATE);
    push_isolated(&mut shown, name);
    shown.push_str(" (");
    push_without_direction_controls(&mut shown, user_id);
    shown.push(')');
    shown.push(POP_DIRECTIONAL_ISOLATE);
    shown
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MemberName {
    #[default]
    Unset,
    Unique(String),
    Shared(String),
}

impl MemberName {
    pub fn new(name: Option<&str>, ambiguous: bool) -> Self {
        match name.filter(|name| !name.trim().is_empty()) {
            None => Self::Unset,
            Some(name) if ambiguous || name.chars().any(steers_direction) => {
                Self::Shared(name.to_owned())
            }
            Some(name) => Self::Unique(name.to_owned()),
        }
    }

    pub fn set(&self) -> Option<&str> {
        match self {
            Self::Unset => None,
            Self::Unique(name) | Self::Shared(name) => Some(name),
        }
    }

    pub fn shown(&self, user_id: &str) -> String {
        match self {
            Self::Unset => isolated(user_id),
            Self::Unique(name) => isolated(name),
            Self::Shared(name) => with_user_id(name, user_id),
        }
    }

    pub fn identified(&self, user_id: &str) -> String {
        match self.set() {
            None => isolated(user_id),
            Some(name) => with_user_id(name, user_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedUser {
    pub user_id: String,
    pub name: MemberName,
}

impl NamedUser {
    pub fn new(user_id: impl Into<String>, name: MemberName) -> Self {
        Self {
            user_id: user_id.into(),
            name,
        }
    }

    pub fn unnamed(user_id: impl Into<String>) -> Self {
        Self::new(user_id, MemberName::Unset)
    }

    pub fn shown(&self) -> String {
        self.name.shown(&self.user_id)
    }

    pub fn identified(&self) -> String {
        self.name.identified(&self.user_id)
    }
}
