use std::sync::Arc;

use slint::{ComponentHandle, Image, VecModel};
use tokio::sync::OwnedSemaphorePermit;

use super::decode::{
    AvatarSlot, DecodeOutcome, advance_animations, patch_rows, set_animation_tick,
    set_avatar_ready, set_image_ready,
};
use super::props::{IntProp, StringProp, UiProps};
use super::reduce::dispatch_effect;
use crate::commands::Effect;
use crate::domain::models::{EnrichmentDelta, Room, RoomId, Space, TimelineMessage};
use crate::ports::media::MediaCache;

pub trait UiBackend: Sized + 'static {
    type Window: ComponentHandle + UiProps + 'static;
    type Message: Clone + 'static;
    type Room: Clone + PartialEq + 'static;
    type Space: Clone + PartialEq + 'static;

    fn convert_message(message: &TimelineMessage, media: &dyn MediaCache) -> Self::Message;
    fn enrich_message(entry: &mut Self::Message, delta: &EnrichmentDelta, media: &dyn MediaCache);
    fn convert_room(room: &Room, media: &dyn MediaCache) -> Self::Room;
    fn convert_space(space: &Space, media: &dyn MediaCache) -> Self::Space;

    fn message_id(entry: &Self::Message) -> String;
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
    B::with_timeline(|timeline| {
        advance_animations(timeline, &|entry| B::message_id(entry), &|entry, frame| {
            B::set_message_frame(entry, frame);
        });
    });
}

fn apply_thumbnail_ready<B: UiBackend>(unique_id: &str, outcome: DecodeOutcome<'_>) {
    if matches!(outcome, DecodeOutcome::Deferred) {
        return;
    }
    B::with_timeline(|timeline| {
        patch_rows(
            timeline,
            |entry| B::message_id(entry).as_str() == unique_id,
            |entry| match outcome {
                DecodeOutcome::Ready(image) => B::set_message_thumbnail(entry, image),
                DecodeOutcome::Failed => B::set_message_media_failed(entry),
                DecodeOutcome::Deferred => {}
            },
        );
    });
}

fn apply_avatar_ready<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    slots: &[AvatarSlot],
    outcome: DecodeOutcome<'_>,
) {
    let DecodeOutcome::Ready(image) = outcome else {
        return;
    };
    B::with_models(|timeline, rooms, spaces, subspaces| {
        for slot in slots {
            match slot {
                AvatarSlot::Message(unique_id) => patch_rows(
                    timeline,
                    |entry| B::message_id(entry).as_str() == unique_id.as_str(),
                    |entry| B::set_message_avatar(entry, image),
                ),
                AvatarSlot::Room(id) => patch_rows(
                    rooms,
                    |entry| B::room_id(entry) == id.as_str(),
                    |entry| B::set_room_avatar(entry, image),
                ),
                AvatarSlot::Space(id) => {
                    patch_rows(
                        spaces,
                        |entry| B::space_id(entry) == id.as_str(),
                        |entry| B::set_space_avatar(entry, image),
                    );
                    patch_rows(
                        subspaces,
                        |entry| B::space_id(entry) == id.as_str(),
                        |entry| B::set_space_avatar(entry, image),
                    );
                }
                AvatarSlot::User => {
                    if let Some(w) = weak.upgrade() {
                        w.apply_user_avatar(Some(image.clone()));
                    }
                }
            }
        }
    });
}
