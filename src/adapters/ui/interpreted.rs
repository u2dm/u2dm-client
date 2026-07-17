use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{Model, ModelRc, VecModel};
use slint_interpreter::{
    Compiler, ComponentHandle, ComponentInstance, SharedString, Struct, Value,
};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

thread_local! {
    static TIMELINE_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
    static ROOMS_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
    static SPACES_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
    static SUBSPACES_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
}

use super::common::{
    BoolProp, IntProp, Status, StringProp, UiProps, avatar_color_index, avatar_initials,
    dispatch_ui_event, load_image_cached, message_body_text, message_preview_kind_token,
    message_sender_label, message_timestamp_label, message_type_token, pronoun_labels,
    room_activity_label, sender_initial, service_kind_token, service_target, unsupported_kind,
};
use super::emoji;
use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{
    LoginCredentials, MessageBody, Room, RoomId, Space, TimelineMessage,
    VerificationEmoji as DomainVerificationEmoji,
};
use crate::error::{AppError, Result};
use crate::ports::media::MediaCache;

fn set_prop(inst: &ComponentInstance, name: &str, value: Value) {
    if let Err(e) = inst.set_property(name, value) {
        tracing::warn!("failed to set property '{name}': {e:?}");
    }
}

fn set_global_prop(inst: &ComponentInstance, global: &str, name: &str, value: Value) -> Result<()> {
    inst.set_global_property(global, name, value)
        .map_err(|e| AppError::Ui(format!("{e:?}")))
}

