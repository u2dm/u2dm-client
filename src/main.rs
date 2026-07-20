use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use adapters::browser::DesktopBrowser;
#[cfg(feature = "demo")]
use adapters::demo;
use adapters::media::DesktopMediaFiles;
use adapters::ui::{SlintUiAdapter, UiEventOutput};
use app::AppService;
use commands::{AppViewState, DirectoryUpdate, Effect, UiCommand, ViewportChanged};
use composition::Backend;
use error::Result;
use ports::browser::BrowserPort;
use ports::media::MediaFilePort;
use ports::output::AppOutputPort;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing_subscriber::EnvFilter;

const UI_EVENT_CHANNEL_CAP: usize = 256;
const SHUTDOWN_WAIT: Duration = Duration::from_secs(6);
const SHUTDOWN_BACKSTOP: Duration = Duration::from_secs(1);

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
    let (ui_tx, ui_rx) = mpsc::channel::<Effect>(UI_EVENT_CHANNEL_CAP);

    let (view_out_tx, view_out_rx) =
        watch::channel::<Arc<AppViewState>>(Arc::new(AppViewState::default()));

    let (dir_in_tx, dir_in_rx) = mpsc::unbounded_channel::<DirectoryUpdate>();
    let (scroll_tx, scroll_rx) = watch::channel::<ViewportChanged>(ViewportChanged::initial());

    ui.register_callbacks(&cmd_tx, &scroll_tx)?;
    #[cfg(feature = "demo")]
    demo::size_window_for_screenshots(&ui);

    let enter_guard = rt.enter();
    let backend = Backend::select(&cfg);
    let media_files: Arc<dyn MediaFilePort> = Arc::new(DesktopMediaFiles::new());
    let browser: Arc<dyn BrowserPort> = Arc::new(DesktopBrowser::new());
    let output: Arc<dyn AppOutputPort> = Arc::new(UiEventOutput::new(ui_tx, view_out_tx));

    let cmd_tx_quit = cmd_tx.clone();
    ui.spawn_event_handler(ui_rx, view_out_rx, backend.media_cache);
    if let Err(e) = cmd_tx.send(UiCommand::RestoreSession) {
        tracing::warn!("failed to send RestoreSession command: {e}");
    }
    let mut service = AppService::new(
        backend.auth,
        backend.storage,
        media_files,
        browser,
        cmd_tx,
        dir_in_tx,
        output,
    );
    let service_handle = tokio::spawn(async move {
        service.run(cmd_rx, dir_in_rx, scroll_rx).await;
    });

    ui.run()?;
    drop(enter_guard);

    shutdown(rt, &cmd_tx_quit, service_handle);
    Ok(())
}

fn shutdown(
    rt: Runtime,
    cmd_tx_quit: &mpsc::UnboundedSender<UiCommand>,
    service_handle: JoinHandle<()>,
) {
    if let Err(e) = cmd_tx_quit.send(UiCommand::Quit) {
        tracing::debug!("failed to send Quit command: {e}");
    }
    let cleaned_up = rt
        .block_on(async { timeout(SHUTDOWN_WAIT, service_handle).await })
        .is_ok();
    if !cleaned_up {
        tracing::warn!("service cleanup did not finish before deadline; forcing shutdown");
    }
    rt.shutdown_timeout(SHUTDOWN_BACKSTOP);
}
