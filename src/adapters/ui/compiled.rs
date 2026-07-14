use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use super::common::{
    BoolProp, IntProp, Status, StringProp, UiProps, dispatch_ui_event, last_message_kind_token,
    load_image_cached, message_body_text, message_sender_label, message_timestamp_label,
    message_type_token, room_activity_label, sender_initial,
};
use super::emoji;
use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{
    LoginCredentials, MessageBody, Room, RoomId, Space, TimelineMessage,
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
            StringProp::InputUsername => self.set_input_username(value),
            StringProp::InputPassword => self.set_input_password(value),
        }
    }

    fn set_bool(&self, prop: BoolProp, value: bool) {
        match prop {
            BoolProp::VerificationVisible => self.set_verification_visible(value),
            BoolProp::VerificationIsSelf => self.set_verification_is_self(value),
            BoolProp::TimelineLoading => self.set_timeline_loading(value),
            BoolProp::BackwardsLoading => self.set_backwards_loading(value),
            BoolProp::ForwardsLoading => self.set_forwards_loading(value),
        }
    }

    fn set_int(&self, prop: IntProp, value: i32) {
        match prop {
            IntProp::NewMessagesCount => self.set_new_messages_count(value),
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
    pub fn register_callbacks(&self, cmd_tx: &mpsc::UnboundedSender<UiCommand>) -> Result<()> {
        setup_emoji_store(&self.window);

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_check_server(move |homeserver| {
            if let Some(w) = weak.upgrade() {
                w.set_login_status(SharedString::from(Status::CheckingServer.as_str()));
                w.set_login_error(SharedString::default());
            }
            if let Err(e) = tx.send(UiCommand::CheckServer(homeserver.to_string())) {
                tracing::debug!("failed to send CheckServer command: {e}");
            }
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
            if let Err(e) = tx.send(UiCommand::LoginPassword(creds)) {
                tracing::debug!("failed to send LoginPassword command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_login_oauth(move || {
            if let Some(w) = weak.upgrade() {
                w.set_login_status(SharedString::from(Status::OpeningBrowser.as_str()));
                w.set_login_error(SharedString::default());
            }
            if let Err(e) = tx.send(UiCommand::LoginOAuth) {
                tracing::debug!("failed to send LoginOAuth command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_select_room(move |room_id| {
            if let Some(w) = weak.upgrade() {
                w.set_timeline_loading(true);
            }
            if let Err(e) = tx.send(UiCommand::SelectRoom(RoomId::new(room_id.to_string()))) {
                tracing::debug!("failed to send SelectRoom command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_select_space(move |space_id| {
            let space_id = space_id.to_string();
            let selected = if space_id.is_empty() {
                None
            } else {
                Some(RoomId::new(space_id))
            };
            if let Err(e) = tx.send(UiCommand::SelectSpace(selected)) {
                tracing::debug!("failed to send SelectSpace command: {e}");
            }
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
            if let Err(e) = tx.send(UiCommand::MoveSpace { from, to }) {
                tracing::debug!("failed to send MoveSpace command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_logout(move || {
            if let Err(e) = tx.send(UiCommand::Logout) {
                tracing::debug!("failed to send Logout command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_send_message(move |req| {
            let room_id = req.room_id.to_string();
            let body = req.body.to_string();
            let reply_to = req.reply_to.to_string();
            if !room_id.is_empty()
                && !body.is_empty()
                && let Err(e) = tx.send(UiCommand::SendMessage {
                    room_id: RoomId::new(room_id),
                    body,
                    reply_to: (!reply_to.is_empty()).then_some(reply_to),
                })
            {
                tracing::debug!("failed to send SendMessage command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_accept_verification(move || {
            if let Err(e) = tx.send(UiCommand::AcceptVerification) {
                tracing::debug!("failed to send AcceptVerification command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_confirm_verification(move || {
            if let Err(e) = tx.send(UiCommand::ConfirmVerification) {
                tracing::debug!("failed to send ConfirmVerification command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_reject_verification(move || {
            if let Err(e) = tx.send(UiCommand::RejectVerification) {
                tracing::debug!("failed to send RejectVerification command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_open_media(move |event_id| {
            let event_id = event_id.to_string();
            if !event_id.is_empty()
                && let Err(e) = tx.send(UiCommand::OpenMedia { event_id })
            {
                tracing::debug!("failed to send OpenMedia command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        self.window.on_save_file(move |req| {
            let event_id = req.event_id.to_string();
            let filename = req.filename.to_string();
            if !event_id.is_empty()
                && let Err(e) = tx.send(UiCommand::SaveFile { event_id, filename })
            {
                tracing::debug!("failed to send SaveFile command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        self.window
            .on_scroll_position_changed(move |at_top, at_bottom| {
                if let Err(e) = tx.send(UiCommand::ScrollPositionChanged { at_top, at_bottom }) {
                    tracing::debug!("failed to send ScrollPositionChanged command: {e}");
                }
            });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_paginate_backwards(move || {
            let room_id = weak
                .upgrade()
                .map(|w| w.get_selected_room_id().to_string())
                .unwrap_or_default();
            if !room_id.is_empty()
                && let Err(e) = tx.send(UiCommand::PaginateBackwards {
                    room_id: RoomId::new(room_id),
                })
            {
                tracing::debug!("failed to send PaginateBackwards command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_paginate_forwards(move || {
            let room_id = weak
                .upgrade()
                .map(|w| w.get_selected_room_id().to_string())
                .unwrap_or_default();
            if !room_id.is_empty()
                && let Err(e) = tx.send(UiCommand::PaginateForwards {
                    room_id: RoomId::new(room_id),
                })
            {
                tracing::debug!("failed to send PaginateForwards command: {e}");
            }
        });

        let tx = cmd_tx.clone();
        let weak = self.window.as_weak();
        self.window.on_jump_to_latest(move || {
            let room_id = weak
                .upgrade()
                .map(|w| w.get_selected_room_id().to_string())
                .unwrap_or_default();
            if !room_id.is_empty()
                && let Err(e) = tx.send(UiCommand::JumpToLatest {
                    room_id: RoomId::new(room_id),
                })
            {
                tracing::debug!("failed to send JumpToLatest command: {e}");
            }
        });

        Ok(())
    }

    pub fn spawn_event_handler(
        &self,
        mut ui_rx: mpsc::UnboundedReceiver<UiEvent>,
        media_cache: Arc<dyn MediaCache>,
    ) {
        let weak = self.window.as_weak();
        let timeline_model: Rc<VecModel<MessageEntry>> = Rc::new(VecModel::default());
        let rooms_model: Rc<VecModel<RoomEntry>> = Rc::new(VecModel::default());
        let spaces_model: Rc<VecModel<SpaceEntry>> = Rc::new(VecModel::default());

        self.window
            .set_timeline(ModelRc::from(Rc::clone(&timeline_model)));
        self.window
            .set_rooms(ModelRc::from(Rc::clone(&rooms_model)));
        self.window
            .set_spaces(ModelRc::from(Rc::clone(&spaces_model)));

        TIMELINE_MODEL.with(|cell| *cell.borrow_mut() = Some(timeline_model));
        ROOMS_MODEL.with(|cell| *cell.borrow_mut() = Some(rooms_model));
        SPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(spaces_model));

        tokio::spawn(async move {
            while let Some(event) = ui_rx.recv().await {
                let media_cache = Arc::clone(&media_cache);
                weak.upgrade_in_event_loop(move |w| {
                    let timeline = TIMELINE_MODEL.with(|cell| cell.borrow().clone());
                    let rooms = ROOMS_MODEL.with(|cell| cell.borrow().clone());
                    let spaces = SPACES_MODEL.with(|cell| cell.borrow().clone());
                    if let (Some(tl), Some(rm), Some(sm)) = (timeline, rooms, spaces) {
                        dispatch_ui_event(
                            &w,
                            event,
                            &tl,
                            &rm,
                            &sm,
                            &|m| message_to_entry(m, media_cache.as_ref()),
                            &room_to_entry,
                            &|s| space_to_entry(s, media_cache.as_ref()),
                            &|e| e.id.as_str(),
                            &|e: &SpaceEntry| e.id.as_str(),
                            &|e: &MessageEntry| e.event_id.to_string(),
                        );
                    }
                })
                .ok();
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

fn message_to_entry(m: &TimelineMessage, media: &dyn MediaCache) -> MessageEntry {
    let mut entry = MessageEntry {
        unique_id: SharedString::from(&m.unique_id),
        sender: SharedString::from(message_sender_label(m)),
        body: SharedString::from(&message_body_text(&m.body)),
        timestamp: SharedString::from(&message_timestamp_label(m.timestamp)),
        message_type: SharedString::from(message_type_token(&m.body)),
        event_id: SharedString::from(&m.event_id.0),
        sender_initial: SharedString::from(sender_initial(message_sender_label(m))),
        is_own: m.is_own,
        edited: m.edited,
        has_reply: m.reply.is_some(),
        reply_sender: SharedString::from(m.reply.as_ref().map_or("", |r| r.sender.as_str())),
        reply_body: SharedString::from(m.reply.as_ref().map_or("", |r| r.preview.as_str())),
        ..Default::default()
    };

    if let MessageBody::Image { meta, .. } = &m.body {
        entry.image_width = meta.width.unwrap_or(0).cast_signed();
        entry.image_height = meta.height.unwrap_or(0).cast_signed();
        if let Some(thumb_path) = media.thumbnail_path(&m.event_id.0)
            && let Some(img) = load_image_cached(&thumb_path)
        {
            entry.thumbnail = img;
            entry.has_thumbnail = true;
        }
    }

    if let Some(avatar_path) = media.avatar_path(&m.sender)
        && let Some(img) = load_image_cached(&avatar_path)
    {
        entry.avatar = img;
        entry.has_avatar = true;
    }

    entry
}

fn room_to_entry(r: &Room) -> RoomEntry {
    RoomEntry {
        id: SharedString::from(r.id.as_ref()),
        name: SharedString::from(&r.display_name),
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
        last_message_kind: SharedString::from(last_message_kind_token(r.last_message_kind)),
        last_message_body: SharedString::from(&r.last_message_body),
        last_message_is_own: r.last_message_is_own,
        last_message_edited: r.last_message_edited,
        last_message_time: SharedString::from(&room_activity_label(r.last_activity_ts)),
    }
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
        && let Some(img) = load_image_cached(&avatar_path)
    {
        entry.avatar = img;
        entry.has_avatar = true;
    }

    entry
}
