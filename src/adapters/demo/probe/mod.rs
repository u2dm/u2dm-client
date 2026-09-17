mod command;
mod dto;
mod journal;
mod names;
mod output;

use std::env;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::time::timeout;

use self::command::{Driven, ProbeCommand};
use self::journal::{Journal, Selected};
use self::output::ProbeOutput;
use super::{catalog, data};
use crate::adapters::ui::dump;
use crate::app::input::CommandSender;
use crate::commands::view::AppViewState;
use crate::ports::output::AppOutputPort;

const PORT_ENV: &str = "U2DM_PROBE_PORT";
const DEFAULT_JOURNAL_LIMIT: usize = 256;
const MAX_WAIT: Duration = Duration::from_secs(30);
const DUMP_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Probe {
    journal: Arc<Journal>,
    selected: Arc<Selected>,
}

pub fn wrap_output(inner: Arc<dyn AppOutputPort>) -> (Arc<dyn AppOutputPort>, Option<Probe>) {
    if port().is_none() {
        return (inner, None);
    }
    let journal = Arc::new(Journal::new());
    let selected = Arc::new(Selected::new());
    let wrapped = Arc::new(ProbeOutput::new(
        inner,
        Arc::clone(&journal),
        Arc::clone(&selected),
    ));
    (wrapped, Some(Probe { journal, selected }))
}

#[derive(Clone)]
struct Server {
    journal: Arc<Journal>,
    selected: Arc<Selected>,
    view: watch::Receiver<Arc<AppViewState>>,
    revision: Arc<AtomicU64>,
    commands: CommandSender,
}

pub fn spawn(probe: Probe, view: watch::Receiver<Arc<AppViewState>>, commands: CommandSender) {
    let Some(port) = port() else {
        return;
    };
    let revision = Arc::new(AtomicU64::new(0));
    let mut counting = view.clone();
    let counter = Arc::clone(&revision);
    tokio::spawn(async move {
        while counting.changed().await.is_ok() {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    });
    let server = Server {
        journal: probe.journal,
        selected: probe.selected,
        view,
        revision,
        commands,
    };
    tokio::spawn(async move { serve(port, server).await });
}

async fn bind(address: SocketAddr) -> Option<TcpListener> {
    match TcpListener::bind(address).await {
        Ok(listener) => Some(listener),
        Err(e) => {
            tracing::error!("the demo probe could not bind {address}: {e}");
            None
        }
    }
}

async fn serve(port: u16, server: Server) {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Some(listener) = bind(address).await else {
        return;
    };
    tracing::warn!("the demo probe is listening on http://{address}; it can inject commands");
    if let Err(e) = axum::serve(listener, router(server)).await {
        tracing::error!("the demo probe stopped serving: {e}");
    }
}

fn router(server: Server) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/state", get(state))
        .route("/timeline", get(timeline))
        .route("/journal", get(read_journal))
        .route("/scenarios", get(scenarios))
        .route("/command", post(run_command))
        .with_state(server)
}

fn port() -> Option<u16> {
    let raw = env::var(PORT_ENV).ok()?;
    match raw.parse() {
        Ok(port) => Some(port),
        Err(e) => {
            tracing::error!("{PORT_ENV} is not a port number ({raw}): {e}");
            None
        }
    }
}

fn failure(status: StatusCode, reason: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": reason.into() })))
}

async fn health(State(server): State<Server>) -> impl IntoResponse {
    let (rooms, spaces, timelines) = data::counts();
    Json(json!({
        "ok": true,
        "demo": true,
        "data_path": data::source_path(),
        "data_error": data::load_error(),
        "fixture": { "rooms": rooms, "spaces": spaces, "timelines": timelines },
        "selected": server.selected.get().map(|(id, generation)| json!({
            "room_id": id,
            "generation": generation,
        })),
    }))
}

#[derive(Deserialize)]
struct StateQuery {
    wait: Option<u64>,
    ms: Option<u64>,
}

async fn state(
    State(mut server): State<Server>,
    Query(query): Query<StateQuery>,
) -> impl IntoResponse {
    if let Some(seen) = query.wait {
        let budget = query
            .ms
            .map_or(MAX_WAIT, |ms| Duration::from_millis(ms).min(MAX_WAIT));
        drop(server.view.borrow_and_update());
        if server.revision.load(Ordering::Relaxed) <= seen {
            drop(timeout(budget, server.view.changed()).await);
        }
    }
    let snapshot = server.view.borrow_and_update().clone();
    Json(json!({
        "seq": server.revision.load(Ordering::Relaxed),
        "view": dto::view(&snapshot),
    }))
}

async fn timeline() -> impl IntoResponse {
    let Some(pending) = dump::request() else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "no timeline dump is installed; this build is interpreted, or no window is running",
        );
    };
    match timeout(DUMP_TIMEOUT, pending).await {
        Ok(Ok(dump)) => (StatusCode::OK, Json(json!(dump))),
        Ok(Err(_)) => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "the window went away before it answered",
        ),
        Err(_) => failure(
            StatusCode::GATEWAY_TIMEOUT,
            "the Slint event loop did not answer within 2s; it is probably blocked",
        ),
    }
}

#[derive(Deserialize)]
struct JournalQuery {
    since: Option<u64>,
    limit: Option<usize>,
}

async fn read_journal(
    State(server): State<Server>,
    Query(query): Query<JournalQuery>,
) -> impl IntoResponse {
    Json(server.journal.page(
        query.since.unwrap_or_default(),
        query.limit.unwrap_or(DEFAULT_JOURNAL_LIMIT),
    ))
}

async fn scenarios() -> impl IntoResponse {
    Json(catalog::json())
}

async fn run_command(
    State(server): State<Server>,
    Json(request): Json<ProbeCommand>,
) -> impl IntoResponse {
    let driven = match command::to_driven(request, server.selected.get().as_ref()) {
        Ok(driven) => driven,
        Err(command::Rejected(reason)) => return failure(StatusCode::CONFLICT, reason),
    };
    let label = driven.to_string();
    server.journal.record_command(label.clone());
    let delivered = match driven {
        Driven::Command(cmd) => server.commands.send(cmd),
        Driven::SessionExpiry => server.commands.inject_session_expiry(),
        Driven::Poke(poke) => {
            if !dump::poke(poke) {
                return failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "this build has no UI poke hook (interpreted mode)".to_owned(),
                );
            }
            Ok(())
        }
    };
    if let Err(e) = delivered {
        return failure(StatusCode::SERVICE_UNAVAILABLE, e.to_string());
    }
    (
        StatusCode::OK,
        Json(json!({ "accepted": true, "command": label })),
    )
}
