use std::env;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::sync::Arc;

use adapters::matrix::MatrixAdapter;
use commands::UiCommand;
use domain::models::{LoginMethod, ServerInfo};
use error::{AppError, Result};
use ports::matrix::MatrixPort;
use slint_interpreter::{
    Compiler, ComponentHandle, ComponentInstance, PlatformError, SharedString, Value,
};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

mod adapters;
mod commands;
mod domain;
mod error;
mod ports;

impl From<PlatformError> for AppError {
    fn from(err: PlatformError) -> Self {
        Self::Ui(err.to_string())
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            writeln!(io::stderr(), "Error: {e}").ok();
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let rt = Runtime::new()?;
    let instance = compile_ui(&rt)?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(8);

    register_callbacks(&instance, &cmd_tx)?;
    spawn_command_handler(&rt, cmd_rx, instance.as_weak());

    instance.run()?;
    Ok(())
}

fn compile_ui(rt: &Runtime) -> Result<ComponentInstance> {
    rt.block_on(async {
        let result = Compiler::new().build_from_path("ui/main.slint").await;
        let def = result
            .component("AppWindow")
            .ok_or_else(|| AppError::Ui("failed to load ui/main.slint".into()))?;
        Ok(def.create()?)
    })
}

fn register_callbacks(
    instance: &ComponentInstance,
    cmd_tx: &mpsc::Sender<UiCommand>,
) -> Result<()> {
    let tx = cmd_tx.clone();
    let weak = instance.as_weak();
    instance
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
                let _r = inst.set_property("login-error", Value::String(SharedString::default()));
            }

            drop(tx.try_send(UiCommand::CheckServer(homeserver)));
            Value::Void
        })
        .map_err(|e| AppError::Ui(format!("{e:?}")))?;

    Ok(())
}

fn spawn_command_handler(
    rt: &Runtime,
    mut cmd_rx: mpsc::Receiver<UiCommand>,
    weak: slint_interpreter::Weak<ComponentInstance>,
) {
    let data_dir = env::current_dir().unwrap_or_default().join("data");
    let matrix: Arc<dyn MatrixPort> = Arc::new(MatrixAdapter::new(data_dir));

    let _guard = rt.enter();
    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                UiCommand::CheckServer(homeserver) => {
                    let result = matrix.discover_auth(&homeserver).await;
                    let weak = weak.clone();
                    slint::invoke_from_event_loop(move || {
                        let Some(inst) = weak.upgrade() else {
                            return;
                        };
                        match result {
                            Ok(info) => apply_server_info(&inst, &info),
                            Err(e) => apply_error(&inst, &e.to_string()),
                        }
                    })
                    .ok();
                }
            }
        }
    });
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

fn apply_error(inst: &ComponentInstance, msg: &str) {
    let _r = inst.set_property("login-error", Value::String(SharedString::from(msg)));
    let _r = inst.set_property("login-status", Value::String(SharedString::default()));
}
