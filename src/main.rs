#![recursion_limit = "256"]

use std::env;
use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "demo")]
use adapters::demo;
use adapters::private_fs;
use adapters::ui::{SlintUiAdapter, UiEventOutput};
use app::AppService;
use commands::effects::Effect;
use commands::sync::DirectoryUpdate;
use commands::ui::{UiCommand, ViewportChanged};
use commands::view::AppViewState;
use composition::Backend;
use error::Result;
use ports::output::AppOutputPort;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing_subscriber::EnvFilter;

const ASSERT_DEMO_ENV: &str = "U2DM_ASSERT_DEMO";
const LOG_FORMAT_ENV: &str = "U2DM_LOG_FORMAT";
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
mod locale;
mod ports;
mod util;

fn main() -> ExitCode {
    init_tracing();
    if let Some(code) = demo_scenarios_cli() {
        return code;
    }
    if demo_was_required_but_missing() {
        return ExitCode::FAILURE;
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn demo_was_required_but_missing() -> bool {
    if !env::var(ASSERT_DEMO_ENV).is_ok_and(|value| value == "1") {
        return false;
    }
    if cfg!(feature = "demo") {
        return false;
    }
    tracing::error!(
        "{ASSERT_DEMO_ENV}=1 was set, but this binary was built without --features demo; \
         refusing to start against a real account"
    );
    true
}

fn init_tracing() {
    let filter = EnvFilter::from_default_env();
    if env::var(LOG_FORMAT_ENV).is_ok_and(|format| format == "json") {
        drop(
            tracing_subscriber::fmt()
                .json()
                .with_current_span(false)
                .with_span_list(false)
                .with_env_filter(filter)
                .try_init(),
        );
    } else {
        drop(
            tracing_subscriber::fmt()
                .with_ansi(io::stdout().is_terminal())
                .with_env_filter(filter)
                .try_init(),
        );
    }
}

fn run() -> Result<()> {
    let catalog_dir = locale::catalog_dir();
    tracing::debug!(dir = %catalog_dir.display(), "loading translation catalogs");
    slint::init_translations!(catalog_dir);
    let rt = Runtime::new()?;
    let cfg = config::AppConfig::from_env()?;
    tracing::info!(data_dir = %cfg.data_dir.display(), cache_dir = %cfg.cache_dir.display(), "starting U2DM");
    rt.block_on(async {
        private_fs::restrict_existing(&cfg.data_dir).await;
        private_fs::restrict_existing(&cfg.cache_dir).await;
    });
    let ui = SlintUiAdapter::compile(&rt)?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<UiCommand>();
    let (ui_tx, ui_rx) = mpsc::channel::<Effect>(UI_EVENT_CHANNEL_CAP);

    let (view_out_tx, view_out_rx) =
        watch::channel::<Arc<AppViewState>>(Arc::new(AppViewState::default()));

    let (dir_in_tx, dir_in_rx) = mpsc::unbounded_channel::<DirectoryUpdate>();
    let (scroll_tx, scroll_rx) = watch::channel::<ViewportChanged>(ViewportChanged::initial());

    ui.register_callbacks(&cmd_tx, &scroll_tx)?;
    size_window_for_demo(&ui);

    let enter_guard = rt.enter();
    let backend = Backend::select(&cfg);
    let media_files = Arc::clone(&backend.media_files);
    let browser = Arc::clone(&backend.browser);
    let probe_view_rx = view_out_tx.subscribe();
    let output: Arc<dyn AppOutputPort> = Arc::new(UiEventOutput::new(ui_tx, view_out_tx));
    let output = attach_probe(output, &ui, probe_view_rx, &cmd_tx);

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

    let ui_result = ui.run();
    drop(enter_guard);

    shutdown(rt, &cmd_tx_quit, service_handle);
    ui_result
}

#[cfg(feature = "demo")]
fn demo_scenarios_cli() -> Option<ExitCode> {
    demo::catalog::handle_cli()
}

#[cfg(not(feature = "demo"))]
fn demo_scenarios_cli() -> Option<ExitCode> {
    None
}

#[cfg(feature = "demo")]
fn size_window_for_demo(ui: &SlintUiAdapter) {
    demo::size_window_for_screenshots(ui);
}

#[cfg(not(feature = "demo"))]
fn size_window_for_demo(_ui: &SlintUiAdapter) {}

#[cfg(feature = "demo")]
fn attach_probe(
    output: Arc<dyn AppOutputPort>,
    ui: &SlintUiAdapter,
    view_rx: watch::Receiver<Arc<AppViewState>>,
    cmd_tx: &mpsc::UnboundedSender<UiCommand>,
) -> Arc<dyn AppOutputPort> {
    let (output, probe) = demo::probe::wrap_output(output);
    if let Some(probe) = probe {
        ui.install_timeline_dump();
        demo::probe::spawn(probe, view_rx, cmd_tx.clone());
    }
    output
}

#[cfg(not(feature = "demo"))]
fn attach_probe(
    output: Arc<dyn AppOutputPort>,
    _ui: &SlintUiAdapter,
    _view_rx: watch::Receiver<Arc<AppViewState>>,
    _cmd_tx: &mpsc::UnboundedSender<UiCommand>,
) -> Arc<dyn AppOutputPort> {
    output
}

fn shutdown(
    rt: Runtime,
    cmd_tx_quit: &mpsc::UnboundedSender<UiCommand>,
    service_handle: JoinHandle<()>,
) {
    if let Err(e) = cmd_tx_quit.send(UiCommand::Quit) {
        tracing::debug!("failed to send Quit command: {e}");
    }
    match rt.block_on(async { timeout(SHUTDOWN_WAIT, service_handle).await }) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::error!("the service task ended abnormally: {e}"),
        Err(_) => {
            tracing::warn!("service cleanup did not finish before deadline; forcing shutdown");
        }
    }
    rt.shutdown_timeout(SHUTDOWN_BACKSTOP);
}
