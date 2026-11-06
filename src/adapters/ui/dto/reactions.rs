use slint::{Image, SharedString};

use super::message::EmojiArt;
use crate::adapters::ui::decode::{
    AvatarSlot, Decoded, MediaSlot, TimelineItemKey, load_avatar_async, peek_thumbnail,
    record_custom_emoji_need,
};
use crate::adapters::ui::present::{
    avatar_color_index, name_initials, reaction_key_label, reactor_labels,
};
use crate::domain::message::{CustomEmoji, Reaction, ReactionSend, Reactor};
use crate::ports::media::MediaCache;
use crate::util::saturating_i32;

pub const REACTION_CHIP_CAP: usize = 6;

#[derive(Clone)]
pub struct ReactorAvatarDto {
    pub user_id: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

#[derive(Clone)]
pub struct ReactionDto {
    pub key: SharedString,
    pub label: SharedString,
    pub art: EmojiArt,
    pub image: Option<Image>,
    pub count: i32,
    pub mine: bool,
    pub send: ReactionSend,
    pub overflow: bool,
    pub reactors: SharedString,
    pub hidden_reactors: i32,
    pub avatars: Vec<ReactorAvatarDto>,
}

fn reactor_avatar_dto(
    item: &TimelineItemKey,
    reactor: &Reactor,
    media: &dyn MediaCache,
) -> ReactorAvatarDto {
    let slot = AvatarSlot::Reactor {
        item: item.clone(),
        user_id: reactor.user_id.clone(),
    };
    let path = reactor
        .avatar_url
        .as_deref()
        .and_then(|mxc| media.user_avatar_path(mxc));
    let avatar = load_avatar_async(path.as_deref(), slot);
    ReactorAvatarDto {
        user_id: SharedString::from(&reactor.user_id),
        initial: SharedString::from(name_initials(&reactor.name, &reactor.user_id)),
        color_index: avatar_color_index(&reactor.user_id),
        has_avatar: avatar.is_some(),
        avatar,
    }
}

fn reactor_avatar_dtos(
    item: &TimelineItemKey,
    reaction: &Reaction,
    media: &dyn MediaCache,
) -> Vec<ReactorAvatarDto> {
    if !reaction.shows_reactors() {
        return Vec::new();
    }
    reaction
        .senders
        .iter()
        .map(|reactor| reactor_avatar_dto(item, reactor, media))
        .collect()
}

fn downloaded_emoji_art(
    item: &TimelineItemKey,
    mxc: &str,
    media: &dyn MediaCache,
) -> (EmojiArt, Option<Image>) {
    let Some(path) = media.custom_emoji_path(mxc) else {
        return (EmojiArt::Pending, None);
    };
    record_custom_emoji_need(item, mxc, &path);
    match peek_thumbnail(&path, &MediaSlot::CustomEmoji(mxc.to_owned())) {
        Decoded::Ready(image) => (EmojiArt::Ready, Some(image)),
        Decoded::Failed(_) => (EmojiArt::Failed, None),
        Decoded::Pending => (EmojiArt::Pending, None),
    }
}

pub(super) fn emoji_art(
    item: &TimelineItemKey,
    mxc: &str,
    state: CustomEmoji,
    media: &dyn MediaCache,
) -> (EmojiArt, Option<Image>) {
    match state {
        CustomEmoji::Downloading => (EmojiArt::Pending, None),
        CustomEmoji::Unavailable => (EmojiArt::Failed, None),
        CustomEmoji::Downloaded => downloaded_emoji_art(item, mxc, media),
    }
}

fn reaction_art(
    item: &TimelineItemKey,
    reaction: &Reaction,
    media: &dyn MediaCache,
) -> (EmojiArt, Option<Image>) {
    match reaction.image {
        None => (EmojiArt::Text, None),
        Some(state) => emoji_art(item, &reaction.key, state, media),
    }
}

fn reaction_dto(
    item: &TimelineItemKey,
    reaction: &Reaction,
    media: &dyn MediaCache,
) -> ReactionDto {
    let (reactors, hidden) = reactor_labels(reaction);
    let (art, image) = reaction_art(item, reaction, media);
    let label = match art {
        EmojiArt::Text => SharedString::from(reaction_key_label(&reaction.key)),
        EmojiArt::Pending | EmojiArt::Ready | EmojiArt::Failed => SharedString::new(),
    };
    ReactionDto {
        key: SharedString::from(&reaction.key),
        label,
        art,
        image,
        count: saturating_i32(reaction.count()),
        mine: reaction.mine,
        send: reaction.send,
        overflow: false,
        reactors: SharedString::from(reactors),
        hidden_reactors: saturating_i32(hidden),
        avatars: reactor_avatar_dtos(item, reaction, media),
    }
}

fn overflow_dto(hidden: usize) -> ReactionDto {
    ReactionDto {
        key: SharedString::new(),
        label: SharedString::new(),
        art: EmojiArt::Text,
        image: None,
        count: saturating_i32(hidden),
        mine: false,
        send: ReactionSend::default(),
        overflow: true,
        reactors: SharedString::new(),
        hidden_reactors: 0,
        avatars: Vec::new(),
    }
}

pub(super) fn reaction_dtos(
    item: &TimelineItemKey,
    reactions: &[Reaction],
    media: &dyn MediaCache,
) -> (Vec<ReactionDto>, Vec<ReactionDto>) {
    let all: Vec<ReactionDto> = reactions
        .iter()
        .map(|reaction| reaction_dto(item, reaction, media))
        .collect();
    if all.len() <= REACTION_CHIP_CAP {
        return (all, Vec::new());
    }
    let mut chips: Vec<ReactionDto> = all
        .get(..REACTION_CHIP_CAP)
        .map(<[ReactionDto]>::to_vec)
        .unwrap_or_default();
    chips.push(overflow_dto(all.len() - REACTION_CHIP_CAP));
    (chips, all)
}
