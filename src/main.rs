use std::io::{self, Write as _};
use std::process::ExitCode;

use error::{AppError, Result};
use slint_interpreter::{
    Compiler, ComponentHandle, ComponentInstance, PlatformError, SharedString, Value,
};
use tokio::runtime::Runtime;

mod error;

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

    let compiled: Result<ComponentInstance> = rt.block_on(async {
        let result = Compiler::new().build_from_path("ui/main.slint").await;
        let def = result
            .component("AppWindow")
            .ok_or_else(|| AppError::Ui("failed to load ui/main.slint".into()))?;
        Ok(def.create()?)
    });
    let instance = compiled?;

    let weak = instance.as_weak();
    instance
        .set_callback("check-server", move |args: &[Value]| -> Value {
            let _homeserver = args
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

            Value::Void
        })
        .map_err(|e| AppError::Ui(format!("{e:?}")))?;

    instance.run()?;
    Ok(())
}
