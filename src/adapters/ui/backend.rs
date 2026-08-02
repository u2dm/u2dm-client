use std::collections::HashSet;
use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, VecModel};
use tokio::sync::OwnedSemaphorePermit;

use super::decode::{
    AvatarSlot, DecodeOutcome, advance_animations, set_animation_tick, set_avatar_ready,
    set_image_ready,
};
use super::dto::{CELL_KEY_SEPARATOR, StickerPackDto, StickerRowDto};
use super::props::{IntProp, StringProp, UiProps};
use super::reduce::dispatch_effect;
use super::rows::{locate_row, patch_first_row, patch_rows_by_id};
use crate::commands::effects::Effect;
use crate::domain::models::{EnrichmentDelta, Room, RoomId, Space, TimelineMessage};
use crate::ports::media::MediaCache;

pub trait UiBackend: Sized + 'static {
    type Window: ComponentHandle + UiProps + 'static;
    type Message: Clone + 'static;
    type Room: Clone + PartialEq + 'static;
    type Space: Clone + PartialEq + 'static;
    type StickerRow: Clone + 'static;
    type StickerPack: Clone + 'static;

    fn convert_message(message: &TimelineMessage, media: &dyn MediaCache) -> Self::Message;
    fn enrich_message(entry: &mut Self::Message, delta: &EnrichmentDelta, media: &dyn MediaCache);
    fn convert_room(room: &Room, media: &dyn MediaCache) -> Self::Room;
    fn convert_space(space: &Space, media: &dyn MediaCache) -> Self::Space;

    fn convert_sticker_row(row: &StickerRowDto) -> Self::StickerRow;
    fn convert_sticker_pack(pack: &StickerPackDto) -> Self::StickerPack;
    fn patch_sticker_cell(row: &Self::StickerRow, key: &str, art: Option<&Image>) -> bool;
    fn sticker_pack_with_icon(
        pack: &Self::StickerPack,
        pack_id: &str,
        image: &Image,
    ) -> Option<Self::StickerPack>;

    fn message_id(entry: &Self::Message) -> &str;
    fn message_event_id(entry: &Self::Message) -> &str;
    fn message_is_first_unread(entry: &Self::Message) -> bool;
    fn room_id(entry: &Self::Room) -> &str;
    fn space_id(entry: &Self::Space) -> &str;

    fn set_message_avatar(entry: &mut Self::Message, image: &Image);
    fn set_room_avatar(entry: &mut Self::Room, image: &Image);
    fn set_space_avatar(entry: &mut Self::Space, image: &Image);
    fn set_message_thumbnail(entry: &mut Self::Message, image: &Image);
    fn set_message_media_failed(entry: &mut Self::Message);
    fn set_message_frame(entry: &mut Self::Message, image: Image);

    fn with_models<R>(
        f: impl FnOnce(
            &VecModel<Self::Message>,
            &VecModel<Self::Room>,
            &VecModel<Self::Space>,
            &VecModel<Self::Space>,
        ) -> R,
    ) -> Option<R>;
    fn with_timeline<R>(f: impl FnOnce(&VecModel<Self::Message>) -> R) -> Option<R>;
    fn with_stickers<R>(
        f: impl FnOnce(&VecModel<Self::StickerRow>, &VecModel<Self::StickerPack>) -> R,
    ) -> Option<R>;
}

pub struct UiEventContext<'a, B: UiBackend> {
    pub timeline: &'a VecModel<B::Message>,
    pub rooms: &'a VecModel<B::Room>,
    pub spaces: &'a VecModel<B::Space>,
    pub subspaces: &'a VecModel<B::Space>,
    pub media: &'a dyn MediaCache,
}

pub fn post_effect<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    media: Arc<dyn MediaCache>,
    event: Effect,
    permit: OwnedSemaphorePermit,
) {
    weak.upgrade_in_event_loop(move |w| {
        B::with_models(move |timeline, rooms, spaces, subspaces| {
            let ctx = UiEventContext::<B> {
                timeline,
                rooms,
                spaces,
                subspaces,
                media: media.as_ref(),
            };
            dispatch_effect::<B>(&w, event, &ctx);
        });
        drop(permit);
    })
    .ok();
}

pub fn install_render_hooks<B: UiBackend>(weak: slint::Weak<B::Window>) {
    set_animation_tick(tick_animations::<B>);

    set_image_ready({
        let weak = weak.clone();
        move |unique_id, outcome| {
            apply_thumbnail_ready::<B>(unique_id, outcome);
            if let Some(w) = weak.upgrade() {
                w.window().request_redraw();
            }
        }
    });

    set_avatar_ready(move |slots, outcome| {
        apply_avatar_ready::<B>(&weak, slots, outcome);
        if let Some(w) = weak.upgrade() {
            w.window().request_redraw();
        }
    });
}

