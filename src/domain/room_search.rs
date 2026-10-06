use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleQuery(Vec<String>);

impl TitleQuery {
    pub fn new(raw: &str) -> Self {
        Self(raw.split_whitespace().map(folded).collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn matches(&self, title: &str) -> bool {
        if self.is_empty() {
            return true;
        }
        let title = folded(title);
        self.0.iter().all(|term| title.contains(term.as_str()))
    }
}

fn folded(text: &str) -> String {
    text.to_lowercase()
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect()
}
