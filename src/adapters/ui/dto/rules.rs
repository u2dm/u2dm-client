use slint::SharedString;

use crate::commands::view::KeywordRow;

pub struct KeywordDto {
    pub keyword: SharedString,
    pub busy: bool,
}

pub fn keyword_to_dto(row: &KeywordRow) -> KeywordDto {
    KeywordDto {
        keyword: SharedString::from(&row.keyword),
        busy: row.busy,
    }
}
