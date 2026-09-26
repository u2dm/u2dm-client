use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, VecModel};
use tokio::sync::{OwnedSemaphorePermit, mpsc, watch};

use super::clock::install_clock_invalidation;
use super::decode::{
    AvatarSlot, DecodeOutcome, MediaSlot, advance_animations, set_animation_tick, set_avatar_ready,
    set_image_ready,
};
use super::dto::{
    MediaFailureKind, MediaState, MessageDto, RoomDto, SpaceChildDto, SpaceDto, StickerPackDto,
    StickerRowDto, ThumbUpdate, cell_pack, decode_failure_kind, enrich_to_update, message_to_dto,
    room_to_dto, space_child_to_dto, space_to_dto,
};
use super::fields::{
    MessageFields, PollAnswerFields, ReactionFields, ReactorFields, RoomFields, SpaceChildFields,
    SpaceFields, StickerCellFields, StickerPackFields, StickerRowFields,
};
use super::multiplex::spawn_event_multiplexer;
use super::props::{IntProp, StringProp, UiProps};
use super::reconcile::{reorder_rows, sticker_cell_row, sticker_pack_row, timeline_row_of};
use super::reduce::{dispatch_effect, is_latest_adoption, set_sticker_query};
use super::rows::{locate_row, patch_rows_by_id};
use super::schema::model_props;
use super::session::begin_session;
use super::splice_model::SpliceModel;
use super::window_events::install_window_events;
use crate::commands::effects::Effect;
use crate::commands::view::{AppViewState, SpaceIndexRow};
use crate::domain::message::TimelineMessage;
use crate::domain::room::{Room, RoomId, Space};
use crate::domain::timeline::EnrichmentDelta;
use crate::ports::media::MediaCache;

pub trait UiBackend: Sized + 'static {
    type Window: ComponentHandle + UiProps + 'static;
    type Message: MessageFields<Self> + Clone + From<MessageDto> + 'static;
    type Reaction: ReactionFields<Self> + Clone + 'static;
    type Reactor: ReactorFields<Self> + Clone + 'static;
    type PollAnswer: PollAnswerFields<Self> + Clone + 'static;
    type Room: RoomFields<Self> + Clone + PartialEq + From<RoomDto> + 'static;
    type Space: SpaceFields<Self> + Clone + PartialEq + From<SpaceDto> + 'static;
    type SpaceChild: SpaceChildFields<Self> + Clone + PartialEq + From<SpaceChildDto> + 'static;
    type StickerRow: StickerRowFields<Self> + Clone + From<StickerRowDto> + 'static;
    type StickerCell: StickerCellFields<Self> + Clone + 'static;
    type StickerPack: StickerPackFields<Self> + Clone + From<StickerPackDto> + 'static;

    fn models() -> Rc<Models<Self>>;
    fn attach_models(window: &Self::Window, models: &Models<Self>);
    fn bind_sticker_search(window: &Self::Window, search: impl Fn(&str) + 'static);

    fn convert_message(message: &TimelineMessage, media: &dyn MediaCache) -> Self::Message {
        message_to_dto(message, media).into()
    }

    fn convert_room(room: &Room, media: &dyn MediaCache) -> Self::Room {
        room_to_dto(room, media).into()
    }

    fn convert_space(space: &Space, media: &dyn MediaCache) -> Self::Space {
        space_to_dto(space, media).into()
    }

    fn convert_space_child(row: &SpaceIndexRow, media: &dyn MediaCache) -> Self::SpaceChild {
        space_child_to_dto(row, media).into()
    }

    fn with_models<R>(
        f: impl FnOnce(
            &SpliceModel<Self::Message>,
            &VecModel<Self::Room>,
            &VecModel<Self::Space>,
            &VecModel<Self::Space>,
        ) -> R,
    ) -> R {
        let models = Self::models();
        f(
            &models.timeline,
            &models.rooms,
            &models.spaces,
            &models.subspaces,
        )
    }

    fn with_timeline<R>(f: impl FnOnce(&SpliceModel<Self::Message>) -> R) -> R {
        f(&Self::models().timeline)
    }

    fn with_stickers<R>(
        f: impl FnOnce(&SpliceModel<Self::StickerRow>, &VecModel<Self::StickerPack>) -> R,
    ) -> R {
        let models = Self::models();
        f(&models.sticker_rows, &models.sticker_packs)
    }
}

