use std::cell::RefCell;
use std::rc::Rc;

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
}

use super::common::{
    BoolProp, Status, StringProp, UiProps, dispatch_ui_event, load_image_cached, sender_initial,
};
use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{
    LoginCredentials, MessageBody, Room, RoomId, Space, TimelineMessage,
    VerificationEmoji as DomainVerificationEmoji,
};
use crate::error::{AppError, Result};

fn set_prop(inst: &ComponentInstance, name: &str, value: Value) {
    if let Err(e) = inst.set_property(name, value) {
        tracing::warn!("failed to set property '{name}': {e:?}");
    }
}

impl UiProps for ComponentInstance {
    fn set_string(&self, prop: StringProp, value: SharedString) {
        set_prop(self, prop.as_str(), Value::String(value));
    }

    fn set_bool(&self, prop: BoolProp, value: bool) {
        set_prop(self, prop.as_str(), Value::Bool(value));
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
            let result = Compiler::new().build_from_path("ui/main.slint").await;
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
            .set_callback("move-space", move |args: &[Value]| -> Value {
                let index = |i: usize| -> Option<usize> {
                    match args.get(i) {
                        Some(Value::Number(n)) if *n >= 0.0 => usize::try_from(*n as i64).ok(),
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
                if !room_id.is_empty()
                    && !body.is_empty()
                    && let Err(e) = tx.send(UiCommand::SendMessage {
                        room_id: RoomId::new(room_id),
                        body,
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

        Ok(())
    }

    pub fn spawn_event_handler(&self, mut ui_rx: mpsc::UnboundedReceiver<UiEvent>) {
        let weak = self.instance.as_weak();
        let timeline_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let rooms_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let spaces_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());

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

        TIMELINE_MODEL.with(|cell| *cell.borrow_mut() = Some(timeline_model));
        ROOMS_MODEL.with(|cell| *cell.borrow_mut() = Some(rooms_model));
        SPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(spaces_model));

        tokio::spawn(async move {
            while let Some(event) = ui_rx.recv().await {
                weak.upgrade_in_event_loop(move |inst| {
                    let timeline = TIMELINE_MODEL.with(|cell| cell.borrow().clone());
                    let rooms = ROOMS_MODEL.with(|cell| cell.borrow().clone());
                    let spaces = SPACES_MODEL.with(|cell| cell.borrow().clone());
                    if let (Some(tl), Some(rm), Some(sm)) = (timeline, rooms, spaces) {
                        dispatch_ui_event(
                            &inst,
                            event,
                            &tl,
                            &rm,
                            &sm,
                            &message_to_value,
                            &room_to_value,
                            &space_to_value,
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
}

fn message_to_value(m: &TimelineMessage) -> Value {
    let mut fields = vec![
        (
            "sender".to_string(),
            Value::String(SharedString::from(m.display_sender())),
        ),
        (
            "body".to_string(),
            Value::String(SharedString::from(&m.body.display_text())),
        ),
        (
            "timestamp".to_string(),
            Value::String(SharedString::from(&m.display_timestamp())),
        ),
        (
            "message-type".to_string(),
            Value::String(SharedString::from(m.body.type_str())),
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
        if let Some(thumb_path) = &meta.thumbnail_path
            && let Some(img) = load_image_cached(thumb_path)
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
    if let Some(avatar_path) = &m.sender_avatar_path
        && let Some(img) = load_image_cached(avatar_path)
    {
        fields.push(("avatar".to_string(), Value::Image(img)));
        has_avatar = true;
    }
    fields.push(("has-avatar".to_string(), Value::Bool(has_avatar)));
    fields.push((
        "sender-initial".to_string(),
        Value::String(SharedString::from(sender_initial(m.display_sender()))),
    ));
    fields.push(("is-own".to_string(), Value::Bool(m.is_own)));

    Value::Struct(Struct::from_iter(fields))
}

fn room_to_value(r: &Room) -> Value {
    Value::Struct(Struct::from_iter([
        (
            "id".to_string(),
            Value::String(SharedString::from(r.id.as_ref())),
        ),
        (
            "name".to_string(),
            Value::String(SharedString::from(&r.display_name)),
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
            Value::String(SharedString::from(&r.last_message_kind)),
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
            "last-message-time".to_string(),
            Value::String(SharedString::from(&r.last_activity_label())),
        ),
    ]))
}

fn space_to_value(s: &Space) -> Value {
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
    if let Some(avatar_path) = &s.avatar_path
        && let Some(img) = load_image_cached(avatar_path)
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
