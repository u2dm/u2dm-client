const TOKEN_DELIMITER: char = ':';

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emote {
    pub token: String,
    pub mxc: String,
    pub title: String,
    pub alt: Option<String>,
    pub height: Option<String>,
    pub width: Option<String>,
}

impl Emote {
    pub fn token_for_title(title: &str) -> String {
        if title.len() > TOKEN_DELIMITER.len_utf8()
            && title.starts_with(TOKEN_DELIMITER)
            && title.ends_with(TOKEN_DELIMITER)
        {
            title.to_owned()
        } else {
            format!("{TOKEN_DELIMITER}{title}{TOKEN_DELIMITER}")
        }
    }

    pub fn body_token(&self) -> String {
        Self::token_for_title(&self.title)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposerText {
    pub text: String,
    pub emotes: Vec<Emote>,
}
