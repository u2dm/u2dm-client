use std::cell::RefCell;
use std::rc::Rc;

use slint::{ModelRc, VecModel};
use slint_interpreter::{
    Compiler, ComponentHandle, ComponentInstance, SharedString, Struct, Value,
};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

thread_local! {
    static TIMELINE_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
    static ROOMS_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
}

use super::common::{LoginStep, Status, VerifyStep, apply_rooms, apply_timeline_patch};
use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{
    ConnectionStatus, LoginCredentials, LoginMethod, MessageBody, Room, RoomId, ServerInfo,
    TimelineMessage, VerificationEvent,
};
use crate::error::{AppError, Result};

fn set_prop(inst: &ComponentInstance, name: &str, value: Value) {
    if let Err(e) = inst.set_property(name, value) {
        tracing::warn!("failed to set property '{name}': {e:?}");
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

        TIMELINE_MODEL.with(|cell| *cell.borrow_mut() = Some(timeline_model));
        ROOMS_MODEL.with(|cell| *cell.borrow_mut() = Some(rooms_model));

        tokio::spawn(async move {
            while let Some(event) = ui_rx.recv().await {
                weak.upgrade_in_event_loop(move |inst| {
                    TIMELINE_MODEL.with(|cell| {
                        if let Some(tl) = cell.borrow().as_ref() {
                            ROOMS_MODEL.with(|rc| {
                                if let Some(rm) = rc.borrow().as_ref() {
                                    dispatch_ui_event(&inst, event, tl, rm);
                                }
                            });
                        }
                    });
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

fn dispatch_ui_event(
    inst: &ComponentInstance,
    event: UiEvent,
    timeline_model: &VecModel<Value>,
    rooms_model: &VecModel<Value>,
) {
    match event {
        UiEvent::ServerInfo(info) => apply_server_info(inst, &info),
        UiEvent::ShowLogin => apply_show_login(inst),
        UiEvent::LoginSuccess { user_id } => apply_login_success(inst, &user_id),
        UiEvent::LoginError(message) => apply_login_error(inst, &message),
        UiEvent::ToastError(message) => apply_toast_error(inst, &message),
        UiEvent::Status(msg) => apply_status(inst, &msg),
        UiEvent::Rooms(rooms) => {
            apply_rooms(rooms_model, &rooms, &room_to_value, &|v| {
                room_id_from_value(v).map_or("", |s| s.as_str())
            });
        }
        UiEvent::Timeline { room_id, patch } => {
            let selected = inst
                .get_property("selected-room-id")
                .ok()
                .and_then(|v| match v {
                    Value::String(s) => Some(s),
                    _ => None,
                });
            if selected
                .as_ref()
                .is_some_and(|s| s.as_str() == room_id.as_ref())
            {
                set_prop(inst, "timeline-loading", Value::Bool(false));
                apply_timeline_patch(timeline_model, *patch, &message_to_value);
            }
        }
        UiEvent::ConnectionStatus(status) => apply_connection_status(inst, &status),
        UiEvent::Verification(event) => apply_verification(inst, &event),
        UiEvent::FileSaved { path } => {
            set_prop(
                inst,
                "saved-file-path",
                Value::String(SharedString::from(&path)),
            );
            set_prop(
                inst,
                "toast-message",
                Value::String(SharedString::from(Status::FileSaved.as_str())),
            );
        }
        UiEvent::LoggedOut => {
            timeline_model.set_vec(Vec::new());
            rooms_model.set_vec(Vec::new());
            apply_logged_out(inst);
        }
    }
}

fn apply_server_info(inst: &ComponentInstance, info: &ServerInfo) {
    let method = LoginMethod::from_auth_methods(&info.auth_methods);
    set_prop(
        inst,
        "login-method",
        Value::String(SharedString::from(method.as_str())),
    );
    set_prop(
        inst,
        "resolved-homeserver",
        Value::String(SharedString::from(&info.homeserver_url)),
    );
    set_prop(
        inst,
        "login-step",
        Value::String(SharedString::from(LoginStep::Credentials.as_str())),
    );
    set_prop(inst, "login-status", Value::String(SharedString::default()));
}

fn apply_show_login(inst: &ComponentInstance) {
    set_prop(
        inst,
        "login-step",
        Value::String(SharedString::from(LoginStep::Homeserver.as_str())),
    );
    set_prop(inst, "login-status", Value::String(SharedString::default()));
}

fn apply_login_success(inst: &ComponentInstance, user_id: &str) {
    set_prop(inst, "user-id", Value::String(SharedString::from(user_id)));
    set_prop(
        inst,
        "login-step",
        Value::String(SharedString::from(LoginStep::LoggedIn.as_str())),
    );
    set_prop(inst, "login-status", Value::String(SharedString::default()));
}

fn apply_login_error(inst: &ComponentInstance, msg: &str) {
    set_prop(inst, "login-error", Value::String(SharedString::from(msg)));
    set_prop(inst, "login-status", Value::String(SharedString::default()));
}

fn apply_toast_error(inst: &ComponentInstance, msg: &str) {
    set_prop(
        inst,
        "toast-message",
        Value::String(SharedString::from(msg)),
    );
}

fn apply_status(inst: &ComponentInstance, msg: &str) {
    set_prop(inst, "login-status", Value::String(SharedString::from(msg)));
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
    if let MessageBody::Image { meta, .. } = &m.body
        && let Some(thumb_path) = &meta.thumbnail_path
        && let Ok(img) = slint::Image::load_from_path(thumb_path)
    {
        fields.push(("thumbnail".to_string(), Value::Image(img)));
        has_thumbnail = true;
    }
    fields.push(("has-thumbnail".to_string(), Value::Bool(has_thumbnail)));

    let mut has_avatar = false;
    if let Some(avatar_path) = &m.sender_avatar_path
        && let Ok(img) = slint::Image::load_from_path(avatar_path)
    {
        fields.push(("avatar".to_string(), Value::Image(img)));
        has_avatar = true;
    }
    fields.push(("has-avatar".to_string(), Value::Bool(has_avatar)));
    fields.push(("is-own".to_string(), Value::Bool(m.is_own)));

    Value::Struct(Struct::from_iter(fields))
}

fn apply_connection_status(inst: &ComponentInstance, status: &ConnectionStatus) {
    set_prop(
        inst,
        "connection-status",
        Value::String(SharedString::from(status.as_str())),
    );
}

fn apply_verification(inst: &ComponentInstance, event: &VerificationEvent) {
    match event {
        VerificationEvent::Requested { sender, is_self } => {
            set_prop(inst, "verification-visible", Value::Bool(true));
            set_prop(
                inst,
                "verification-step",
                Value::String(SharedString::from(VerifyStep::Requested.as_str())),
            );
            set_prop(
                inst,
                "verification-sender",
                Value::String(SharedString::from(sender.as_str())),
            );
            set_prop(inst, "verification-is-self", Value::Bool(*is_self));
            set_prop(
                inst,
                "verification-error",
                Value::String(SharedString::default()),
            );
        }
        VerificationEvent::Emojis(emojis) => {
            set_prop(
                inst,
                "verification-step",
                Value::String(SharedString::from(VerifyStep::Emojis.as_str())),
            );
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
            let model = Value::Model(ModelRc::new(VecModel::from(entries)));
            set_prop(inst, "verification-emojis", model);
        }
        VerificationEvent::Confirming => {
            set_prop(
                inst,
                "verification-step",
                Value::String(SharedString::from(VerifyStep::Confirming.as_str())),
            );
        }
        VerificationEvent::Done => {
            set_prop(
                inst,
                "verification-step",
                Value::String(SharedString::from(VerifyStep::Done.as_str())),
            );
        }
        VerificationEvent::Cancelled(reason) => {
            set_prop(
                inst,
                "verification-step",
                Value::String(SharedString::from(VerifyStep::Cancelled.as_str())),
            );
            set_prop(
                inst,
                "verification-error",
                Value::String(SharedString::from(reason.as_str())),
            );
        }
    }
}

fn apply_logged_out(inst: &ComponentInstance) {
    set_prop(
        inst,
        "login-step",
        Value::String(SharedString::from(LoginStep::Homeserver.as_str())),
    );
    set_prop(inst, "user-id", Value::String(SharedString::default()));
    set_prop(inst, "login-status", Value::String(SharedString::default()));
    set_prop(inst, "login-error", Value::String(SharedString::default()));
    set_prop(inst, "login-method", Value::String(SharedString::default()));
    set_prop(
        inst,
        "resolved-homeserver",
        Value::String(SharedString::default()),
    );
    set_prop(
        inst,
        "selected-room-name",
        Value::String(SharedString::default()),
    );
    set_prop(
        inst,
        "selected-room-id",
        Value::String(SharedString::default()),
    );
    set_prop(
        inst,
        "input-username",
        Value::String(SharedString::default()),
    );
    set_prop(
        inst,
        "input-password",
        Value::String(SharedString::default()),
    );
    set_prop(
        inst,
        "connection-status",
        Value::String(SharedString::from(ConnectionStatus::Disconnected.as_str())),
    );
    set_prop(inst, "verification-visible", Value::Bool(false));
    set_prop(
        inst,
        "verification-step",
        Value::String(SharedString::default()),
    );
    set_prop(
        inst,
        "verification-sender",
        Value::String(SharedString::default()),
    );
    set_prop(inst, "verification-is-self", Value::Bool(false));
    set_prop(
        inst,
        "verification-error",
        Value::String(SharedString::default()),
    );
    set_prop(
        inst,
        "toast-message",
        Value::String(SharedString::default()),
    );
    set_prop(
        inst,
        "saved-file-path",
        Value::String(SharedString::default()),
    );
    let empty_model = Value::Model(ModelRc::new(VecModel::<Value>::default()));
    set_prop(inst, "verification-emojis", empty_model);
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
    ]))
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
