use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use super::backend::{Models, UiBackend};
use super::decode::MediaSession;
use super::dto::PackBrowse;
use super::emote::EmoteBindings;
use super::props::{StringProp, UiProps};
use super::reconcile::{PackIndex, TimelineIndex};
use super::reduce::{RoomCursor, SheetAvatar};
use super::richtext::{CoverLabels, StyledBodies, recognise_cover_labels};
use super::sending::SendingRoom;
use super::spoilers::SpoilerRows;
use crate::commands::view::AppViewState;

#[cfg(not(feature = "interpreted"))]
pub type ActiveBackend = super::compiled::CompiledBackend;
#[cfg(feature = "interpreted")]
pub type ActiveBackend = super::interpreted::InterpretedBackend;

pub struct UiSession<B: UiBackend> {
    pub models: Rc<Models<B>>,
    pub timeline_index: TimelineIndex,
    pub pack_index: PackIndex,
    pub room: RoomCursor,
    pub invite_avatar: SheetAvatar,
    pub removal_avatar: SheetAvatar,
    pub room_preview_avatar: SheetAvatar,
    pub snapshot: Option<Arc<AppViewState>>,
    pub pack_browse: Option<PackBrowse>,
    pub emotes: EmoteBindings,
    pub sending: Option<SendingRoom>,
    pub media: MediaSession,
    pub bodies: StyledBodies,
    pub spoilers: SpoilerRows,
    pub drafts: HashMap<String, B::Draft>,
}

impl<B: UiBackend> Default for UiSession<B> {
    fn default() -> Self {
        Self {
            models: Rc::default(),
            timeline_index: TimelineIndex::default(),
            pack_index: PackIndex::default(),
            room: RoomCursor::default(),
            invite_avatar: SheetAvatar::default(),
            removal_avatar: SheetAvatar::default(),
            room_preview_avatar: SheetAvatar::default(),
            snapshot: None,
            pack_browse: None,
            emotes: EmoteBindings::default(),
            sending: None,
            media: MediaSession::default(),
            bodies: StyledBodies::default(),
            spoilers: SpoilerRows::default(),
            drafts: HashMap::new(),
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
    recognise_cover_labels(CoverLabels {
        label: window.get_string(StringProp::SpoilerLabel).to_string(),
        reason_label: window
            .get_string(StringProp::SpoilerReasonLabel)
            .to_string(),
        reason_slot: window.get_string(StringProp::SpoilerReasonSlot).to_string(),
    });
    let models = B::models();
    B::attach_models(window, &models);
    models
}
