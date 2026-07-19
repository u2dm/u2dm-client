use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};
use tokio::runtime::Runtime;
use tokio::sync::{OwnedSemaphorePermit, mpsc, watch};

use super::common::{BoolProp, IntProp, StringProp, UiProps, dispatch_ui_event, reorder_rows};
use super::decode::{
    AvatarSlot, advance_animations, patch_rows, request_media, set_animation_tick,
    set_avatar_ready, set_image_ready,
};
use super::dto::{ThumbUpdate, enrich_to_update, message_to_dto, room_to_dto, space_to_dto};
use super::multiplex::spawn_event_multiplexer;
use super::{emoji, router};
use crate::commands::{UiCommand, UiEvent, ViewportChanged};
use crate::domain::models::{
    ConnectionStatus, EnrichmentDelta, LoginCredentials, Room, RoomId, Space, TimelineMessage,
    VerificationEmoji as DomainVerificationEmoji,
};
use crate::error::Result;
use crate::ports::media::MediaCache;

#[allow(clippy::all, clippy::pedantic, clippy::restriction, clippy::nursery)]
mod generated {
    slint::include_modules!();
}
use generated::{
    AppWindow, EmojiEntry, EmojiGroup, EmojiInsert, EmojiStore, MessageEntry, RoomEntry,
    SpaceEntry, VerificationEmoji,
};

thread_local! {
    static TIMELINE_MODEL: RefCell<Option<Rc<VecModel<MessageEntry>>>> = const { RefCell::new(None) };
    static ROOMS_MODEL: RefCell<Option<Rc<VecModel<RoomEntry>>>> = const { RefCell::new(None) };
    static SPACES_MODEL: RefCell<Option<Rc<VecModel<SpaceEntry>>>> = const { RefCell::new(None) };
    static SUBSPACES_MODEL: RefCell<Option<Rc<VecModel<SpaceEntry>>>> = const { RefCell::new(None) };
}

impl UiProps for AppWindow {
    fn set_string(&self, prop: StringProp, value: SharedString) {
        match prop {
            StringProp::LoginStep => self.set_login_step(value),
            StringProp::LoginStatus => self.set_login_status(value),
            StringProp::LoginError => self.set_login_error(value),
            StringProp::LoginMethod => self.set_login_method(value),
            StringProp::ResolvedHomeserver => self.set_resolved_homeserver(value),
            StringProp::UserId => self.set_user_id(value),
            StringProp::UserInitial => self.set_user_initial(value),
            StringProp::ToastMessage => self.set_toast_message(value),
            StringProp::ConnectionStatus => self.set_connection_status(value),
            StringProp::VerificationStep => self.set_verification_step(value),
            StringProp::VerificationSender => self.set_verification_sender(value),
            StringProp::VerificationError => self.set_verification_error(value),
            StringProp::SavedFilePath => self.set_saved_file_path(value),
            StringProp::SelectedRoomName => self.set_selected_room_name(value),
            StringProp::SelectedRoomId => self.set_selected_room_id(value),
            StringProp::SelectedSpaceId => self.set_selected_space_id(value),
            StringProp::SelectedSubspaceId => self.set_selected_subspace_id(value),
            StringProp::TimelineStatus => self.set_timeline_status(value),
            StringProp::InputUsername => self.set_input_username(value),
            StringProp::InputPassword => self.set_input_password(value),
        }
    }

    fn set_bool(&self, prop: BoolProp, value: bool) {
        match prop {
            BoolProp::VerificationVisible => self.set_verification_visible(value),
            BoolProp::VerificationIsSelf => self.set_verification_is_self(value),
            BoolProp::TimelineRetryable => self.set_timeline_retryable(value),
            BoolProp::BackwardsLoading => self.set_backwards_loading(value),
            BoolProp::ForwardsLoading => self.set_forwards_loading(value),
        }
    }

    fn set_int(&self, prop: IntProp, value: i32) {
        match prop {
            IntProp::NewMessagesCount => self.set_new_messages_count(value),
            IntProp::PrependToken => self.set_prepend_token(value),
            IntProp::SelectedRoomMembers => self.set_selected_room_members(value),
            IntProp::SelectedGeneration => self.set_selected_generation(value),
        }
    }

