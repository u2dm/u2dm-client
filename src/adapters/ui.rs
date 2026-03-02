use slint::{ModelRc, VecModel};
use slint_interpreter::{
    Compiler, ComponentHandle, ComponentInstance, PlatformError, SharedString, Struct, Value,
};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{
    ConnectionStatus, LoginCredentials, LoginMethod, MessageBody, Room, RoomId, ServerInfo,
    TimelineMessage,
};
use crate::error::{AppError, Result};

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
    pub fn register_callbacks(&self, cmd_tx: &mpsc::Sender<UiCommand>) -> Result<()> {
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
                    let _r = inst.set_property(
                        "login-status",
                        Value::String(SharedString::from("Checking server...")),
                    );
                    let _r =
                        inst.set_property("login-error", Value::String(SharedString::default()));
                }

                drop(tx.try_send(UiCommand::CheckServer(homeserver)));
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
                    let _r = inst.set_property(
                        "login-status",
                        Value::String(SharedString::from("Logging in...")),
                    );
                    let _r =
                        inst.set_property("login-error", Value::String(SharedString::default()));
                }

                drop(tx.try_send(UiCommand::LoginPassword(creds)));
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
                    let _r = inst.set_property(
                        "login-status",
                        Value::String(SharedString::from("Opening browser...")),
                    );
                    let _r =
                        inst.set_property("login-error", Value::String(SharedString::default()));
                }

                drop(tx.try_send(UiCommand::LoginOAuth(homeserver)));
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

                drop(tx.try_send(UiCommand::SelectRoom(RoomId(room_id))));
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        let tx = cmd_tx.clone();
        self.instance
            .set_callback("logout", move |_args: &[Value]| -> Value {
                drop(tx.try_send(UiCommand::Logout));
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
                if !room_id.is_empty() && !body.is_empty() {
                    drop(tx.try_send(UiCommand::SendMessage {
                        room_id: RoomId(room_id),
                        body,
                    }));
                }
                Value::Void
            })
            .map_err(|e| AppError::Ui(format!("{e:?}")))?;

        Ok(())
    }

    pub fn spawn_event_handler(&self, mut ui_rx: mpsc::Receiver<UiEvent>) {
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
        UiEvent::Error(msg) => apply_error(inst, &msg),
        UiEvent::Status(msg) => apply_status(inst, &msg),
        UiEvent::Rooms(rooms) => apply_rooms(inst, &rooms),
        UiEvent::Timeline(messages) => apply_timeline(inst, &messages),
        UiEvent::ConnectionStatus(status) => apply_connection_status(inst, &status),
        UiEvent::LoggedOut => apply_logged_out(inst),
    }
}

fn apply_server_info(inst: &ComponentInstance, info: &ServerInfo) {
    let method = LoginMethod::from_auth_methods(&info.auth_methods);
    let _r = inst.set_property(
        "login-method",
        Value::String(SharedString::from(method.as_str())),
    );
    let _r = inst.set_property(
        "resolved-homeserver",
        Value::String(SharedString::from(&info.homeserver_url)),
    );
    let _r = inst.set_property(
        "login-step",
        Value::String(SharedString::from("credentials")),
    );
    let _r = inst.set_property("login-status", Value::String(SharedString::default()));
}

fn apply_login_success(inst: &ComponentInstance, user_id: &str) {
    let _r = inst.set_property("user-id", Value::String(SharedString::from(user_id)));
    let _r = inst.set_property("login-step", Value::String(SharedString::from("logged-in")));
    let _r = inst.set_property("login-status", Value::String(SharedString::default()));
}

fn apply_error(inst: &ComponentInstance, msg: &str) {
    let _r = inst.set_property("login-error", Value::String(SharedString::from(msg)));
    let _r = inst.set_property("login-status", Value::String(SharedString::default()));
}

fn apply_status(inst: &ComponentInstance, msg: &str) {
    let _r = inst.set_property("login-status", Value::String(SharedString::from(msg)));
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
            let ts_secs = m.timestamp / 1000;
            let hrs = (ts_secs / 3600) % 24;
            let mins = (ts_secs / 60) % 60;
            let timestamp = format!("{hrs:02}:{mins:02}");

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
    let _r = inst.set_property("timeline", model);
}

fn apply_connection_status(inst: &ComponentInstance, status: &ConnectionStatus) {
    let _r = inst.set_property(
        "connection-status",
        Value::String(SharedString::from(status.as_str())),
    );
}

fn apply_logged_out(inst: &ComponentInstance) {
    let _r = inst.set_property(
        "login-step",
        Value::String(SharedString::from("homeserver")),
    );
    let _r = inst.set_property("user-id", Value::String(SharedString::default()));
    let _r = inst.set_property("login-status", Value::String(SharedString::default()));
    let _r = inst.set_property("login-error", Value::String(SharedString::default()));
    let _r = inst.set_property("login-method", Value::String(SharedString::default()));
    let _r = inst.set_property(
        "resolved-homeserver",
        Value::String(SharedString::default()),
    );
    let _r = inst.set_property("selected-room-name", Value::String(SharedString::default()));
    let _r = inst.set_property("selected-room-id", Value::String(SharedString::default()));
    let _r = inst.set_property("input-username", Value::String(SharedString::default()));
    let _r = inst.set_property("input-password", Value::String(SharedString::default()));
    let _r = inst.set_property(
        "connection-status",
        Value::String(SharedString::from("disconnected")),
    );
    let empty_model = Value::Model(ModelRc::new(VecModel::<Value>::default()));
    let _r = inst.set_property("rooms", empty_model.clone());
    let _r = inst.set_property("timeline", empty_model);
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
    let _r = inst.set_property("rooms", model);
}