macro_rules! declare_models {
    ($($field:ident $row:ident $model:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        pub struct Models<B: UiBackend> {
            $(pub $field: Rc<$model<B::$row>>,)*
        }

        impl<B: UiBackend> Default for Models<B> {
            fn default() -> Self {
                Self { $($field: Rc::default(),)* }
            }
        }
    };
}

model_props!(declare_models);

pub fn spawn_event_handler<B: UiBackend>(
    window: &B::Window,
    ui_rx: mpsc::Receiver<Effect>,
    view_rx: watch::Receiver<Arc<AppViewState>>,
    media_cache: Arc<dyn MediaCache>,
) {
    begin_session::<B>(window);

    install_render_hooks::<B>(window.as_weak());
    install_clock_invalidation::<B>(Arc::clone(&media_cache));
    install_window_events::<B>(window);

    let media = Arc::clone(&media_cache);
    B::bind_sticker_search(window, move |query| {
        set_sticker_query::<B>(query, media.as_ref());
    });

    let weak = window.as_weak();
    spawn_event_multiplexer(ui_rx, view_rx, media_cache, move |event, media, permit| {
        post_effect::<B>(&weak, media, event, permit);
    });
}

pub fn reorder_spaces<B: UiBackend>(from: usize, to: usize) {
    B::with_models(|_timeline, _rooms, spaces, _subspaces| reorder_rows(spaces, from, to));
}

pub struct UiEventContext<'a, B: UiBackend> {
    pub models: &'a Models<B>,
    pub media: &'a dyn MediaCache,
}

fn post_effect<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    media: Arc<dyn MediaCache>,
    event: Effect,
    permit: OwnedSemaphorePermit,
) {
    weak.upgrade_in_event_loop(move |w| {
        let models = B::models();
        let ctx = UiEventContext::<B> {
            models: &models,
            media: media.as_ref(),
        };
        dispatch_effect::<B>(&w, event, &ctx);
        drop(permit);
    })
    .ok();
}

