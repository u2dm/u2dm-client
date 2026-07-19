use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use adapters::browser::DesktopBrowser;
#[cfg(feature = "demo")]
use adapters::demo;
use adapters::media::DesktopMediaFiles;
use adapters::ui::{SlintUiAdapter, UiEventOutput};
use app::AppService;
use commands::{UiCommand, UiEvent, ViewportChanged};
use composition::Backend;
use domain::models::{ConnectionStatus, Room, Space};
use error::Result;
use ports::browser::BrowserPort;
use ports::media::MediaFilePort;
use ports::output::AppOutputPort;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use tracing_subscriber::EnvFilter;

const UI_EVENT_CHANNEL_CAP: usize = 256;

mod adapters;
mod app;
mod commands;
mod composition;
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
    let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>(UI_EVENT_CHANNEL_CAP);

    let (rooms_out_tx, rooms_out_rx) = watch::channel::<Arc<[Room]>>(Arc::from(Vec::new()));
    let (spaces_out_tx, spaces_out_rx) = watch::channel::<Arc<[Space]>>(Arc::from(Vec::new()));
    let (subspaces_out_tx, subspaces_out_rx) =
        watch::channel::<Arc<[Space]>>(Arc::from(Vec::new()));
    let (connection_out_tx, connection_out_rx) =
        watch::channel::<ConnectionStatus>(ConnectionStatus::Disconnected);
    let (status_out_tx, status_out_rx) = watch::channel::<String>(String::new());

    let (rooms_in_tx, rooms_in_rx) = watch::channel::<Arc<[Room]>>(Arc::from(Vec::new()));
    let (spaces_in_tx, spaces_in_rx) = watch::channel::<Arc<[Space]>>(Arc::from(Vec::new()));
    let (scroll_tx, scroll_rx) = watch::channel::<ViewportChanged>(ViewportChanged::initial());

    ui.register_callbacks(&cmd_tx, &scroll_tx)?;
    #[cfg(feature = "demo")]
    demo::size_window_for_screenshots(&ui);

    let _guard = rt.enter();
    let backend = Backend::select(&cfg);
    let media_files: Arc<dyn MediaFilePort> = Arc::new(DesktopMediaFiles::new());
    let browser: Arc<dyn BrowserPort> = Arc::new(DesktopBrowser::new());
    let output: Arc<dyn AppOutputPort> = Arc::new(UiEventOutput::new(
        ui_tx,
        rooms_out_tx,
        spaces_out_tx,
        subspaces_out_tx,
        connection_out_tx,
        status_out_tx,
    ));

    let cmd_tx_quit = cmd_tx.clone();
    ui.spawn_event_handler(
        ui_rx,
        rooms_out_rx,
        spaces_out_rx,
        subspaces_out_rx,
        connection_out_rx,
        status_out_rx,
        backend.media_cache,
    );
    if let Err(e) = cmd_tx.send(UiCommand::RestoreSession) {
        tracing::warn!("failed to send RestoreSession command: {e}");
    }
    let mut service = AppService::new(
        backend.matrix,
        backend.storage,
        media_files,
        browser,
        cmd_tx,
        rooms_in_tx,
        spaces_in_tx,
        output,
    );
    tokio::spawn(async move {
        service
            .run(cmd_rx, rooms_in_rx, spaces_in_rx, scroll_rx)
            .await;
    });

    ui.run()?;

    if let Err(e) = cmd_tx_quit.send(UiCommand::Quit) {
        tracing::debug!("failed to send Quit command: {e}");
    }
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
