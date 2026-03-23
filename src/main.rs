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
use tracing_subscriber::EnvFilter;

mod adapters;
mod app;
mod commands;
mod config;
mod domain;
mod error;
mod ports;
mod util;

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
    drop(
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .try_init(),
    );
}

fn run() -> Result<()> {
    slint::init_translations!(concat!(env!("CARGO_MANIFEST_DIR"), "/lang/"));
    let rt = Runtime::new()?;
    let cfg = config::AppConfig::from_env()?;
    tracing::info!(data_dir = %cfg.data_dir.display(), cache_dir = %cfg.cache_dir.display(), "starting U2DM");
    let ui = SlintUiAdapter::compile(&rt)?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<UiCommand>();
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiEvent>();

    ui.register_callbacks(&cmd_tx)?;

    let matrix_adapter = MatrixAdapter::new(cfg.data_dir.clone(), cfg.cache_dir.clone());
    matrix_adapter.clean_media_cache();
    let matrix: Arc<dyn MatrixPort> = Arc::new(matrix_adapter);
    let storage: Arc<dyn StoragePort> = Arc::new(SecureStorage::new(&cfg.data_dir));

    let cmd_tx_quit = cmd_tx.clone();
    let _guard = rt.enter();
    ui.spawn_event_handler(ui_rx);
    if let Err(e) = cmd_tx.send(UiCommand::RestoreSession) {
        tracing::warn!("failed to send RestoreSession command: {e}");
    }
    let mut service = AppService::new(matrix, storage, cmd_rx, cmd_tx, ui_tx);
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