    fn get_string(&self, prop: StringProp) -> SharedString {
        match prop {
            StringProp::SelectedRoomId => self.get_selected_room_id(),
            other => {
                tracing::warn!("unexpected get for property: {}", other.as_str());
                SharedString::default()
            }
        }
    }

    fn apply_user_avatar(&self, avatar: Option<Image>) {
        match avatar {
            Some(img) => {
                self.set_user_avatar(img);
                self.set_user_has_avatar(true);
            }
            None => self.set_user_has_avatar(false),
        }
    }

    fn apply_emoji_model(&self, emojis: &[DomainVerificationEmoji]) {
        let entries: Vec<VerificationEmoji> = emojis
            .iter()
            .map(|e| VerificationEmoji {
                symbol: SharedString::from(&e.symbol),
                description: SharedString::from(&e.description),
            })
            .collect();
        self.set_verification_emojis(ModelRc::new(VecModel::from(entries)));
    }

    fn clear_emoji_model(&self) {
        self.set_verification_emojis(ModelRc::new(VecModel::<VerificationEmoji>::default()));
    }
}

pub struct SlintUiAdapter {
    window: AppWindow,
}

impl SlintUiAdapter {
    pub fn compile(_rt: &Runtime) -> Result<Self> {
        let window = AppWindow::new()?;
        Ok(Self { window })
    }

