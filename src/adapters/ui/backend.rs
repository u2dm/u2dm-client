use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, VecModel};
use tokio::sync::{OwnedSemaphorePermit, mpsc, watch};

use super::clock::install_clock_invalidation;
use super::decode::{
    AvatarSlot, DecodeOutcome, advance_animations, set_animation_tick, set_avatar_ready,
    set_image_ready,
};
use super::dto::{
    AudioRowUpdate, DecodeTarget, MediaFailureKind, MessageDto, RoomDto, SpaceDto, StickerPackDto,
    StickerRowDto, message_to_dto, room_to_dto, space_to_dto,
};
use super::multiplex::spawn_event_multiplexer;
use super::props::{IntProp, StringProp, UiProps};
use super::reconcile::{reorder_rows, sticker_cell_row, sticker_pack_row, timeline_row_of};
use super::reduce::{dispatch_effect, set_sticker_query};
use super::rows::{locate_row, patch_rows_by_id};
use super::schema::model_props;
use super::splice_model::SpliceModel;
use crate::commands::effects::Effect;
use crate::commands::view::AppViewState;
use crate::domain::message::TimelineMessage;
use crate::domain::room::{Room, RoomId, Space};
use crate::domain::timeline::EnrichmentDelta;
use crate::ports::media::MediaCache;

