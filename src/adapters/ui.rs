use slint::{ModelRc, VecModel};
use slint_interpreter::{
    Compiler, ComponentHandle, ComponentInstance, PlatformError, SharedString, Struct, Value,
};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{
    ConnectionStatus, LoginCredentials, LoginMethod, MessageBody, Room, RoomId, ServerInfo,
    TimelineMessage, UiErrorKind, VerificationEvent,
};
use crate::error::{AppError, Result};

fn set_prop(inst: &ComponentInstance, name: &str, value: Value) {
    if let Err(e) = inst.set_property(name, value) {
        tracing::warn!("failed to set property '{name}': {e:?}");
    }
}

impl From<PlatformError> for AppError {
    fn from(err: PlatformError) -> Self {
        Self::Ui(err.to_string())
    }
}

pub struct SlintUiAdapter {
    instance: ComponentInstance,
}

impl SlintUiAdapter {
    pub fn compile(rt: &Runtime) -> Result<Self> {
        let instance = rt.block_on(async {
            let result = Compiler::new().build_from_path("ui/main.slint").await;
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
                        Value::String(SharedString::from("Checking server...")),
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
                        Value::String(SharedString::from("Logging in...")),
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
            .set_callback("login-oauth", move |args: &[Value]| -> Value {
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
                        Value::String(SharedString::from("Opening browser...")),
                    );
                    set_prop(&inst, "login-error", Value::String(SharedString::default()));
                }

                if let Err(e) = tx.send(UiCommand::LoginOAuth(homeserver)) {
                    tracing::debug!("failed to send LoginOAuth command: {e}");
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("select-room", move |args: &[Value]| -> Value {
                let room_id = args
                    .first()
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .unwrap_or_default();

                if let Err(e) = tx.send(UiCommand::SelectRoom(RoomId(room_id))) {
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
                        room_id: RoomId(room_id),
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

        Ok(())
    }

    pub fn spawn_event_handler(&self, mut ui_rx: mpsc::UnboundedReceiver<UiEvent>) {
        let weak = self.instance.as_weak();
        tokio::spawn(async move {
            while let Some(event) = ui_rx.recv().await {
                let weak = weak.clone();
                slint::invoke_from_event_loop(move || {
                    let Some(inst) = weak.upgrade() else {
                        return;
                    };
                    dispatch_ui_event(&inst, event);
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

fn dispatch_ui_event(inst: &ComponentInstance, event: UiEvent) {
    match event {
        UiEvent::ServerInfo(info) => apply_server_info(inst, &info),
        UiEvent::LoginSuccess { user_id } => apply_login_success(inst, &user_id),
        UiEvent::Error { message, kind } => apply_error(inst, &message, &kind),
        UiEvent::Status(msg) => apply_status(inst, &msg),
        UiEvent::Rooms(rooms) => apply_rooms(inst, &rooms),
        UiEvent::Timeline(messages) => apply_timeline(inst, &messages),
        UiEvent::ConnectionStatus(status) => apply_connection_status(inst, &status),
        UiEvent::Verification(event) => apply_verification(inst, &event),
        UiEvent::LoggedOut => apply_logged_out(inst),
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
        Value::String(SharedString::from("credentials")),
    );
    set_prop(inst, "login-status", Value::String(SharedString::default()));
}

fn apply_login_success(inst: &ComponentInstance, user_id: &str) {
    set_prop(inst, "user-id", Value::String(SharedString::from(user_id)));
    set_prop(
        inst,
        "login-step",
        Value::String(SharedString::from("logged-in")),
    );
    set_prop(inst, "login-status", Value::String(SharedString::default()));
}

fn apply_error(inst: &ComponentInstance, msg: &str, _kind: &UiErrorKind) {
    set_prop(inst, "login-error", Value::String(SharedString::from(msg)));
    set_prop(inst, "login-status", Value::String(SharedString::default()));
}

fn apply_status(inst: &ComponentInstance, msg: &str) {
    set_prop(inst, "login-status", Value::String(SharedString::from(msg)));
}

fn apply_timeline(inst: &ComponentInstance, messages: &[TimelineMessage]) {
    let entries: Vec<Value> = messages
        .iter()
        .map(|m| {
            let body_text = match &m.body {
                MessageBody::Text(s)
                | MessageBody::Notice(s)
                | MessageBody::Emote(s)
                | MessageBody::Image(s)
                | MessageBody::File(s)
                | MessageBody::Unknown(s) => s.clone(),
            };
            let sender = m
                .sender_display_name
                .as_deref()
                .unwrap_or(&m.sender)
                .to_string();
            let timestamp = chrono::DateTime::from_timestamp((m.timestamp / 1000).cast_signed(), 0)
                .map(|utc| {
                    utc.with_timezone(&chrono::Local)
                        .format("%H:%M")
                        .to_string()
                })
                .unwrap_or_default();

            Value::Struct(Struct::from_iter([
                (
                    "sender".to_string(),
                    Value::String(SharedString::from(&sender)),
                ),
                (
                    "body".to_string(),
                    Value::String(SharedString::from(&body_text)),
                ),
                (
                    "timestamp".to_string(),
                    Value::String(SharedString::from(&timestamp)),
                ),
            ]))
        })
        .collect();
    let model = Value::Model(ModelRc::new(VecModel::from(entries)));
    set_prop(inst, "timeline", model);
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
                Value::String(SharedString::from("requested")),
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
                Value::String(SharedString::from("emojis")),
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
                Value::String(SharedString::from("confirming")),
            );
        }
        VerificationEvent::Done => {
            set_prop(
                inst,
                "verification-step",
                Value::String(SharedString::from("done")),
            );
        }
        VerificationEvent::Cancelled(reason) => {
            set_prop(
                inst,
                "verification-step",
                Value::String(SharedString::from("cancelled")),
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
        Value::String(SharedString::from("homeserver")),
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
        Value::String(SharedString::from("disconnected")),
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
    let empty_model = Value::Model(ModelRc::new(VecModel::<Value>::default()));
    set_prop(inst, "rooms", empty_model.clone());
    set_prop(inst, "timeline", empty_model.clone());
    set_prop(inst, "verification-emojis", empty_model);
}

fn apply_rooms(inst: &ComponentInstance, rooms: &[Room]) {
    let entries: Vec<Value> = rooms
        .iter()
        .map(|r| {
            Value::Struct(Struct::from_iter([
                ("id".to_string(), Value::String(SharedString::from(&r.id.0))),
                (
                    "name".to_string(),
                    Value::String(SharedString::from(&r.display_name)),
                ),
                #[allow(clippy::cast_precision_loss)]
                ("unread".to_string(), Value::Number(r.unread_count as f64)),
            ]))
        })
        .collect();
    let model = Value::Model(ModelRc::new(VecModel::from(entries)));
    set_prop(inst, "rooms", model);
}
