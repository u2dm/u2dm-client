use std::io::{self, Write as _};
use std::process::ExitCode;

use error::{AppError, Result};
use slint_interpreter::{Compiler, ComponentHandle, ComponentInstance, PlatformError};
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

    instance.run()?;
    Ok(())
}