pub trait UiBackend: Sized + 'static {
    type Window: ComponentHandle + UiProps + 'static;
    type Message: Clone + From<MessageDto> + 'static;
    type Room: Clone + PartialEq + From<RoomDto> + 'static;
    type Space: Clone + PartialEq + From<SpaceDto> + 'static;
    type StickerRow: Clone + From<StickerRowDto> + 'static;
    type StickerPack: Clone + From<StickerPackDto> + 'static;

    fn attach_models(window: &Self::Window, models: &Models<Self>);
    fn bind_sticker_search(window: &Self::Window, search: impl Fn(&str) + 'static);

    fn enrich_message(entry: &mut Self::Message, delta: &EnrichmentDelta, media: &dyn MediaCache);
    fn patch_sticker_cell(row: &Self::StickerRow, key: &str, art: Option<&Image>) -> bool;
    fn patch_reactor_avatar(entry: &Self::Message, user_id: &str, image: &Image) -> bool;
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
    fn set_message_media_failed(entry: &mut Self::Message, reason: MediaFailureKind);
    fn set_message_audio(entry: &mut Self::Message, update: &AudioRowUpdate);

    fn convert_message(message: &TimelineMessage, media: &dyn MediaCache) -> Self::Message {
        message_to_dto(message, media).into()
    }

    fn convert_room(room: &Room, media: &dyn MediaCache) -> Self::Room {
        room_to_dto(room, media).into()
    }

    fn convert_space(space: &Space, media: &dyn MediaCache) -> Self::Space {
        space_to_dto(space, media).into()
    }

    fn with_models<R>(
        f: impl FnOnce(
            &SpliceModel<Self::Message>,
            &VecModel<Self::Room>,
            &VecModel<Self::Space>,
            &VecModel<Self::Space>,
        ) -> R,
    ) -> Option<R> {
        let models = models::<Self>()?;
        Some(f(
            &models.timeline,
            &models.rooms,
            &models.spaces,
            &models.subspaces,
        ))
    }

    fn with_timeline<R>(f: impl FnOnce(&SpliceModel<Self::Message>) -> R) -> Option<R> {
        models::<Self>().map(|models| f(&models.timeline))
    }

    fn with_stickers<R>(
        f: impl FnOnce(&SpliceModel<Self::StickerRow>, &VecModel<Self::StickerPack>) -> R,
    ) -> Option<R> {
        models::<Self>().map(|models| f(&models.sticker_rows, &models.sticker_packs))
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

thread_local! {
    static MODELS: RefCell<Option<Rc<dyn Any>>> = const { RefCell::new(None) };
}

fn models<B: UiBackend>() -> Option<Rc<Models<B>>> {
    MODELS.with(|cell| cell.borrow().clone())?.downcast().ok()
}

pub fn spawn_event_handler<B: UiBackend>(
    window: &B::Window,
    ui_rx: mpsc::Receiver<Effect>,
    view_rx: watch::Receiver<Arc<AppViewState>>,
    media_cache: Arc<dyn MediaCache>,
) {
    let models = Rc::new(Models::<B>::default());
    B::attach_models(window, &models);
    MODELS.with(|cell| *cell.borrow_mut() = Some(models));

    install_render_hooks::<B>(window.as_weak());
    install_clock_invalidation::<B>(Arc::clone(&media_cache));

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
    pub timeline: &'a SpliceModel<B::Message>,
    pub rooms: &'a VecModel<B::Room>,
    pub spaces: &'a VecModel<B::Space>,
    pub subspaces: &'a VecModel<B::Space>,
    pub media: &'a dyn MediaCache,
}

fn post_effect<B: UiBackend>(
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

fn install_render_hooks<B: UiBackend>(weak: slint::Weak<B::Window>) {
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
    advance_animations(&mut |key, hint, frame| match DecodeTarget::of(key) {
        DecodeTarget::Timeline { unique_id } => patch_timeline_row::<B>(unique_id, hint, |entry| {
            B::set_message_thumbnail(entry, &frame);
        }),
        DecodeTarget::StickerCell { key, .. } => place_sticker_cell::<B>(key, Some(&frame)),
    });
}

fn patch_timeline_row<B: UiBackend>(
    unique_id: &str,
    hint: usize,
    apply: impl FnOnce(&mut B::Message),
) -> Option<usize> {
    B::with_timeline(|timeline| {
        let hint = timeline_row_of(unique_id).unwrap_or(hint);
        let row = locate_row(timeline, &B::message_id, unique_id, hint)?;
        let mut entry = timeline.row_data(row)?;
        apply(&mut entry);
        timeline.set_row_data(row, entry);
        Some(row)
    })
    .flatten()
}

fn place_sticker_cell<B: UiBackend>(key: &str, art: Option<&Image>) -> Option<usize> {
    let row = sticker_cell_row(key)?;
    B::with_stickers(|rows, _| {
        let entry = rows.row_data(row)?;
        B::patch_sticker_cell(&entry, key, art).then_some(row)
    })
    .flatten()
}

fn adopt_pack_icon<B: UiBackend>(pack_id: &str, image: &Image) {
    let Some(row) = sticker_pack_row(pack_id) else {
        return;
    };
    B::with_stickers(|_, packs| {
        let Some(updated) = packs
            .row_data(row)
            .and_then(|tab| B::sticker_pack_with_icon(&tab, pack_id, image))
        else {
            return;
        };
        packs.remove(row);
        packs.insert(row, updated);
    });
}

pub(super) fn apply_thumbnail_ready<B: UiBackend>(key: &str, outcome: DecodeOutcome<'_>) {
    let art = match outcome {
        DecodeOutcome::Ready(image) => Some(image),
        DecodeOutcome::Failed => None,
        DecodeOutcome::Deferred => return,
    };
    match DecodeTarget::of(key) {
        DecodeTarget::Timeline { unique_id } => {
            let placed = patch_timeline_row::<B>(unique_id, 0, |entry| match art {
                Some(image) => B::set_message_thumbnail(entry, image),
                None => {
                    B::set_message_media_failed(entry, MediaFailureKind::Unreadable);
                }
            });
            if placed.is_none() {
                tracing::debug!(
                    unique_id,
                    "dropped a decoded image with no live timeline row"
                );
            }
        }
        DecodeTarget::StickerCell { key, pack } => {
            if place_sticker_cell::<B>(key, art).is_none() {
                tracing::debug!(key, "dropped a decoded image with no live sticker cell");
            }
            if let Some(image) = art {
                adopt_pack_icon::<B>(pack, image);
            }
        }
    }
}

#[derive(Default)]
struct AvatarTargets<'a> {
    messages: HashSet<&'a str>,
    reactors_by_message: HashMap<&'a str, HashSet<&'a str>>,
    rooms: HashSet<&'a str>,
    spaces: HashSet<&'a str>,
    user: bool,
    attachment_preview: bool,
}

fn group_slots(slots: &[AvatarSlot]) -> AvatarTargets<'_> {
    let mut targets = AvatarTargets::default();
    for slot in slots {
        match slot {
            AvatarSlot::Message(id) => {
                targets.messages.insert(id.as_str());
            }
            AvatarSlot::Reactor { unique_id, user_id } => {
                targets
                    .reactors_by_message
                    .entry(unique_id.as_str())
                    .or_default()
                    .insert(user_id.as_str());
            }
            AvatarSlot::Room(id) => {
                targets.rooms.insert(id.as_str());
            }
            AvatarSlot::Space(id) => {
                targets.spaces.insert(id.as_str());
            }
            AvatarSlot::User => targets.user = true,
            AvatarSlot::AttachmentPreview => targets.attachment_preview = true,
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
        &B::message_id,
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
        B::set_message_avatar(&mut entry, image);
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
            B::patch_reactor_avatar(&entry, user_id, image);
        }
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