    #[allow(clippy::too_many_lines, clippy::unnecessary_wraps)]
    pub fn register_callbacks(
        &self,
        cmd_tx: &mpsc::UnboundedSender<UiCommand>,
        scroll_tx: &watch::Sender<ViewportChanged>,
    ) -> Result<()> {
        setup_emoji_store(&self.window);

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_check_server(move |homeserver| {
            let w = weak.upgrade();
            router::check_server(props(w.as_ref()), &tx, homeserver.to_string());
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_login_password(move |req| {
            let creds = LoginCredentials {
                homeserver: req.homeserver.to_string(),
                username: req.username.to_string(),
                password: req.password.to_string(),
            };
            let w = weak.upgrade();
            router::login_password(props(w.as_ref()), &tx, creds);
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_login_oauth(move || {
            let w = weak.upgrade();
            router::login_oauth(props(w.as_ref()), &tx);
        });

        let tx = cmd_tx.clone();
        self.window
            .on_cancel_oauth(move || router::cancel_oauth(&tx));

        let tx = cmd_tx.clone();
        self.window
            .on_select_room(move |room_id| router::select_room(&tx, room_id.to_string()));

        let tx = cmd_tx.clone();
        self.window
            .on_select_space(move |space_id| router::select_space(&tx, space_id.to_string()));

        let tx = cmd_tx.clone();
        self.window
            .on_select_subspace(move |space_id| router::select_subspace(&tx, space_id.to_string()));

        let tx = cmd_tx.clone();
        self.window.on_move_space(move |from, to| {
            let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                return;
            };
            router::move_space(&tx, from, to, |from, to| {
                SPACES_MODEL.with(|cell| {
                    if let Some(model) = cell.borrow().as_ref() {
                        reorder_rows(model, from, to);
                    }
                });
            });
        });

        let tx = cmd_tx.clone();
        self.window.on_logout(move || router::logout(&tx));

        let tx = cmd_tx.clone();
        self.window.on_send_message(move |req| {
            router::send_message(
                &tx,
                req.room_id.to_string(),
                req.body.to_string(),
                req.reply_to.to_string(),
            );
        });

        let tx = cmd_tx.clone();
        self.window
            .on_accept_verification(move || router::accept_verification(&tx));

        let tx = cmd_tx.clone();
        self.window
            .on_confirm_verification(move || router::confirm_verification(&tx));

        let tx = cmd_tx.clone();
        self.window
            .on_reject_verification(move || router::reject_verification(&tx));

        let tx = cmd_tx.clone();
        self.window
            .on_open_media(move |event_id| router::open_media(&tx, event_id.to_string()));

        self.window
            .on_request_media(move |unique_id| request_media(&unique_id));

        let tx = cmd_tx.clone();
        self.window.on_save_file(move |req| {
            router::save_file(&tx, req.event_id.to_string(), req.filename.to_string());
        });

        let scroll_tx = scroll_tx.clone();
        let weak = self.window.as_weak();
        self.window
            .on_scroll_position_changed(move |at_top, at_bottom| {
                router::scroll_position(&scroll_tx, selected_room_key(&weak), at_top, at_bottom);
            });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_paginate_backwards(move || {
            router::paginate_backwards(&tx, selected_room_key(&weak));
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window
            .on_paginate_forwards(move || router::paginate_forwards(&tx, selected_room_key(&weak)));

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window
            .on_jump_to_latest(move || router::jump_to_latest(&tx, selected_room_key(&weak)));

        let tx = cmd_tx.clone();
        self.window
            .on_retry_timeline(move || router::retry_timeline(&tx));

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_event_handler(
        &self,
        ui_rx: mpsc::Receiver<UiEvent>,
        rooms_rx: watch::Receiver<Arc<[Room]>>,
        spaces_rx: watch::Receiver<Arc<[Space]>>,
        subspaces_rx: watch::Receiver<Arc<[Space]>>,
        connection_rx: watch::Receiver<ConnectionStatus>,
        status_rx: watch::Receiver<String>,
        media_cache: Arc<dyn MediaCache>,
    ) {
        let weak = self.window.as_weak();
        let timeline_model: Rc<VecModel<MessageEntry>> = Rc::new(VecModel::default());
        let rooms_model: Rc<VecModel<RoomEntry>> = Rc::new(VecModel::default());
        let spaces_model: Rc<VecModel<SpaceEntry>> = Rc::new(VecModel::default());
        let subspaces_model: Rc<VecModel<SpaceEntry>> = Rc::new(VecModel::default());

        self.window
            .set_timeline(ModelRc::from(Rc::clone(&timeline_model)));
        self.window
            .set_rooms(ModelRc::from(Rc::clone(&rooms_model)));
        self.window
            .set_spaces(ModelRc::from(Rc::clone(&spaces_model)));
        self.window
            .set_subspaces(ModelRc::from(Rc::clone(&subspaces_model)));

        TIMELINE_MODEL.with(|cell| *cell.borrow_mut() = Some(timeline_model));
        ROOMS_MODEL.with(|cell| *cell.borrow_mut() = Some(rooms_model));

        set_animation_tick(|| {
            if let Some(timeline) = TIMELINE_MODEL.with(|cell| cell.borrow().clone()) {
                advance_animations(
                    &timeline,
                    &|e: &MessageEntry| e.unique_id.to_string(),
                    &|e: &mut MessageEntry, frame| e.thumbnail = frame,
                );
            }
        });

        register_image_callbacks(&self.window.as_weak());

        SPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(spaces_model));
        SUBSPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(subspaces_model));

        spawn_event_multiplexer(
            ui_rx,
            rooms_rx,
            spaces_rx,
            subspaces_rx,
            connection_rx,
            status_rx,
            media_cache,
            move |event, media, permit| post_ui_event(&weak, media, event, permit),
        );
    }

    pub fn run(&self) -> Result<()> {
        self.window.run()?;
        Ok(())
    }

    #[cfg(feature = "demo")]
    pub fn set_window_size(&self, width: f32, height: f32) {
        self.window
            .window()
            .set_size(slint::LogicalSize::new(width, height));
    }
}

fn props(window: Option<&AppWindow>) -> Option<&dyn UiProps> {
    window.map(|w| w as &dyn UiProps)
}

fn selected_room_key(weak: &slint::Weak<AppWindow>) -> Option<(RoomId, i32)> {
    let w = weak.upgrade()?;
    let room_id = w.get_selected_room_id().to_string();
    if room_id.is_empty() {
        return None;
    }
    Some((RoomId::new(room_id), w.get_selected_generation()))
}

fn emoji_entry_to_ui(e: &emoji::EmojiEntry) -> EmojiEntry {
    let tones: Vec<SharedString> = e
        .tones
        .iter()
        .map(|t| SharedString::from(t.as_str()))
        .collect();
    EmojiEntry {
        base: SharedString::from(&e.base),
        tones: ModelRc::new(VecModel::from(tones)),
        name: SharedString::from(&e.name),
    }
}

fn setup_emoji_store(window: &AppWindow) {
    let store = window.global::<EmojiStore>();
    let groups: Vec<EmojiGroup> = emoji::groups()
        .iter()
        .map(|items| {
            let entries: Vec<EmojiEntry> = items.iter().map(emoji_entry_to_ui).collect();
            EmojiGroup {
                items: ModelRc::new(VecModel::from(entries)),
            }
        })
        .collect();
    store.set_groups(ModelRc::new(VecModel::from(groups)));

    let weak = window.as_weak();
    store.on_search(move |query| {
        let Some(w) = weak.upgrade() else {
            return;
        };
        let results: Vec<EmojiEntry> = emoji::search(&query)
            .iter()
            .map(emoji_entry_to_ui)
            .collect();
        w.global::<EmojiStore>()
            .set_results(ModelRc::new(VecModel::from(results)));
    });

    store.on_insert(|text, offset, glyph| {
        let (inserted, caret) = emoji::insert_at(text.as_str(), offset, glyph.as_str());
        EmojiInsert {
            text: SharedString::from(inserted),
            caret,
        }
    });
}

fn post_ui_event(
    weak: &slint::Weak<AppWindow>,
    media_cache: Arc<dyn MediaCache>,
    event: UiEvent,
    permit: OwnedSemaphorePermit,
) {
    weak.upgrade_in_event_loop(move |w| {
        let timeline = TIMELINE_MODEL.with(|cell| cell.borrow().clone());
        let rooms = ROOMS_MODEL.with(|cell| cell.borrow().clone());
        let spaces = SPACES_MODEL.with(|cell| cell.borrow().clone());
        let subspaces = SUBSPACES_MODEL.with(|cell| cell.borrow().clone());
        if let (Some(tl), Some(rm), Some(sm), Some(ssm)) = (timeline, rooms, spaces, subspaces) {
            dispatch_ui_event(
                &w,
                event,
                &tl,
                &rm,
                &sm,
                &ssm,
                &|m| message_to_entry(m, media_cache.as_ref()),
                &|e, d| enrich_entry(e, d, media_cache.as_ref()),
                &|r| room_to_entry(r, media_cache.as_ref()),
                &|s| space_to_entry(s, media_cache.as_ref()),
                &|e| e.id.as_str(),
                &|e: &SpaceEntry| e.id.as_str(),
                &|e: &MessageEntry| e.unique_id.to_string(),
            );
        }
        drop(permit);
    })
    .ok();
}

fn string_model(items: Vec<SharedString>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items))
}

fn message_to_entry(m: &TimelineMessage, media: &dyn MediaCache) -> MessageEntry {
    let d = message_to_dto(m, media);
    MessageEntry {
        unique_id: d.unique_id,
        sender: d.sender,
        pronouns: string_model(d.pronouns),
        body: d.body,
        timestamp: d.timestamp,
        message_type: d.message_type,
        preview_kind: d.preview_kind,
        unsupported_kind: d.unsupported_kind,
        has_thumbnail: d.has_thumbnail,
        thumbnail: d.thumbnail.unwrap_or_default(),
        media_failed: d.media_failed,
        image_width: d.image_width,
        image_height: d.image_height,
        event_id: d.event_id,
        has_avatar: d.has_avatar,
        avatar: d.avatar.unwrap_or_default(),
        sender_initial: d.sender_initial,
        color_index: d.color_index,
        is_own: d.is_own,
        edited: d.edited,
        has_reply: d.has_reply,
        reply_sender: d.reply_sender,
        reply_kind: d.reply_kind,
        reply_body: d.reply_body,
        service_kind: d.service_kind,
        service_target: d.service_target,
    }
}

fn register_image_callbacks(weak: &slint::Weak<AppWindow>) {
    set_image_ready({
        let weak = weak.clone();
        move |unique_id, image| {
            apply_thumbnail_ready(unique_id, image);
            if let Some(window) = weak.upgrade() {
                window.window().request_redraw();
            }
        }
    });

    set_avatar_ready({
        let weak = weak.clone();
        move |slots, image| {
            apply_avatar_ready(&weak, slots, image);
            if let Some(window) = weak.upgrade() {
                window.window().request_redraw();
            }
        }
    });
}

fn apply_avatar_ready(weak: &slint::Weak<AppWindow>, slots: &[AvatarSlot], image: Option<&Image>) {
    let Some(image) = image else {
        return;
    };
    for slot in slots {
        match slot {
            AvatarSlot::Message(unique_id) => {
                if let Some(timeline) = TIMELINE_MODEL.with(|cell| cell.borrow().clone()) {
                    patch_rows(
                        &timeline,
                        |e: &MessageEntry| e.unique_id == unique_id.as_str(),
                        |e: &mut MessageEntry| {
                            e.avatar = image.clone();
                            e.has_avatar = true;
                        },
                    );
                }
            }
            AvatarSlot::Room(id) => {
                if let Some(rooms) = ROOMS_MODEL.with(|cell| cell.borrow().clone()) {
                    patch_rows(
                        &rooms,
                        |e: &RoomEntry| e.id == id.as_str(),
                        |e: &mut RoomEntry| {
                            e.avatar = image.clone();
                            e.has_avatar = true;
                        },
                    );
                }
            }
            AvatarSlot::Space(id) => {
                for cell in [&SPACES_MODEL, &SUBSPACES_MODEL] {
                    if let Some(spaces) = cell.with(|cell| cell.borrow().clone()) {
                        patch_rows(
                            &spaces,
                            |e: &SpaceEntry| e.id == id.as_str(),
                            |e: &mut SpaceEntry| {
                                e.avatar = image.clone();
                                e.has_avatar = true;
                            },
                        );
                    }
                }
            }
            AvatarSlot::User => {
                if let Some(window) = weak.upgrade() {
                    window.set_user_avatar(image.clone());
                    window.set_user_has_avatar(true);
                }
            }
        }
    }
}

fn apply_thumbnail_ready(unique_id: &str, image: Option<&Image>) {
    let Some(timeline) = TIMELINE_MODEL.with(|cell| cell.borrow().clone()) else {
        return;
    };
    patch_rows(
        &timeline,
        |e: &MessageEntry| e.unique_id == unique_id,
        |e: &mut MessageEntry| match image {
            Some(img) => {
                e.thumbnail = img.clone();
                e.has_thumbnail = true;
                e.media_failed = false;
            }
            None => e.media_failed = true,
        },
    );
}

fn enrich_entry(entry: &mut MessageEntry, delta: &EnrichmentDelta, media: &dyn MediaCache) {
    let update = enrich_to_update(delta, media);
    match update.thumbnail {
        ThumbUpdate::Ready(img) => {
            entry.thumbnail = img;
            entry.has_thumbnail = true;
            entry.media_failed = false;
        }
        ThumbUpdate::Failed => entry.media_failed = true,
        ThumbUpdate::Unchanged => {}
    }
    if let Some(img) = update.avatar {
        entry.avatar = img;
        entry.has_avatar = true;
    }
    if let Some(pronouns) = update.pronouns {
        entry.pronouns = string_model(pronouns);
    }
}

fn room_to_entry(r: &Room, media: &dyn MediaCache) -> RoomEntry {
    let d = room_to_dto(r, media);
    RoomEntry {
        id: d.id,
        name: d.name,
        initial: d.initial,
        avatar: d.avatar.unwrap_or_default(),
        has_avatar: d.has_avatar,
        color_index: d.color_index,
        members: d.members,
        unread: d.unread,
        mentions: d.mentions,
        last_message_sender: d.last_message_sender,
        last_message_kind: d.last_message_kind,
        last_message_body: d.last_message_body,
        last_message_service_kind: d.last_message_service_kind,
        last_message_service_target: d.last_message_service_target,
        last_message_is_own: d.last_message_is_own,
        last_message_edited: d.last_message_edited,
        last_message_time: d.last_message_time,
    }
}

fn space_to_entry(s: &Space, media: &dyn MediaCache) -> SpaceEntry {
    let d = space_to_dto(s, media);
    SpaceEntry {
        id: d.id,
        name: d.name,
        unread: d.unread,
        mentions: d.mentions,
        initial: d.initial,
        avatar: d.avatar.unwrap_or_default(),
        has_avatar: d.has_avatar,
    }
}
