use slint::{Image, Model, ModelRc};

use super::UiBackend;
use super::avatars::indexed_timeline_row;
use crate::adapters::ui::decode::{
    AvatarSlot, DecodeFailure, request_pack_cell, rows_showing_custom_emoji,
};
use crate::adapters::ui::dto::{EmojiArt, MediaState, cell_pack};
use crate::adapters::ui::fields::{
    BodyLineFields, BodyPieceFields, MessageFields, PackCellFields, PackRowFields, PackTabFields,
    ReactionFields,
};
use crate::adapters::ui::reconcile::{
    forget_pack_avatar, pack_cell_row, pack_tab_row, shows_pack_avatar,
};
use crate::adapters::ui::rows::{patch_first, patch_rows_where};

pub(super) fn place_custom_emoji<B: UiBackend>(
    mxc: &str,
    art: Result<&Image, DecodeFailure>,
) -> Option<usize> {
    let rows = rows_showing_custom_emoji(mxc);
    B::with_timeline(|timeline| {
        let mut placed = None;
        for unique_id in &rows {
            let Some(row) = indexed_timeline_row::<B>(timeline, unique_id) else {
                continue;
            };
            let Some(entry) = timeline.row_data(row) else {
                continue;
            };
            let in_chips = patch_reaction_art::<B>(&entry.reactions(), mxc, art);
            let in_detail = patch_reaction_art::<B>(&entry.all_reactions(), mxc, art);
            let in_body = patch_body_emoji::<B>(&entry.body_lines(), mxc, art);
            if in_chips || in_detail || in_body {
                placed = Some(row);
            }
        }
        placed
    })
}

fn patch_body_emoji<B: UiBackend>(
    lines: &ModelRc<B::BodyLine>,
    mxc: &str,
    art: Result<&Image, DecodeFailure>,
) -> bool {
    let mut patched = false;
    for line in lines.iter() {
        patched |= patch_rows_where(
            &line.pieces(),
            |piece| piece.key() == mxc,
            |piece| match art {
                Ok(image) => {
                    piece.set_image(image.clone());
                    piece.set_art(EmojiArt::Ready);
                }
                Err(_) => piece.set_art(EmojiArt::Failed),
            },
        );
    }
    patched
}

fn patch_reaction_art<B: UiBackend>(
    reactions: &ModelRc<B::Reaction>,
    key: &str,
    art: Result<&Image, DecodeFailure>,
) -> bool {
    patch_first(
        reactions,
        |reaction| reaction.key() == key,
        |reaction| match art {
            Ok(image) => {
                reaction.set_image(image.clone());
                reaction.set_art(EmojiArt::Ready);
            }
            Err(_) => reaction.set_art(EmojiArt::Failed),
        },
    )
}

pub(super) fn place_pack_cell<B: UiBackend>(key: &str, art: Option<&Image>) -> Option<usize> {
    let row = pack_cell_row(key)?;
    let entry = B::models().pack_rows.row_data(row)?;
    patch_pack_cell::<B>(&entry, key, art).then_some(row)
}

fn patch_pack_cell<B: UiBackend>(row: &B::PackRow, key: &str, art: Option<&Image>) -> bool {
    patch_first(
        &row.cells(),
        |cell| cell.key() == key,
        |cell| match art {
            Some(art) => {
                cell.set_image(art.clone());
                cell.set_media_state(MediaState::Ready);
            }
            None => cell.set_media_state(MediaState::Failed),
        },
    )
}

fn adopt_pack_icon<B: UiBackend>(pack_id: &str, image: &Image) {
    let Some(row) = pack_tab_row(pack_id) else {
        return;
    };
    let packs = &B::models().pack_tabs;
    let Some(updated) = packs
        .row_data(row)
        .and_then(|tab| pack_with_icon::<B>(&tab, pack_id, image))
    else {
        return;
    };
    packs.remove(row);
    packs.insert(row, updated);
}

fn pack_with_icon<B: UiBackend>(
    pack: &B::PackTab,
    pack_id: &str,
    image: &Image,
) -> Option<B::PackTab> {
    if pack.has_icon() || pack.id() != pack_id {
        return None;
    }
    let mut updated = pack.clone();
    updated.set_icon(image.clone());
    updated.set_has_icon(true);
    Some(updated)
}

pub(in crate::adapters::ui) fn apply_pack_art<B: UiBackend>(key: &str, art: Option<&Image>) {
    if place_pack_cell::<B>(key, art).is_none() {
        tracing::debug!(key, "dropped a decoded image with no live sticker cell");
    }
    let pack = cell_pack(key);
    if let Some(image) = art
        && !shows_pack_avatar(pack)
    {
        adopt_pack_icon::<B>(pack, image);
    }
}

pub(in crate::adapters::ui) fn apply_pack_avatar<B: UiBackend>(pack_id: &str, art: Option<&Image>) {
    match art {
        Some(image) => adopt_pack_icon::<B>(pack_id, image),
        None => {
            if let Some(first_cell) = forget_pack_avatar(pack_id) {
                request_pack_cell(&first_cell);
            }
        }
    }
}

pub(super) fn apply_pack_avatars<B: UiBackend>(slots: &[AvatarSlot], art: Option<&Image>) {
    for slot in slots {
        if let AvatarSlot::Pack(pack_id) = slot {
            apply_pack_avatar::<B>(pack_id, art);
        }
    }
}