pub fn selected_room_key<B: UiBackend>(weak: &slint::Weak<B::Window>) -> Option<(RoomId, i32)> {
    let w = weak.upgrade()?;
    let room_id = w.get_string(StringProp::SelectedRoomId).to_string();
    if room_id.is_empty() {
        return None;
    }
    Some((RoomId::new(room_id), w.get_int(IntProp::SelectedGeneration)))
}

fn tick_animations<B: UiBackend>() {
    advance_animations(&mut |key, hint, frame| {
        place_timeline_frame::<B>(key, hint, &frame)
            .or_else(|| place_sticker_image::<B>(key, Some(&frame)))
    });
}

fn place_timeline_frame<B: UiBackend>(key: &str, hint: usize, frame: &Image) -> Option<usize> {
    B::with_timeline(|timeline| {
        let row = locate_row(timeline, &B::message_id, key, hint)?;
        let entry = timeline.row_data(row)?;
        let mut updated = entry;
        B::set_message_frame(&mut updated, frame.clone());
        timeline.set_row_data(row, updated);
        Some(row)
    })
    .flatten()
}

fn place_sticker_image<B: UiBackend>(key: &str, art: Option<&Image>) -> Option<usize> {
    B::with_stickers(|rows, packs| {
        let mut placed = None;
        for row in 0..rows.row_count() {
            let Some(entry) = rows.row_data(row) else {
                continue;
            };
            if B::patch_sticker_cell(&entry, key, art) {
                placed = Some(row);
                break;
            }
        }
        if let Some(art) = art {
            adopt_pack_icon::<B>(packs, key, art);
        }
        placed
    })
    .flatten()
}

fn adopt_pack_icon<B: UiBackend>(packs: &VecModel<B::StickerPack>, key: &str, image: &Image) {
    let Some(pack_id) = key.split(CELL_KEY_SEPARATOR).next() else {
        return;
    };
    let mut tabs: Vec<B::StickerPack> = packs.iter().collect();
    let mut adopted = false;
    for tab in &mut tabs {
        if let Some(updated) = B::sticker_pack_with_icon(tab, pack_id, image) {
            *tab = updated;
            adopted = true;
            break;
        }
    }
    if adopted {
        packs.set_vec(tabs);
    }
}

fn apply_thumbnail_ready<B: UiBackend>(unique_id: &str, outcome: DecodeOutcome<'_>) {
    if matches!(outcome, DecodeOutcome::Deferred) {
        return;
    }
    B::with_timeline(|timeline| {
        patch_first_row(
            timeline,
            |entry| B::message_id(entry) == unique_id,
            |entry| match outcome {
                DecodeOutcome::Ready(image) => B::set_message_thumbnail(entry, image),
                DecodeOutcome::Failed => B::set_message_media_failed(entry),
                DecodeOutcome::Deferred => {}
            },
        );
    });
    let image = match outcome {
        DecodeOutcome::Ready(image) => Some(image),
        DecodeOutcome::Failed | DecodeOutcome::Deferred => None,
    };
    place_sticker_image::<B>(unique_id, image);
}

#[derive(Default)]
struct AvatarTargets<'a> {
    messages: HashSet<&'a str>,
    rooms: HashSet<&'a str>,
    spaces: HashSet<&'a str>,
    user: bool,
}

fn group_slots(slots: &[AvatarSlot]) -> AvatarTargets<'_> {
    let mut targets = AvatarTargets::default();
    for slot in slots {
        match slot {
            AvatarSlot::Message(id) => {
                targets.messages.insert(id.as_str());
            }
            AvatarSlot::Room(id) => {
                targets.rooms.insert(id.as_str());
            }
            AvatarSlot::Space(id) => {
                targets.spaces.insert(id.as_str());
            }
            AvatarSlot::User => targets.user = true,
        }
    }
    targets
}

fn apply_avatar_ready<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    slots: &[AvatarSlot],
    outcome: DecodeOutcome<'_>,
) {
    let DecodeOutcome::Ready(image) = outcome else {
        return;
    };
    let targets = group_slots(slots);
    if targets.user
        && let Some(w) = weak.upgrade()
    {
        w.apply_user_avatar(Some(image.clone()));
    }
    B::with_models(|timeline, rooms, spaces, subspaces| {
        patch_rows_by_id(timeline, &targets.messages, &B::message_id, |entry| {
            B::set_message_avatar(entry, image);
        });
        patch_rows_by_id(rooms, &targets.rooms, &B::room_id, |entry| {
            B::set_room_avatar(entry, image);
        });
        patch_rows_by_id(spaces, &targets.spaces, &B::space_id, |entry| {
            B::set_space_avatar(entry, image);
        });
        patch_rows_by_id(subspaces, &targets.spaces, &B::space_id, |entry| {
            B::set_space_avatar(entry, image);
        });
    });
}
