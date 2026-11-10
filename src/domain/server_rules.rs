#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoomDefault {
    #[default]
    AllMessages,
    MentionsAndKeywords,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomClass {
    Direct,
    Group,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerRules {
    pub account_notifications: Option<bool>,
    pub direct: RoomDefault,
    pub group: RoomDefault,
    pub keywords: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyword(String);

impl Keyword {
    pub fn parse(text: &str) -> Option<Self> {
        let keyword = text.trim();
        let refused =
            keyword.is_empty() || keyword.starts_with('.') || keyword.contains(['/', '\\']);
        (!refused).then(|| Self(keyword.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