fn string_arg(args: &[Value], index: usize) -> String {
    args.get(index)
        .and_then(|v| match v {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn bool_arg(args: &[Value], index: usize) -> bool {
    args.get(index)
        .and_then(|v| match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        })
        .unwrap_or_default()
}

impl UiProps for ComponentInstance {
    fn set_string(&self, prop: StringProp, value: SharedString) {
        set_prop(self, prop.as_str(), Value::String(value));
    }

    fn set_bool(&self, prop: BoolProp, value: bool) {
        set_prop(self, prop.as_str(), Value::Bool(value));
    }

    fn set_int(&self, prop: IntProp, value: i32) {
        set_prop(self, prop.as_str(), Value::Number(value.into()));
    }

    fn get_string(&self, prop: StringProp) -> SharedString {
        self.get_property(prop.as_str())
            .ok()
            .and_then(|v| match v {
                Value::String(s) => Some(s),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn apply_user_avatar(&self, avatar: Option<slint::Image>) {
        match avatar {
            Some(img) => {
                set_prop(self, "user-avatar", Value::Image(img));
                set_prop(self, "user-has-avatar", Value::Bool(true));
            }
            None => set_prop(self, "user-has-avatar", Value::Bool(false)),
        }
    }

    fn apply_emoji_model(&self, emojis: &[DomainVerificationEmoji]) {
        let entries: Vec<Value> = emojis
            .iter()
            .map(|e| {
                Value::Struct(Struct::from_iter([
                    (
                        "symbol".to_string(),
                        Value::String(SharedString::from(&e.symbol)),
                    ),
                    (
                        "description".to_string(),
                        Value::String(SharedString::from(&e.description)),
                    ),
                ]))
            })
            .collect();
        set_prop(
            self,
            "verification-emojis",
            Value::Model(ModelRc::new(VecModel::from(entries))),
        );
    }

    fn clear_emoji_model(&self) {
        set_prop(
            self,
            "verification-emojis",
            Value::Model(ModelRc::new(VecModel::<Value>::default())),
        );
    }
}

pub struct SlintUiAdapter {
    instance: ComponentInstance,
}

impl SlintUiAdapter {
    pub fn compile(rt: &Runtime) -> Result<Self> {
        let instance = rt.block_on(async {
            let mut compiler = Compiler::new();
            compiler.set_library_paths(HashMap::from([(
                "lucide".to_string(),
                PathBuf::from(lucide_slint::lib()),
            )]));
            let result = compiler.build_from_path("ui/main.slint").await;
            for diag in result.diagnostics() {
                tracing::error!("slint: {diag}");
            }
            let def = result
                .component("AppWindow")
                .ok_or_else(|| AppError::Ui("failed to load ui/main.slint".into()))?;
            let inst = def.create().map_err(|e| AppError::Ui(e.to_string()))?;
            Ok::<_, AppError>(inst)
        })?;
        Ok(Self { instance })
    }

    #[allow(clippy::too_many_lines)]
    pub fn register_callbacks(&self, cmd_tx: &mpsc::UnboundedSender<UiCommand>) -> Result<()> {
        setup_emoji_store(&self.instance)?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        self.instance
            .set_callback("check-server", move |args: &[Value]| -> Value {
                let homeserver = args
                    .first()
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .unwrap_or_default();

                if let Some(inst) = weak.upgrade() {
                    set_prop(
                        &inst,
                        "login-status",
                        Value::String(SharedString::from(Status::CheckingServer.as_str())),
                    );
                    set_prop(&inst, "login-error", Value::String(SharedString::default()));
                }

                if let Err(e) = tx.send(UiCommand::CheckServer(homeserver)) {
                    tracing::debug!("failed to send CheckServer command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        self.instance
            .set_callback("login-password", move |args: &[Value]| -> Value {
                let creds = match args.first() {
                    Some(Value::Struct(s)) => {
                        let field = |name: &str| -> String {
                            s.get_field(name)
                                .and_then(|v| match v {
                                    Value::String(s) => Some(s.to_string()),
                                    _ => None,
                                })
                                .unwrap_or_default()
                        };
                        LoginCredentials {
                            homeserver: field("homeserver"),
                            username: field("username"),
                            password: field("password"),
                        }
                    }
                    _ => return Value::Void,
                };

                if let Some(inst) = weak.upgrade() {
                    set_prop(
                        &inst,
                        "login-status",
                        Value::String(SharedString::from(Status::LoggingIn.as_str())),
                    );
                    set_prop(&inst, "login-error", Value::String(SharedString::default()));
                }

                if let Err(e) = tx.send(UiCommand::LoginPassword(creds)) {
                    tracing::debug!("failed to send LoginPassword command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        self.instance
            .set_callback("login-oauth", move |_args: &[Value]| -> Value {
                if let Some(inst) = weak.upgrade() {
                    set_prop(
                        &inst,
                        "login-status",
                        Value::String(SharedString::from(Status::OpeningBrowser.as_str())),
                    );
                    set_prop(&inst, "login-error", Value::String(SharedString::default()));
                }

                if let Err(e) = tx.send(UiCommand::LoginOAuth) {
                    tracing::debug!("failed to send LoginOAuth command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        self.instance
            .set_callback("select-room", move |args: &[Value]| -> Value {
                let room_id = args
                    .first()
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .unwrap_or_default();

                if let Some(inst) = weak.upgrade() {
                    set_prop(&inst, "timeline-loading", Value::Bool(true));
                }

                if let Err(e) = tx.send(UiCommand::SelectRoom(RoomId::new(room_id))) {
                    tracing::debug!("failed to send SelectRoom command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("select-space", move |args: &[Value]| -> Value {
                let space_id = args
                    .first()
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let selected = if space_id.is_empty() {
                    None
                } else {
                    Some(RoomId::new(space_id))
                };
                if let Err(e) = tx.send(UiCommand::SelectSpace(selected)) {
                    tracing::debug!("failed to send SelectSpace command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("select-subspace", move |args: &[Value]| -> Value {
                let space_id = string_arg(args, 0);
                let selected = if space_id.is_empty() {
                    None
                } else {
                    Some(RoomId::new(space_id))
                };
                if let Err(e) = tx.send(UiCommand::SelectSubspace(selected)) {
                    tracing::debug!("failed to send SelectSubspace command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("move-space", move |args: &[Value]| -> Value {
                let index = |i: usize| -> Option<usize> {
                    match args.get(i) {
                        Some(Value::Number(n))
                            if n.is_finite()
                                && n.fract() == 0.0
                                && *n >= 0.0
                                && *n <= u32::MAX.into() =>
                        {
                            n.to_string().parse().ok()
                        }
                        _ => None,
                    }
                };
                if let (Some(from), Some(to)) = (index(0), index(1))
                    && from != to
                {
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
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("logout", move |_args: &[Value]| -> Value {
                if let Err(e) = tx.send(UiCommand::Logout) {
                    tracing::debug!("failed to send Logout command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("send-message", move |args: &[Value]| -> Value {
                let Some(Value::Struct(s)) = args.first() else {
                    return Value::Void;
                };
                let field = |name: &str| -> String {
                    s.get_field(name)
                        .and_then(|v| match v {
                            Value::String(s) => Some(s.to_string()),
                            _ => None,
                        })
                        .unwrap_or_default()
                };
                let room_id = field("room-id");
                let body = field("body");
                let reply_to = field("reply-to");
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
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("accept-verification", move |_args: &[Value]| -> Value {
                if let Err(e) = tx.send(UiCommand::AcceptVerification) {
                    tracing::debug!("failed to send AcceptVerification command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("confirm-verification", move |_args: &[Value]| -> Value {
                if let Err(e) = tx.send(UiCommand::ConfirmVerification) {
                    tracing::debug!("failed to send ConfirmVerification command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("reject-verification", move |_args: &[Value]| -> Value {
                if let Err(e) = tx.send(UiCommand::RejectVerification) {
                    tracing::debug!("failed to send RejectVerification command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("open-media", move |args: &[Value]| -> Value {
                let event_id = args
                    .first()
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .unwrap_or_default();
                if !event_id.is_empty()
                    && let Err(e) = tx.send(UiCommand::OpenMedia { event_id })
                {
                    tracing::debug!("failed to send OpenMedia command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("save-file", move |args: &[Value]| -> Value {
                let Some(Value::Struct(s)) = args.first() else {
                    return Value::Void;
                };
                let field = |name: &str| -> String {
                    s.get_field(name)
                        .and_then(|v| match v {
                            Value::String(s) => Some(s.to_string()),
                            _ => None,
                        })
                        .unwrap_or_default()
                };
                let event_id = field("event-id");
                let filename = field("filename");
                if !event_id.is_empty()
                    && let Err(e) = tx.send(UiCommand::SaveFile { event_id, filename })
                {
                    tracing::debug!("failed to send SaveFile command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("scroll-position-changed", move |args: &[Value]| -> Value {
                if let Err(e) = tx.send(UiCommand::ScrollPositionChanged {
                    at_top: bool_arg(args, 0),
                    at_bottom: bool_arg(args, 1),
                }) {
                    tracing::debug!("failed to send ScrollPositionChanged command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        self.instance
            .set_callback("paginate-backwards", move |_args: &[Value]| -> Value {
                let room_id = weak
                    .upgrade()
                    .map(|inst| inst.get_string(StringProp::SelectedRoomId).to_string())
                    .unwrap_or_default();
                if !room_id.is_empty()
                    && let Err(e) = tx.send(UiCommand::PaginateBackwards {
                        room_id: RoomId::new(room_id),
                    })
                {
                    tracing::debug!("failed to send PaginateBackwards command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        self.instance
            .set_callback("paginate-forwards", move |_args: &[Value]| -> Value {
                let room_id = weak
                    .upgrade()
                    .map(|inst| inst.get_string(StringProp::SelectedRoomId).to_string())
                    .unwrap_or_default();
                if !room_id.is_empty()
                    && let Err(e) = tx.send(UiCommand::PaginateForwards {
                        room_id: RoomId::new(room_id),
                    })
                {
                    tracing::debug!("failed to send PaginateForwards command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        self.instance
            .set_callback("jump-to-latest", move |_args: &[Value]| -> Value {
                let room_id = weak
                    .upgrade()
                    .map(|inst| inst.get_string(StringProp::SelectedRoomId).to_string())
                    .unwrap_or_default();
                if !room_id.is_empty()
                    && let Err(e) = tx.send(UiCommand::JumpToLatest {
                        room_id: RoomId::new(room_id),
                    })
                {
                    tracing::debug!("failed to send JumpToLatest command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        Ok(())
    }

    pub fn spawn_event_handler(
        &self,
        mut ui_rx: mpsc::UnboundedReceiver<UiEvent>,
        media_cache: Arc<dyn MediaCache>,
    ) {
        let weak = self.instance.as_weak();
        let timeline_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let rooms_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let spaces_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let subspaces_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());

        set_prop(
            &self.instance,
            "timeline",
            Value::Model(ModelRc::from(Rc::clone(&timeline_model))),
        );
        set_prop(
            &self.instance,
            "rooms",
            Value::Model(ModelRc::from(Rc::clone(&rooms_model))),
        );
        set_prop(
            &self.instance,
            "spaces",
            Value::Model(ModelRc::from(Rc::clone(&spaces_model))),
        );
        set_prop(
            &self.instance,
            "subspaces",
            Value::Model(ModelRc::from(Rc::clone(&subspaces_model))),
        );

        TIMELINE_MODEL.with(|cell| *cell.borrow_mut() = Some(timeline_model));
        ROOMS_MODEL.with(|cell| *cell.borrow_mut() = Some(rooms_model));
        SPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(spaces_model));
        SUBSPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(subspaces_model));

        tokio::spawn(async move {
            while let Some(event) = ui_rx.recv().await {
                let media_cache = Arc::clone(&media_cache);
                weak.upgrade_in_event_loop(move |inst| {
                    let timeline = TIMELINE_MODEL.with(|cell| cell.borrow().clone());
                    let rooms = ROOMS_MODEL.with(|cell| cell.borrow().clone());
                    let spaces = SPACES_MODEL.with(|cell| cell.borrow().clone());
                    let subspaces = SUBSPACES_MODEL.with(|cell| cell.borrow().clone());
                    if let (Some(tl), Some(rm), Some(sm), Some(ssm)) =
                        (timeline, rooms, spaces, subspaces)
                    {
                        dispatch_ui_event(
                            &inst,
                            event,
                            &tl,
                            &rm,
                            &sm,
                            &ssm,
                            &|m| message_to_value(m, media_cache.as_ref()),
                            &|r| room_to_value(r, media_cache.as_ref()),
                            &|s| space_to_value(s, media_cache.as_ref()),
                            &|v| room_id_from_value(v).map_or("", SharedString::as_str),
                            &|v| room_id_from_value(v).map_or("", SharedString::as_str),
                            &event_id_from_value,
                        );
                    }
                })
                .ok();
            }
        });
    }

    pub fn run(&self) -> Result<()> {
        self.instance.run()?;
        Ok(())
    }

    #[cfg(feature = "demo")]
    pub fn set_window_size(&self, width: f32, height: f32) {
        self.instance
            .window()
            .set_size(slint::LogicalSize::new(width, height));
    }
}

fn emoji_entry_to_value(e: &emoji::EmojiEntry) -> Value {
    let tones: Vec<Value> = e
        .tones
        .iter()
        .map(|t| Value::String(SharedString::from(t.as_str())))
        .collect();

    Value::Struct(Struct::from_iter([
        (
            "base".to_string(),
            Value::String(SharedString::from(&e.base)),
        ),
        (
            "tones".to_string(),
            Value::Model(ModelRc::new(VecModel::from(tones))),
        ),
        (
            "name".to_string(),
            Value::String(SharedString::from(&e.name)),
        ),
    ]))
}

fn emoji_groups_to_value() -> Value {
    let groups: Vec<Value> = emoji::groups()
        .iter()
        .map(|items| {
            let entries: Vec<Value> = items.iter().map(emoji_entry_to_value).collect();
            Value::Struct(Struct::from_iter([(
                "items".to_string(),
                Value::Model(ModelRc::new(VecModel::from(entries))),
            )]))
        })
        .collect();

    Value::Model(ModelRc::new(VecModel::from(groups)))
}

fn emoji_search_results_to_value(query: &str) -> Value {
    let results: Vec<Value> = emoji::search(query)
        .iter()
        .map(emoji_entry_to_value)
        .collect();
    Value::Model(ModelRc::new(VecModel::from(results)))
}

fn setup_emoji_store(inst: &ComponentInstance) -> Result<()> {
    set_global_prop(inst, "EmojiStore", "groups", emoji_groups_to_value())?;

    let weak = inst.as_weak();
    inst.set_global_callback("EmojiStore", "search", move |args: &[Value]| -> Value {
        if let Some(inst) = weak.upgrade()
            && let Err(e) = inst.set_global_property(
                "EmojiStore",
                "results",
                emoji_search_results_to_value(&string_arg(args, 0)),
            )
        {
            tracing::warn!("failed to set EmojiStore.results: {e:?}");
        }
        Value::Void
    })
    .map_err(|e| AppError::Ui(format!("{e:?}")))?;

    inst.set_global_callback("EmojiStore", "insert", move |args: &[Value]| -> Value {
        let text = string_arg(args, 0);
        let offset = args
            .get(1)
            .and_then(|v| match v {
                Value::Number(n)
                    if n.is_finite()
                        && n.fract() == 0.0
                        && *n >= f64::from(i32::MIN)
                        && *n <= f64::from(i32::MAX) =>
                {
                    n.to_string().parse().ok()
                }
                _ => None,
            })
            .unwrap_or_default();
        let glyph = string_arg(args, 2);
        let (inserted, caret) = emoji::insert_at(&text, offset, &glyph);
        Value::Struct(Struct::from_iter([
            (
                "text".to_string(),
                Value::String(SharedString::from(inserted)),
            ),
            ("caret".to_string(), Value::Number(f64::from(caret))),
        ]))
    })
    .map_err(|e| AppError::Ui(format!("{e:?}")))?;

    Ok(())
}

fn message_to_value(m: &TimelineMessage, media: &dyn MediaCache) -> Value {
    let mut fields = vec![
        (
            "unique-id".to_string(),
            Value::String(SharedString::from(&m.unique_id)),
        ),
        (
            "sender".to_string(),
            Value::String(SharedString::from(message_sender_label(m))),
        ),
        (
            "body".to_string(),
            Value::String(SharedString::from(message_body_text(&m.body))),
        ),
        (
            "timestamp".to_string(),
            Value::String(SharedString::from(&message_timestamp_label(m.timestamp))),
        ),
        (
            "message-type".to_string(),
            Value::String(SharedString::from(message_type_token(&m.body))),
        ),
        (
            "preview-kind".to_string(),
            Value::String(SharedString::from(message_preview_kind_token(
                m.body.preview_kind(),
            ))),
        ),
        (
            "unsupported-kind".to_string(),
            Value::String(SharedString::from(unsupported_kind(&m.body))),
        ),
        (
            "event-id".to_string(),
            Value::String(SharedString::from(&m.event_id.0)),
        ),
    ];

    let mut has_thumbnail = false;
    let mut image_width: i32 = 0;
    let mut image_height: i32 = 0;
    if let MessageBody::Image { meta, .. } = &m.body {
        image_width = meta.width.unwrap_or(0).cast_signed();
        image_height = meta.height.unwrap_or(0).cast_signed();
        if let Some(thumb_path) = media.thumbnail_path(&m.event_id.0)
            && let Some(img) = load_image_cached(&thumb_path)
        {
            fields.push(("thumbnail".to_string(), Value::Image(img)));
            has_thumbnail = true;
        }
    }
    fields.push(("has-thumbnail".to_string(), Value::Bool(has_thumbnail)));
    fields.push((
        "image-width".to_string(),
        Value::Number(f64::from(image_width)),
    ));
    fields.push((
        "image-height".to_string(),
        Value::Number(f64::from(image_height)),
    ));

    let mut has_avatar = false;
    if let Some(avatar_path) = media.avatar_path(&m.sender)
        && let Some(img) = load_image_cached(&avatar_path)
    {
        fields.push(("avatar".to_string(), Value::Image(img)));
        has_avatar = true;
    }
    fields.push(("has-avatar".to_string(), Value::Bool(has_avatar)));
    fields.push((
        "sender-initial".to_string(),
        Value::String(SharedString::from(avatar_initials(message_sender_label(m)))),
    ));
    fields.push((
        "color-index".to_string(),
        Value::Number(f64::from(avatar_color_index(&m.sender))),
    ));
    let pronouns: Vec<Value> = pronoun_labels(&m.sender_pronouns)
        .into_iter()
        .map(|set| Value::String(SharedString::from(set)))
        .collect();
    fields.push((
        "pronouns".to_string(),
        Value::Model(ModelRc::new(VecModel::from(pronouns))),
    ));
    fields.push(("is-own".to_string(), Value::Bool(m.is_own)));
    fields.push(("edited".to_string(), Value::Bool(m.edited)));
    fields.extend(reply_fields(m));
    fields.push((
        "service-kind".to_string(),
        Value::String(SharedString::from(service_kind_token(&m.body))),
    ));
    fields.push((
        "service-target".to_string(),
        Value::String(SharedString::from(service_target(&m.body))),
    ));

    Value::Struct(Struct::from_iter(fields))
}

fn reply_fields(m: &TimelineMessage) -> Vec<(String, Value)> {
    vec![
        ("has-reply".to_string(), Value::Bool(m.reply.is_some())),
        (
            "reply-sender".to_string(),
            Value::String(SharedString::from(
                m.reply.as_ref().map_or("", |r| r.sender.as_str()),
            )),
        ),
        (
            "reply-kind".to_string(),
            Value::String(SharedString::from(
                m.reply
                    .as_ref()
                    .map_or("", |r| message_preview_kind_token(r.kind)),
            )),
        ),
        (
            "reply-body".to_string(),
            Value::String(SharedString::from(
                m.reply.as_ref().map_or("", |r| r.body.as_str()),
            )),
        ),
    ]
}

fn room_to_value(r: &Room, media: &dyn MediaCache) -> Value {
    let mut fields = vec![
        (
            "id".to_string(),
            Value::String(SharedString::from(r.id.as_ref())),
        ),
        (
            "name".to_string(),
            Value::String(SharedString::from(&r.display_name)),
        ),
        (
            "initial".to_string(),
            Value::String(SharedString::from(avatar_initials(&r.display_name))),
        ),
        (
            "color-index".to_string(),
            Value::Number(f64::from(avatar_color_index(r.id.as_ref()))),
        ),
        #[allow(clippy::cast_precision_loss)]
        (
            "members".to_string(),
            Value::Number(if r.is_direct { 0 } else { r.member_count } as f64),
        ),
        #[allow(clippy::cast_precision_loss)]
        ("unread".to_string(), Value::Number(r.unread_count as f64)),
        #[allow(clippy::cast_precision_loss)]
        (
            "mentions".to_string(),
            Value::Number(r.mention_count as f64),
        ),
        (
            "last-message-sender".to_string(),
            Value::String(SharedString::from(
                r.last_message_sender.as_deref().unwrap_or_default(),
            )),
        ),
        (
            "last-message-kind".to_string(),
            Value::String(SharedString::from(message_preview_kind_token(
                r.last_message_kind,
            ))),
        ),
        (
            "last-message-body".to_string(),
            Value::String(SharedString::from(&r.last_message_body)),
        ),
        (
            "last-message-is-own".to_string(),
            Value::Bool(r.last_message_is_own),
        ),
        (
            "last-message-edited".to_string(),
            Value::Bool(r.last_message_edited),
        ),
        (
            "last-message-time".to_string(),
            Value::String(SharedString::from(&room_activity_label(r.last_activity_ts))),
        ),
    ];

    let mut has_avatar = false;
    if let Some(mxc) = &r.avatar_mxc
        && let Some(avatar_path) = media.room_avatar_path(mxc)
        && let Some(img) = load_image_cached(&avatar_path)
    {
        fields.push(("avatar".to_string(), Value::Image(img)));
        has_avatar = true;
    }
    fields.push(("has-avatar".to_string(), Value::Bool(has_avatar)));

    Value::Struct(Struct::from_iter(fields))
}

fn space_to_value(s: &Space, media: &dyn MediaCache) -> Value {
    let mut fields = vec![
        ("id".to_string(), Value::String(SharedString::from(&s.id))),
        (
            "name".to_string(),
            Value::String(SharedString::from(&s.name)),
        ),
        #[allow(clippy::cast_precision_loss)]
        ("unread".to_string(), Value::Number(s.unread as f64)),
        #[allow(clippy::cast_precision_loss)]
        ("mentions".to_string(), Value::Number(s.mentions as f64)),
        (
            "initial".to_string(),
            Value::String(SharedString::from(sender_initial(&s.name))),
        ),
    ];

    let mut has_avatar = false;
    if let Some(mxc) = &s.avatar_mxc
        && let Some(avatar_path) = media.space_avatar_path(mxc)
        && let Some(img) = load_image_cached(&avatar_path)
    {
        fields.push(("avatar".to_string(), Value::Image(img)));
        has_avatar = true;
    }
    fields.push(("has-avatar".to_string(), Value::Bool(has_avatar)));

    Value::Struct(Struct::from_iter(fields))
}

fn event_id_from_value(val: &Value) -> String {
    if let Value::Struct(s) = val
        && let Some(Value::String(id)) = s.get_field("event-id")
    {
        id.to_string()
    } else {
        String::new()
    }
}

fn room_id_from_value(val: &Value) -> Option<&SharedString> {
    if let Value::Struct(s) = val
        && let Some(Value::String(id)) = s.get_field("id")
    {
        Some(id)
    } else {
        None
    }
}
