use std::collections::{BTreeMap, BTreeSet};

use slint::{Image, SharedString, StyledText};

use super::message::{EmojiArt, REVEALABLE, SpoilerState};
use super::reactions::emoji_art;
use crate::adapters::ui::decode::TimelineItemKey;
use crate::adapters::ui::present::{message_body_text, message_sender_label, preview_subject};
use crate::adapters::ui::richtext::{
    self, Cover, InlineContent, InlineLine, InlinePiece, RoomWord, Spoilers,
};
use crate::adapters::ui::spoilers::{keep_covered_body, spoilers_on};
use crate::domain::message::{CustomEmoji, MessageBody, RichText, TimelineMessage};
use crate::ports::media::MediaCache;
use crate::util::saturating_i32;

#[derive(Clone)]
pub struct BodyPieceDto {
    pub styled: StyledText,
    pub key: SharedString,
    pub art: EmojiArt,
    pub image: Option<Image>,
    pub spaced: bool,
}

#[derive(Clone)]
pub struct BodyLineDto {
    pub pieces: Vec<BodyPieceDto>,
    pub words: StyledText,
    pub gaps: i32,
}

fn one_line(text: &str) -> SharedString {
    SharedString::from(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub fn rich_body(text: &RichText) -> richtext::StyledBody {
    match text.html() {
        Some(html) => richtext::styled_body(
            html,
            &text.plain,
            None,
            RoomWord::Text,
            Spoilers::Covered(Cover::Text),
        ),
        None => richtext::plain_body(&text.plain, None, RoomWord::Text),
    }
}

pub fn preview_line(text: &RichText) -> SharedString {
    match text.html() {
        Some(html) => one_line(
            &richtext::styled_body(html, &text.plain, None, RoomWord::Text, REVEALABLE).plain,
        ),
        None => one_line(&text.plain),
    }
}

pub fn message_body(message: &TimelineMessage) -> richtext::StyledBody {
    let actor = emote_actor(message);
    BodyText::of(message, actor.as_deref()).styled(REVEALABLE)
}

pub struct RowBody {
    pub rich: richtext::StyledBody,
    pub lines: Vec<BodyLineDto>,
    pub emote_only: bool,
    pub spoilers: SpoilerState,
}

struct BodyText<'a> {
    body: &'a MessageBody,
    actor: Option<&'a str>,
    room_word: RoomWord,
    emoji: &'a BTreeMap<String, CustomEmoji>,
}

fn emote_actor(message: &TimelineMessage) -> Option<String> {
    matches!(message.body, MessageBody::Emote(_)).then(|| message_sender_label(message))
}

impl<'a> BodyText<'a> {
    fn of(message: &'a TimelineMessage, actor: Option<&'a str>) -> Self {
        Self {
            body: &message.body,
            actor,
            room_word: RoomWord::when(message.mentions_room),
            emoji: &message.body_emoji,
        }
    }

    fn styled(&self, spoilers: Spoilers) -> richtext::StyledBody {
        let plain = message_body_text(self.body);
        match self.body.html() {
            Some(html) => richtext::styled_body(html, plain, self.actor, self.room_word, spoilers),
            None => richtext::plain_body(plain, self.actor, self.room_word),
        }
    }

    fn row_body(
        &self,
        item: &TimelineItemKey,
        spoilers: Spoilers,
        media: &dyn MediaCache,
    ) -> RowBody {
        let (lines, emote_only) = self.inline_lines(item, spoilers, media);
        let rich = self.styled(spoilers);
        RowBody {
            spoilers: spoiler_state(rich.has_spoilers, spoilers),
            rich,
            lines,
            emote_only,
        }
    }

    fn inline_lines(
        &self,
        item: &TimelineItemKey,
        spoilers: Spoilers,
        media: &dyn MediaCache,
    ) -> (Vec<BodyLineDto>, bool) {
        let drawn: BTreeSet<String> = self
            .emoji
            .iter()
            .filter(|(_, state)| **state != CustomEmoji::Unavailable)
            .map(|(mxc, _)| mxc.clone())
            .collect();
        if drawn.is_empty() {
            return (Vec::new(), false);
        }
        let Some(inline) = self.body.html().and_then(|html| {
            richtext::inline_body(html, self.actor, self.room_word, spoilers, drawn)
        }) else {
            return (Vec::new(), false);
        };
        let lines = inline
            .lines
            .iter()
            .map(|line| body_line_dto(item, line, self.emoji, media))
            .collect();
        (lines, inline.emote_only)
    }
}

#[derive(Clone)]
pub struct CoveredBody {
    body: MessageBody,
    actor: Option<String>,
    room_word: RoomWord,
    emoji: BTreeMap<String, CustomEmoji>,
}

impl CoveredBody {
    fn of(message: &TimelineMessage, actor: Option<String>) -> Self {
        Self {
            body: message.body.clone(),
            actor,
            room_word: RoomWord::when(message.mentions_room),
            emoji: message.body_emoji.clone(),
        }
    }

    fn text(&self) -> BodyText<'_> {
        BodyText {
            body: &self.body,
            actor: self.actor.as_deref(),
            room_word: self.room_word,
            emoji: &self.emoji,
        }
    }

    pub fn revealed(&self, item: &TimelineItemKey, media: &dyn MediaCache) -> RowBody {
        self.text().row_body(item, Spoilers::Shown, media)
    }
}