fn install_render_hooks<B: UiBackend>(weak: slint::Weak<B::Window>) {
    set_animation_tick(tick_animations::<B>);

    set_image_ready({
        let weak = weak.clone();
        move |slot, outcome| {
            apply_thumbnail_ready::<B>(slot, outcome);
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

pub fn adopted_room_key<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    seen_timeline_token: i32,
) -> Option<(RoomId, i32)> {
    selected_room_key::<B>(weak)
        .filter(|(_, generation)| is_latest_adoption(*generation, seen_timeline_token))
}

pub fn unread_below<B: UiBackend>(first_unseen_row: i32) -> u32 {
    let from = usize::try_from(first_unseen_row).unwrap_or(0);
    let below = B::with_timeline(|timeline| {
        timeline.count_tail(from, |entry: &B::Message| entry.counts_as_unread())
    });
    u32::try_from(below).unwrap_or(u32::MAX)
}

pub fn enrich_message<B: UiBackend>(
    entry: &mut B::Message,
    delta: &EnrichmentDelta,
    media: &dyn MediaCache,
) {
    let update = enrich_to_update(delta, media);
    match update.thumbnail {
        ThumbUpdate::Ready(image) => show_thumbnail::<B>(entry, image),
        ThumbUpdate::Failed(reason) => show_media_failure::<B>(entry, reason),
        ThumbUpdate::Unchanged => {}
    }
    if let Some(image) = update.avatar {
        entry.set_avatar(image);
        entry.set_has_avatar(true);
    }
    if let Some(pronouns) = update.pronouns {
        entry.set_pronouns(pronouns);
    }
}

fn show_thumbnail<B: UiBackend>(entry: &mut B::Message, image: Image) {
    entry.set_thumbnail(image);
    entry.set_media_state(MediaState::Ready);
}

fn show_media_failure<B: UiBackend>(entry: &mut B::Message, reason: MediaFailureKind) {
    entry.set_media_state(MediaState::Failed);
    entry.set_media_failure(reason);
}

fn tick_animations<B: UiBackend>() {
    advance_animations(&mut |slot, hint, frame| match slot {
        MediaSlot::Thumbnail(item) => patch_timeline_row::<B>(item.unique_id(), hint, |entry| {
            show_thumbnail::<B>(entry, frame);
        }),
        MediaSlot::StickerCell(key) => place_sticker_cell::<B>(key, Some(&frame)),
    });
}

fn patch_timeline_row<B: UiBackend>(
    unique_id: &str,
    hint: usize,
    apply: impl FnOnce(&mut B::Message),
) -> Option<usize> {
    B::with_timeline(|timeline| {
        let hint = timeline_row_of(unique_id).unwrap_or(hint);
        let row = locate_row(timeline, &B::Message::unique_id, unique_id, hint)?;
        let mut entry = timeline.row_data(row)?;
        apply(&mut entry);
        timeline.set_row_data(row, entry);
        Some(row)
    })
}

fn place_sticker_cell<B: UiBackend>(key: &str, art: Option<&Image>) -> Option<usize> {
    let row = sticker_cell_row(key)?;
    B::with_stickers(|rows, _| {
        let entry = rows.row_data(row)?;
        patch_sticker_cell::<B>(&entry, key, art).then_some(row)
    })
}

fn patch_sticker_cell<B: UiBackend>(row: &B::StickerRow, key: &str, art: Option<&Image>) -> bool {
    let cells = row.cells();
    let Some(index) = cells.iter().position(|cell| cell.key() == key) else {
        return false;
    };
    let Some(mut cell) = cells.row_data(index) else {
        return false;
    };
    match art {
        Some(art) => {
            cell.set_image(art.clone());
            cell.set_media_state(MediaState::Ready);
        }
        None => cell.set_media_state(MediaState::Failed),
    }
    cells.set_row_data(index, cell);
    true
}

fn adopt_pack_icon<B: UiBackend>(pack_id: &str, image: &Image) {
    let Some(row) = sticker_pack_row(pack_id) else {
        return;
    };
    B::with_stickers(|_, packs| {
        let Some(updated) = packs
            .row_data(row)
            .and_then(|tab| pack_with_icon::<B>(&tab, pack_id, image))
        else {
            return;
        };
        packs.remove(row);
        packs.insert(row, updated);
    });
}

fn pack_with_icon<B: UiBackend>(
    pack: &B::StickerPack,
    pack_id: &str,
    image: &Image,
) -> Option<B::StickerPack> {
    if pack.has_icon() || pack.id() != pack_id {
        return None;
    }
    let mut updated = pack.clone();
    updated.set_icon(image.clone());
    updated.set_has_icon(true);
    Some(updated)
}

pub(super) fn apply_thumbnail_ready<B: UiBackend>(slot: &MediaSlot, outcome: DecodeOutcome<'_>) {
    let art = match outcome {
        DecodeOutcome::Ready(image) => Ok(image),
        DecodeOutcome::Failed(failure) => Err(failure),
        DecodeOutcome::Deferred => return,
    };
    match slot {
        MediaSlot::Thumbnail(item) => {
            let unique_id = item.unique_id();
            let placed = patch_timeline_row::<B>(unique_id, 0, |entry| match art {
                Ok(image) => show_thumbnail::<B>(entry, image.clone()),
                Err(failure) => show_media_failure::<B>(entry, decode_failure_kind(failure)),
            });
            if placed.is_none() {
                tracing::debug!(
                    unique_id,
                    "dropped a decoded image with no live timeline row"
                );
            }
        }
        MediaSlot::StickerCell(key) => apply_sticker_art::<B>(key, art.ok()),
    }
}

pub(super) fn apply_sticker_art<B: UiBackend>(key: &str, art: Option<&Image>) {
    if place_sticker_cell::<B>(key, art).is_none() {
        tracing::debug!(key, "dropped a decoded image with no live sticker cell");
    }
    if let Some(image) = art {
        adopt_pack_icon::<B>(cell_pack(key), image);
    }
}

#[derive(Default)]
struct AvatarTargets<'a> {
    messages: HashSet<&'a str>,
    reactors_by_message: HashMap<&'a str, HashSet<&'a str>>,
    rooms: HashSet<&'a str>,
    spaces: HashSet<&'a str>,
    space_children: HashSet<&'a str>,
    user: bool,
    attachment_preview: bool,
}

fn group_slots(slots: &[AvatarSlot]) -> AvatarTargets<'_> {
    let mut targets = AvatarTargets::default();
    for slot in slots {
        match slot {
            AvatarSlot::Message(item) => {
                targets.messages.insert(item.unique_id());
            }
            AvatarSlot::Reactor { item, user_id } => {
                targets
                    .reactors_by_message
                    .entry(item.unique_id())
                    .or_default()
                    .insert(user_id.as_str());
            }
            AvatarSlot::Room(id) => {
                targets.rooms.insert(id.as_str());
            }
            AvatarSlot::Space(id) => {
                targets.spaces.insert(id.as_str());
            }
            AvatarSlot::SpaceChild(id) => {
                targets.space_children.insert(id.as_str());
            }
            AvatarSlot::User => targets.user = true,
            AvatarSlot::AttachmentPreview { .. } => targets.attachment_preview = true,
        }
    }
    targets
}

fn indexed_timeline_row<B: UiBackend>(
    timeline: &SpliceModel<B::Message>,
    unique_id: &str,
) -> Option<usize> {
    locate_row(
        timeline,
        &B::Message::unique_id,
        unique_id,
        timeline_row_of(unique_id)?,
    )
}

fn patch_message_avatars<B: UiBackend>(
    timeline: &SpliceModel<B::Message>,
    unique_ids: &HashSet<&str>,
    image: &Image,
) {
    for unique_id in unique_ids {
        let Some(row) = indexed_timeline_row::<B>(timeline, unique_id) else {
            continue;
        };
        let Some(mut entry) = timeline.row_data(row) else {
            continue;
        };
        entry.set_avatar(image.clone());
        entry.set_has_avatar(true);
        timeline.set_row_data(row, entry);
    }
}

fn patch_reactor_avatars<B: UiBackend>(
    timeline: &SpliceModel<B::Message>,
    reactors_by_message: &HashMap<&str, HashSet<&str>>,
    image: &Image,
) {
    for (unique_id, user_ids) in reactors_by_message {
        let Some(entry) =
            indexed_timeline_row::<B>(timeline, unique_id).and_then(|row| timeline.row_data(row))
        else {
            continue;
        };
        for user_id in user_ids {
            patch_reactor_avatar::<B>(&entry, user_id, image);
        }
    }
}

fn patch_reactor_avatar<B: UiBackend>(entry: &B::Message, user_id: &str, image: &Image) {
    for reaction in entry.reactions().iter() {
        let faces = reaction.avatars();
        let Some(index) = faces
            .iter()
            .position(|face| face.user_id() == user_id && !face.has_avatar())
        else {
            continue;
        };
        let Some(mut face) = faces.row_data(index) else {
            continue;
        };
        face.set_avatar(image.clone());
        face.set_has_avatar(true);
        faces.set_row_data(index, face);
    }
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
    if (targets.user || targets.attachment_preview)
        && let Some(w) = weak.upgrade()
    {
        if targets.user {
            w.apply_user_avatar(Some(image.clone()));
        }
        if targets.attachment_preview {
            w.apply_attachment_preview(Some(image.clone()));
        }
    }
    B::with_models(|timeline, rooms, spaces, subspaces| {
        patch_message_avatars::<B>(timeline, &targets.messages, image);
        patch_reactor_avatars::<B>(timeline, &targets.reactors_by_message, image);
        patch_rows_by_id(rooms, &targets.rooms, &B::Room::id, |entry| {
            entry.set_avatar(image.clone());
            entry.set_has_avatar(true);
        });
        patch_rows_by_id(spaces, &targets.spaces, &B::Space::id, |entry| {
            entry.set_avatar(image.clone());
            entry.set_has_avatar(true);
        });
        patch_rows_by_id(subspaces, &targets.spaces, &B::Space::id, |entry| {
            entry.set_avatar(image.clone());
            entry.set_has_avatar(true);
        });
    });
    if !targets.space_children.is_empty() {
        let models = B::models();
        patch_rows_by_id(
            &*models.space_children,
            &targets.space_children,
            &B::SpaceChild::id,
            |entry| {
                entry.set_avatar(image.clone());
                entry.set_has_avatar(true);
            },
        );
    }
}
