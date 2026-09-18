use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use super::backend::{Models, UiBackend};
use super::decode::MediaSession;
use super::reconcile::{StickerIndex, TimelineIndex};
use super::reduce::RoomCursor;
use super::richtext::StyledBodies;
use crate::commands::view::AppViewState;

#[cfg(not(feature = "interpreted"))]
pub type ActiveBackend = super::compiled::CompiledBackend;
#[cfg(feature = "interpreted")]
pub type ActiveBackend = super::interpreted::InterpretedBackend;

pub struct UiSession<B: UiBackend> {
    pub models: Rc<Models<B>>,
    pub timeline_index: TimelineIndex,
    pub sticker_index: StickerIndex,
    pub room: RoomCursor,
    pub snapshot: Option<Arc<AppViewState>>,
    pub sticker_needle: String,
    pub media: MediaSession,
    pub bodies: StyledBodies,
}

impl<B: UiBackend> Default for UiSession<B> {
    fn default() -> Self {
        Self {
            models: Rc::default(),
            timeline_index: TimelineIndex::default(),
            sticker_index: StickerIndex::default(),
            room: RoomCursor::default(),
            snapshot: None,
            sticker_needle: String::new(),
            media: MediaSession::default(),
            bodies: StyledBodies::default(),
        }
    }
}

thread_local! {
    static SESSION: RefCell<UiSession<ActiveBackend>> = RefCell::new(UiSession::default());
}

pub fn with_session<R>(f: impl FnOnce(&mut UiSession<ActiveBackend>) -> R) -> R {
    SESSION.with_borrow_mut(f)
}

pub fn active_models() -> Rc<Models<ActiveBackend>> {
    with_session(|session| Rc::clone(&session.models))
}

pub fn begin_session<B: UiBackend>(window: &B::Window) -> Rc<Models<B>> {
    with_session(|session| *session = UiSession::default());
    let models = B::models();
    B::attach_models(window, &models);
    models
}