pub(super) fn row_body_and_preview(
    item: &TimelineItemKey,
    message: &TimelineMessage,
    media: &dyn MediaCache,
) -> (RowBody, SharedString) {
    let actor = emote_actor(message);
    let text = BodyText::of(message, actor.as_deref());
    let spoilers = spoilers_on(&message.unique_id);
    let body = text.row_body(item, spoilers, media);
    if body.rich.has_spoilers {
        keep_covered_body(&message.unique_id, CoveredBody::of(message, actor.clone()));
    }
    let preview = match (preview_subject(&message.body), spoilers) {
        (Some(subject), _) => SharedString::from(subject),
        (None, Spoilers::Shown) => text.styled(REVEALABLE).plain,
        (None, Spoilers::Covered(_)) => body.rich.plain.clone(),
    };
    (body, one_line(&preview))
}

fn spoiler_state(has_spoilers: bool, spoilers: Spoilers) -> SpoilerState {
    match (has_spoilers, spoilers) {
        (false, _) => SpoilerState::None,
        (true, Spoilers::Covered(_)) => SpoilerState::Covered,
        (true, Spoilers::Shown) => SpoilerState::Shown,
    }
}

fn body_line_dto(
    item: &TimelineItemKey,
    line: &InlineLine,
    body_emoji: &BTreeMap<String, CustomEmoji>,
    media: &dyn MediaCache,
) -> BodyLineDto {
    BodyLineDto {
        pieces: line
            .pieces
            .iter()
            .map(|piece| body_piece_dto(item, piece, body_emoji, media))
            .collect(),
        words: line.words.clone(),
        gaps: saturating_i32(line.gaps),
    }
}

fn body_piece_dto(
    item: &TimelineItemKey,
    piece: &InlinePiece,
    body_emoji: &BTreeMap<String, CustomEmoji>,
    media: &dyn MediaCache,
) -> BodyPieceDto {
    let (styled, key, (art, image)) = match &piece.content {
        InlineContent::Text(styled) => {
            (styled.clone(), SharedString::new(), (EmojiArt::Text, None))
        }
        InlineContent::Emoji(mxc) => {
            let state = body_emoji
                .get(mxc)
                .copied()
                .unwrap_or(CustomEmoji::Unavailable);
            (
                StyledText::default(),
                SharedString::from(mxc),
                emoji_art(item, mxc, state, media),
            )
        }
    };
    BodyPieceDto {
        styled,
        key,
        art,
        image,
        spaced: piece.spaced,
    }
}
