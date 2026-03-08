use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use adapters::matrix::MatrixAdapter;
use adapters::storage::SecureStorage;
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
    init_tracing();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    drop(
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .try_init(),
    );
}

fn run() -> Result<()> {
    let rt = Runtime::new()?;
    let cfg = config::AppConfig::from_env()?;
    let ui = SlintUiAdapter::compile(&rt)?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<UiCommand>();
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiEvent>();

    ui.register_callbacks(&cmd_tx)?;

    let matrix: Arc<dyn MatrixPort> = Arc::new(MatrixAdapter::new(
        cfg.data_dir.clone(),
        cfg.cache_dir.clone(),
    ));
    let storage: Arc<dyn StoragePort> = Arc::new(SecureStorage::new(&cfg.data_dir));

    let cmd_tx_quit = cmd_tx.clone();
    let _guard = rt.enter();
    ui.spawn_event_handler(ui_rx);
    if let Err(e) = cmd_tx.send(UiCommand::RestoreSession) {
        tracing::warn!("failed to send RestoreSession command: {e}");
    }
    let mut service = AppService::new(matrix, storage, cfg, cmd_rx, cmd_tx, ui_tx);
    tokio::spawn(async move {
        service.run().await;
    });

    ui.run()?;

    if let Err(e) = cmd_tx_quit.send(UiCommand::Quit) {
        tracing::debug!("failed to send Quit command: {e}");
    }
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
