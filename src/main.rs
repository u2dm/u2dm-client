use std::io::{self, Write as _};
use std::process::ExitCode;
use std::sync::Arc;

use adapters::matrix::MatrixAdapter;
use adapters::storage::JsonFileStorage;
use adapters::ui::SlintUiAdapter;
use app::AppService;
use commands::{UiCommand, UiEvent};
use error::Result;
use ports::matrix::MatrixPort;
use ports::storage::StoragePort;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

mod adapters;
mod app;
mod commands;
mod config;
mod domain;
mod error;
mod ports;

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
    let cfg = config::AppConfig::from_env()?;
    let ui = SlintUiAdapter::compile(&rt)?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(8);
    let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>(32);

    ui.register_callbacks(&cmd_tx)?;

    let matrix: Arc<dyn MatrixPort> = Arc::new(MatrixAdapter::new(cfg.data_dir.clone()));
    let storage: Arc<dyn StoragePort> = Arc::new(JsonFileStorage::new(&cfg.data_dir));

    {
        let _guard = rt.enter();
        ui.spawn_event_handler(ui_rx);
        drop(cmd_tx.try_send(UiCommand::RestoreSession));
        let mut service = AppService::new(matrix, storage, cmd_rx, cmd_tx, ui_tx);
        tokio::spawn(async move {
            service.run().await;
        });
    }

    ui.run()
}
