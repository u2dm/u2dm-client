use std::time::Duration;

use futures_util::StreamExt;
use matrix_sdk::Client;
use matrix_sdk::encryption::verification::{
    SasState, SasVerification, Verification, VerificationRequest, VerificationRequestState,
};
use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEvent;
use matrix_sdk::ruma::events::room::message::{MessageType, OriginalSyncRoomMessageEvent};
use tokio::sync::{Mutex, mpsc};
use tokio::time::timeout;

use crate::domain::models::{VerificationEmoji, VerificationEvent};
use crate::error::{AppError, Result};

const VERIFICATION_QUEUE: usize = 8;
const VERIFICATION_TIMEOUT: Duration = Duration::from_secs(300);

async fn enqueue_or_reject(tx: &mpsc::Sender<VerificationRequest>, request: VerificationRequest) {
    if let Err(mpsc::error::TrySendError::Full(request)) = tx.try_send(request) {
        tracing::warn!("verification request queue full; rejecting incoming request");
        request.cancel().await.ok();
    }
}

fn setup_verification_handlers(
    client: &Client,
    verification_req_rx: &mut Option<mpsc::Receiver<VerificationRequest>>,
    handler_guards: &mut Vec<EventHandlerDropGuard>,
) {
    handler_guards.clear();
    *verification_req_rx = None;

    let (req_tx, rx) = mpsc::channel::<VerificationRequest>(VERIFICATION_QUEUE);

    let to_device_handle = client.add_event_handler({
        let req_tx = req_tx.clone();
        move |ev: ToDeviceKeyVerificationRequestEvent, client: Client| {
            let req_tx = req_tx.clone();
            async move {
                if let Some(request) = client
                    .encryption()
                    .get_verification_request(&ev.sender, &ev.content.transaction_id)
                    .await
                {
                    enqueue_or_reject(&req_tx, request).await;
                }
            }
        }
    });

    let in_room_handle = client.add_event_handler({
        move |ev: OriginalSyncRoomMessageEvent, client: Client| {
            let req_tx = req_tx.clone();
            async move {
                if let MessageType::VerificationRequest(_) = &ev.content.msgtype
                    && let Some(request) = client
                        .encryption()
                        .get_verification_request(&ev.sender, &ev.event_id)
                        .await
                {
                    enqueue_or_reject(&req_tx, request).await;
                }
            }
        }
    });

    handler_guards.push(client.event_handler_drop_guard(to_device_handle));
    handler_guards.push(client.event_handler_drop_guard(in_room_handle));
    *verification_req_rx = Some(rx);
}

pub(super) async fn listen_for_verification(
    client: &Client,
    verification_req_rx: &Mutex<Option<mpsc::Receiver<VerificationRequest>>>,
    handler_guards: &Mutex<Vec<EventHandlerDropGuard>>,
    verification_request: &Mutex<Option<VerificationRequest>>,
    sas_verification: &Mutex<Option<SasVerification>>,
    verification_tx: mpsc::UnboundedSender<VerificationEvent>,
) -> Result<()> {
    let mut rx_guard = verification_req_rx.lock().await;
    let mut guards = handler_guards.lock().await;
    setup_verification_handlers(client, &mut rx_guard, &mut guards);
    drop(guards);

    let mut rx = rx_guard
        .take()
        .ok_or_else(|| AppError::Other("verification channel not initialized".into()))?;
    drop(rx_guard);

    while let Some(request) = rx.recv().await {
        let sender = request.other_user_id().to_string();
        let is_self = request.is_self_verification();
        let flow_id = request.flow_id().to_string();
        tracing::info!(sender = %sender, is_self, "verification request received");
        *verification_request.lock().await = Some(request.clone());

        verification_tx
            .send(VerificationEvent::Requested { sender, is_self })
            .ok();

        run_verification(
            request,
            &flow_id,
            &mut rx,
            sas_verification,
            verification_request,
            &verification_tx,
        )
        .await;

        *verification_request.lock().await = None;
        *sas_verification.lock().await = None;
    }

    Ok(())
}

async fn run_verification(
    request: VerificationRequest,
    flow_id: &str,
    rx: &mut mpsc::Receiver<VerificationRequest>,
    sas_verification: &Mutex<Option<SasVerification>>,
    verification_request: &Mutex<Option<VerificationRequest>>,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) {
    let handle = timeout(
        VERIFICATION_TIMEOUT,
        handle_verification_request(request, sas_verification, tx),
    );
    tokio::pin!(handle);

    let mut channel_open = true;
    loop {
        tokio::select! {
            biased;
            result = &mut handle => {
                if result.is_err() {
                    tracing::warn!("verification timed out; cancelling");
                    cancel_active_verification(sas_verification, verification_request, tx).await;
                }
                break;
            }
            incoming = rx.recv(), if channel_open => {
                match incoming {
                    Some(other) if other.flow_id() != flow_id => {
                        tracing::info!("busy with a verification; rejecting incoming request");
                        other.cancel().await.ok();
                    }
                    Some(_) => {}
                    None => channel_open = false,
                }
            }
        }
    }
}

