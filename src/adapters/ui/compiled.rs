use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use tokio::runtime::Runtime;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};

use super::common::{
    BoolProp, IntProp, SLINT_INFLIGHT, Status, StringProp, UiProps, avatar_color_index,
    avatar_initials, dispatch_ui_event, message_body_text, message_preview_kind_token,
    message_sender_label, message_timestamp_label, message_type_token, pronoun_labels,
    room_activity_label, send_command, sender_initial, service_kind_token, service_target,
    unsupported_kind,
};
use super::decode::{
    AvatarSlot, advance_animations, load_avatar_async, load_thumbnail, patch_rows,
    set_animation_tick, set_avatar_ready, set_image_ready,
};
use super::emoji;
use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{
    ConnectionStatus, EnrichmentDelta, LoginCredentials, MessageBody, Room, RoomId, Space,
    ThumbnailOutcome, TimelineMessage, VerificationEmoji as DomainVerificationEmoji,
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
        scroll_tx: &watch::Sender<(bool, bool)>,
    ) -> Result<()> {
        setup_emoji_store(&self.window);

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_check_server(move |homeserver| {
            if let Some(w) = weak.upgrade() {
                w.set_login_status(SharedString::from(Status::CheckingServer.as_str()));
                w.set_login_error(SharedString::default());
            }
            send_command(&tx, UiCommand::CheckServer(homeserver.to_string()));
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_login_password(move |req| {
            let creds = LoginCredentials {
                homeserver: req.homeserver.to_string(),
                username: req.username.to_string(),
                password: req.password.to_string(),
            };
            if let Some(w) = weak.upgrade() {
                w.set_login_status(SharedString::from(Status::LoggingIn.as_str()));
                w.set_login_error(SharedString::default());
            }
            send_command(&tx, UiCommand::LoginPassword(creds));
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_login_oauth(move || {
            if let Some(w) = weak.upgrade() {
                w.set_login_status(SharedString::from(Status::OpeningBrowser.as_str()));
                w.set_login_error(SharedString::default());
            }
            send_command(&tx, UiCommand::LoginOAuth);
        });

        let tx = cmd_tx.clone();
        self.window.on_select_room(move |room_id| {
            send_command(&tx, UiCommand::SelectRoom(RoomId::new(room_id.to_string())));
        });

        let tx = cmd_tx.clone();
        self.window.on_select_space(move |space_id| {
            let space_id = space_id.to_string();
            let selected = if space_id.is_empty() {
                None
            } else {
                Some(RoomId::new(space_id))
            };
            send_command(&tx, UiCommand::SelectSpace(selected));
        });

        let tx = cmd_tx.clone();
        self.window.on_select_subspace(move |space_id| {
            let space_id = space_id.to_string();
            let selected = if space_id.is_empty() {
                None
            } else {
                Some(RoomId::new(space_id))
            };
            send_command(&tx, UiCommand::SelectSubspace(selected));
        });

        let tx = cmd_tx.clone();
        self.window.on_move_space(move |from, to| {
            let Ok(from) = usize::try_from(from) else {
                return;
            };
            let Ok(to) = usize::try_from(to) else {
                return;
            };
            if from == to {
                return;
            }
            SPACES_MODEL.with(|cell| {
                if let Some(model) = cell.borrow().as_ref()
                    && from < model.row_count()
                    && to < model.row_count()
                {
                    let entry = model.remove(from);
                    model.insert(to, entry);
                }
            });
            send_command(&tx, UiCommand::MoveSpace { from, to });
        });

        let tx = cmd_tx.clone();
        self.window.on_logout(move || {
            send_command(&tx, UiCommand::Logout);
        });

        let tx = cmd_tx.clone();
        self.window.on_send_message(move |req| {
            let room_id = req.room_id.to_string();
            let body = req.body.to_string();
            let reply_to = req.reply_to.to_string();
            if !room_id.is_empty() && !body.is_empty() {
                send_command(
                    &tx,
                    UiCommand::SendMessage {
                        room_id: RoomId::new(room_id),
                        body,
                        reply_to: (!reply_to.is_empty()).then_some(reply_to),
                    },
                );
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_accept_verification(move || {
            send_command(&tx, UiCommand::AcceptVerification);
        });

        let tx = cmd_tx.clone();
        self.window.on_confirm_verification(move || {
            send_command(&tx, UiCommand::ConfirmVerification);
        });

        let tx = cmd_tx.clone();
        self.window.on_reject_verification(move || {
            send_command(&tx, UiCommand::RejectVerification);
        });

        let tx = cmd_tx.clone();
        self.window.on_open_media(move |event_id| {
            let event_id = event_id.to_string();
            if !event_id.is_empty() {
                send_command(&tx, UiCommand::OpenMedia { event_id });
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_save_file(move |req| {
            let event_id = req.event_id.to_string();
            let filename = req.filename.to_string();
            if !event_id.is_empty() {
                send_command(&tx, UiCommand::SaveFile { event_id, filename });
            }
        });

        let scroll_tx = scroll_tx.clone();
        self.window
            .on_scroll_position_changed(move |at_top, at_bottom| {
                if scroll_tx.send((at_top, at_bottom)).is_err() {
                    tracing::debug!("scroll position receiver closed");
                }
            });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_paginate_backwards(move || {
            let room_id = weak
                .upgrade()
                .map(|w| w.get_selected_room_id().to_string())
                .unwrap_or_default();
            if !room_id.is_empty() {
                send_command(
                    &tx,
                    UiCommand::PaginateBackwards {
                        room_id: RoomId::new(room_id),
                    },
                );
            }
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_paginate_forwards(move || {
            let room_id = weak
                .upgrade()
                .map(|w| w.get_selected_room_id().to_string())
                .unwrap_or_default();
            if !room_id.is_empty() {
                send_command(
                    &tx,
                    UiCommand::PaginateForwards {
                        room_id: RoomId::new(room_id),
                    },
                );
            }
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_jump_to_latest(move || {
            let room_id = weak
                .upgrade()
                .map(|w| w.get_selected_room_id().to_string())
                .unwrap_or_default();
            if !room_id.is_empty() {
                send_command(
                    &tx,
                    UiCommand::JumpToLatest {
                        room_id: RoomId::new(room_id),
                    },
                );
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_retry_timeline(move || {
            send_command(&tx, UiCommand::RetryTimeline);
        });

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_event_handler(
        &self,
        mut ui_rx: mpsc::Receiver<UiEvent>,
        mut rooms_rx: watch::Receiver<Arc<[Room]>>,
        mut spaces_rx: watch::Receiver<Arc<[Space]>>,
        mut subspaces_rx: watch::Receiver<Arc<[Space]>>,
        mut connection_rx: watch::Receiver<ConnectionStatus>,
        mut status_rx: watch::Receiver<String>,
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

        tokio::spawn(async move {
            let sem = Arc::new(Semaphore::new(SLINT_INFLIGHT));
            let mut rooms_done = false;
            let mut spaces_done = false;
            let mut subspaces_done = false;
            let mut connection_done = false;
            let mut status_done = false;
            loop {
                let Ok(permit) = Arc::clone(&sem).acquire_owned().await else {
                    break;
                };
                tokio::select! {
                    maybe = ui_rx.recv() => {
                        let Some(event) = maybe else { break };
                        post_ui_event(&weak, Arc::clone(&media_cache), event, permit);
                    }
                    changed = rooms_rx.changed(), if !rooms_done => {
                        if changed.is_err() {
                            rooms_done = true;
                        } else {
                            let rooms = rooms_rx.borrow_and_update().clone();
                            post_ui_event(&weak, Arc::clone(&media_cache), UiEvent::Rooms(rooms), permit);
                        }
                    }
                    changed = spaces_rx.changed(), if !spaces_done => {
                        if changed.is_err() {
                            spaces_done = true;
                        } else {
                            let spaces = spaces_rx.borrow_and_update().clone();
                            post_ui_event(&weak, Arc::clone(&media_cache), UiEvent::Spaces(spaces), permit);
                        }
                    }
                    changed = subspaces_rx.changed(), if !subspaces_done => {
                        if changed.is_err() {
                            subspaces_done = true;
                        } else {
                            let subspaces = subspaces_rx.borrow_and_update().clone();
                            post_ui_event(&weak, Arc::clone(&media_cache), UiEvent::Subspaces(subspaces), permit);
                        }
                    }
                    changed = connection_rx.changed(), if !connection_done => {
                        if changed.is_err() {
                            connection_done = true;
                        } else {
                            let status = connection_rx.borrow_and_update().clone();
                            post_ui_event(&weak, Arc::clone(&media_cache), UiEvent::ConnectionStatus(status), permit);
                        }
                    }
                    changed = status_rx.changed(), if !status_done => {
                        if changed.is_err() {
                            status_done = true;
                        } else {
                            let message = status_rx.borrow_and_update().clone();
                            post_ui_event(&weak, Arc::clone(&media_cache), UiEvent::Status(message), permit);
                        }
                    }
                }
            }
        });
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

fn message_to_entry(m: &TimelineMessage, media: &dyn MediaCache) -> MessageEntry {
    let pronouns: Vec<SharedString> = pronoun_labels(&m.sender_pronouns)
        .into_iter()
        .map(SharedString::from)
        .collect();
    let mut entry = MessageEntry {
        unique_id: SharedString::from(&m.unique_id),
        sender: SharedString::from(message_sender_label(m)),
        pronouns: ModelRc::new(VecModel::from(pronouns)),
        body: SharedString::from(message_body_text(&m.body)),
        timestamp: SharedString::from(&message_timestamp_label(m.timestamp)),
        message_type: SharedString::from(message_type_token(&m.body)),
        preview_kind: SharedString::from(message_preview_kind_token(m.body.preview_kind())),
        unsupported_kind: SharedString::from(unsupported_kind(&m.body)),
        event_id: SharedString::from(m.event_id.as_ref().map_or("", |e| e.0.as_str())),
        sender_initial: SharedString::from(avatar_initials(message_sender_label(m))),
        color_index: avatar_color_index(&m.sender),
        is_own: m.is_own,
        edited: m.edited,
        has_reply: m.reply.is_some(),
        reply_sender: SharedString::from(m.reply.as_ref().map_or("", |r| r.sender.as_str())),
        reply_kind: SharedString::from(
            m.reply
                .as_ref()
                .map_or("", |r| message_preview_kind_token(r.kind)),
        ),
        reply_body: SharedString::from(m.reply.as_ref().map_or("", |r| r.body.as_str())),
        service_kind: SharedString::from(m.body.service().map_or("", service_kind_token)),
        service_target: SharedString::from(m.body.service().map_or("", service_target)),
        ..Default::default()
    };

    if let MessageBody::Image { meta, .. } = &m.body {
        entry.image_width = meta.width.unwrap_or(0).cast_signed();
        entry.image_height = meta.height.unwrap_or(0).cast_signed();
        if let Some(event_id) = m.event_id.as_ref() {
            if let Some(thumb_path) = media.thumbnail_path(&event_id.0) {
                if let Some(img) = load_thumbnail(&thumb_path, &m.unique_id) {
                    entry.thumbnail = img;
                    entry.has_thumbnail = true;
                }
            } else {
                entry.media_failed = media.thumbnail_failed(&event_id.0);
            }
        }
    }

    if let Some(avatar_path) = media.avatar_path(&m.sender)
        && let Some(img) = load_avatar_async(&avatar_path, AvatarSlot::Message(m.unique_id.clone()))
    {
        entry.avatar = img;
        entry.has_avatar = true;
    }

    entry
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
    match delta.thumbnail {
        ThumbnailOutcome::Ready => {
            if let Some(event_id) = delta.event_id.as_ref()
                && let Some(thumb_path) = media.thumbnail_path(&event_id.0)
                && let Some(img) = load_thumbnail(&thumb_path, &delta.unique_id)
            {
                entry.thumbnail = img;
                entry.has_thumbnail = true;
                entry.media_failed = false;
            }
        }
        ThumbnailOutcome::Failed => entry.media_failed = true,
        ThumbnailOutcome::Unchanged => {}
    }

    if delta.avatar_ready
        && let Some(avatar_path) = media.avatar_path(&delta.sender)
        && let Some(img) =
            load_avatar_async(&avatar_path, AvatarSlot::Message(delta.unique_id.clone()))
    {
        entry.avatar = img;
        entry.has_avatar = true;
    }

    if let Some(pronouns) = &delta.pronouns {
        let labels: Vec<SharedString> = pronoun_labels(pronouns)
            .into_iter()
            .map(SharedString::from)
            .collect();
        entry.pronouns = ModelRc::new(VecModel::from(labels));
    }
}

fn room_to_entry(r: &Room, media: &dyn MediaCache) -> RoomEntry {
    let mut entry = RoomEntry {
        id: SharedString::from(r.id.as_ref()),
        name: SharedString::from(&r.display_name),
        initial: SharedString::from(avatar_initials(&r.display_name)),
        color_index: avatar_color_index(r.id.as_ref()),
        #[allow(clippy::cast_possible_truncation)]
        members: if r.is_direct {
            0
        } else {
            r.member_count as i32
        },
        #[allow(clippy::cast_possible_truncation)]
        unread: r.unread_count as i32,
        #[allow(clippy::cast_possible_truncation)]
        mentions: r.mention_count as i32,
        last_message_sender: SharedString::from(
            r.last_message_sender.as_deref().unwrap_or_default(),
        ),
        last_message_kind: SharedString::from(message_preview_kind_token(r.last_message_kind)),
        last_message_body: SharedString::from(&r.last_message_body),
        last_message_service_kind: SharedString::from(
            r.last_message_service
                .as_ref()
                .map_or("", service_kind_token),
        ),
        last_message_service_target: SharedString::from(
            r.last_message_service.as_ref().map_or("", service_target),
        ),
        last_message_is_own: r.last_message_is_own,
        last_message_edited: r.last_message_edited,
        last_message_time: SharedString::from(&room_activity_label(r.last_activity_ts)),
        ..Default::default()
    };

    if let Some(mxc) = &r.avatar_mxc
        && let Some(avatar_path) = media.room_avatar_path(mxc)
        && let Some(img) =
            load_avatar_async(&avatar_path, AvatarSlot::Room(r.id.as_ref().to_owned()))
    {
        entry.avatar = img;
        entry.has_avatar = true;
    }

    entry
}

fn space_to_entry(s: &Space, media: &dyn MediaCache) -> SpaceEntry {
    let mut entry = SpaceEntry {
        id: SharedString::from(&s.id),
        name: SharedString::from(&s.name),
        #[allow(clippy::cast_possible_truncation)]
        unread: s.unread as i32,
        #[allow(clippy::cast_possible_truncation)]
        mentions: s.mentions as i32,
        initial: SharedString::from(sender_initial(&s.name)),
        ..Default::default()
    };

    if let Some(mxc) = &s.avatar_mxc
        && let Some(avatar_path) = media.space_avatar_path(mxc)
        && let Some(img) = load_avatar_async(&avatar_path, AvatarSlot::Space(s.id.clone()))
    {
        entry.avatar = img;
        entry.has_avatar = true;
    }

    entry
}