async fn cancel_active_verification(
    sas_verification: &Mutex<Option<SasVerification>>,
    verification_request: &Mutex<Option<VerificationRequest>>,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) {
    let sas = sas_verification.lock().await.take();
    let request = verification_request.lock().await.take();
    if let Some(sas) = sas {
        sas.cancel().await.ok();
    }
    if let Some(request) = request {
        request.cancel().await.ok();
    }
    tx.send(VerificationEvent::Cancelled(
        "Verification timed out".to_owned(),
    ))
    .ok();
}

#[allow(clippy::cognitive_complexity)]
async fn handle_verification_request(
    request: VerificationRequest,
    sas_mutex: &Mutex<Option<SasVerification>>,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) {
    let mut stream = request.changes();

    while let Some(state) = stream.next().await {
        match state {
            VerificationRequestState::Transitioned { verification } => {
                if let Verification::SasV1(sas) = verification {
                    tracing::info!("verification transitioned to SAS");
                    *sas_mutex.lock().await = Some(sas.clone());
                    handle_sas_verification(sas, tx).await;
                }
                break;
            }
            VerificationRequestState::Done => {
                tracing::info!("verification completed");
                tx.send(VerificationEvent::Done).ok();
                break;
            }
            VerificationRequestState::Cancelled(info) => {
                tracing::info!(reason = %info.reason(), "verification cancelled");
                tx.send(VerificationEvent::Cancelled(info.reason().to_string()))
                    .ok();
                break;
            }
            _ => {}
        }
    }
}

#[allow(clippy::cognitive_complexity)]
async fn handle_sas_verification(
    sas: SasVerification,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) {
    if let Err(e) = sas.accept().await {
        tx.send(VerificationEvent::Cancelled(format!(
            "Failed to accept SAS: {e}"
        )))
        .ok();
        return;
    }

    let mut stream = sas.changes();

    while let Some(state) = stream.next().await {
        match state {
            SasState::KeysExchanged { .. } => {
                tracing::debug!("SAS keys exchanged, presenting emojis");
                if let Some(emojis) = sas.emoji() {
                    let domain_emojis: Vec<VerificationEmoji> = emojis
                        .iter()
                        .map(|e| VerificationEmoji {
                            symbol: e.symbol.to_string(),
                            description: e.description.to_string(),
                        })
                        .collect();
                    tx.send(VerificationEvent::Emojis(domain_emojis)).ok();
                }
            }
            SasState::Confirmed => {
                tracing::debug!("SAS confirmed, waiting for other side");
                tx.send(VerificationEvent::Confirming).ok();
            }
            SasState::Done { .. } => {
                tracing::info!("SAS verification done");
                tx.send(VerificationEvent::Done).ok();
                break;
            }
            SasState::Cancelled(info) => {
                tracing::info!(reason = %info.reason(), "SAS verification cancelled");
                tx.send(VerificationEvent::Cancelled(info.reason().to_string()))
                    .ok();
                break;
            }
            _ => {}
        }
    }
}

pub(super) async fn accept_verification(
    verification_request: &Mutex<Option<VerificationRequest>>,
) -> Result<()> {
    let request = {
        let guard = verification_request.lock().await;
        guard
            .clone()
            .ok_or_else(|| AppError::Other("No pending verification request".into()))?
    };
    request.accept().await?;
    Ok(())
}

pub(super) async fn confirm_verification(
    sas_verification: &Mutex<Option<SasVerification>>,
) -> Result<()> {
    let sas = {
        let guard = sas_verification.lock().await;
        guard
            .clone()
            .ok_or_else(|| AppError::Other("No active SAS verification".into()))?
    };
    sas.confirm().await?;
    Ok(())
}

pub(super) async fn reject_verification(
    sas_verification: &Mutex<Option<SasVerification>>,
    verification_request: &Mutex<Option<VerificationRequest>>,
) -> Result<()> {
    let sas = sas_verification.lock().await.take();
    let request = if sas.is_none() {
        verification_request.lock().await.take()
    } else {
        None
    };
    if let Some(sas) = sas {
        sas.mismatch().await?;
    } else if let Some(request) = request {
        request.cancel().await?;
    }
    Ok(())
}
