use futures_util::StreamExt;
use matrix_sdk::Client;
use matrix_sdk::encryption::verification::{
    SasState, SasVerification, Verification, VerificationRequest, VerificationRequestState,
};
use matrix_sdk::ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEvent;
use matrix_sdk::ruma::events::room::message::{MessageType, OriginalSyncRoomMessageEvent};
use tokio::sync::{Mutex, mpsc};

use crate::domain::models::{VerificationEmoji, VerificationEvent};
use crate::error::{AppError, Result};

pub(super) async fn ensure_verification_handlers(
    client: &Client,
    verification_req_rx: &Mutex<Option<mpsc::UnboundedReceiver<VerificationRequest>>>,
) -> Result<()> {
    if verification_req_rx.lock().await.is_some() {
        return Ok(());
    }

    let (req_tx, rx) = mpsc::unbounded_channel::<VerificationRequest>();

    client.add_event_handler({
        let req_tx = req_tx.clone();
        move |ev: ToDeviceKeyVerificationRequestEvent, client: Client| {
            let req_tx = req_tx.clone();
            async move {
                if let Some(request) = client
                    .encryption()
                    .get_verification_request(&ev.sender, &ev.content.transaction_id)
                    .await
                {
                    req_tx.send(request).ok();
                }
            }
        }
    });

    client.add_event_handler({
        let req_tx = req_tx.clone();
        move |ev: OriginalSyncRoomMessageEvent, client: Client| {
            let req_tx = req_tx.clone();
            async move {
                if let MessageType::VerificationRequest(_) = &ev.content.msgtype
                    && let Some(request) = client
                        .encryption()
                        .get_verification_request(&ev.sender, &ev.event_id)
                        .await
                {
                    req_tx.send(request).ok();
                }
            }
        }
    });

    *verification_req_rx.lock().await = Some(rx);
    Ok(())
}

pub(super) async fn listen_for_verification(
    client: &Client,
    verification_req_rx: &Mutex<Option<mpsc::UnboundedReceiver<VerificationRequest>>>,
    verification_request: &Mutex<Option<VerificationRequest>>,
    sas_verification: &Mutex<Option<SasVerification>>,
    verification_tx: mpsc::UnboundedSender<VerificationEvent>,
) -> Result<()> {
    ensure_verification_handlers(client, verification_req_rx).await?;

    let mut rx_guard = verification_req_rx.lock().await;
    let rx = rx_guard
        .as_mut()
        .ok_or_else(|| AppError::Other("verification channel not initialized".into()))?;

    while let Some(request) = rx.recv().await {
        *verification_request.lock().await = Some(request.clone());

        verification_tx
            .send(VerificationEvent::Requested {
                sender: request.other_user_id().to_string(),
                is_self: request.is_self_verification(),
            })
            .ok();

        handle_verification_request(request, sas_verification, &verification_tx).await;

        *verification_request.lock().await = None;
        *sas_verification.lock().await = None;
    }

    Ok(())
}

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
                    *sas_mutex.lock().await = Some(sas.clone());
                    handle_sas_verification(sas, tx).await;
                }
                break;
            }
            VerificationRequestState::Done => {
                tx.send(VerificationEvent::Done).ok();
                break;
            }
            VerificationRequestState::Cancelled(info) => {
                tx.send(VerificationEvent::Cancelled(info.reason().to_string()))
                    .ok();
                break;
            }
            _ => {}
        }
    }
}

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
                tx.send(VerificationEvent::Confirming).ok();
            }
            SasState::Done { .. } => {
                tx.send(VerificationEvent::Done).ok();
                break;
            }
            SasState::Cancelled(info) => {
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
    let guard = verification_request.lock().await;
    let request = guard
        .as_ref()
        .ok_or_else(|| AppError::Other("No pending verification request".into()))?;
    request.accept().await?;
    Ok(())
}

pub(super) async fn confirm_verification(
    sas_verification: &Mutex<Option<SasVerification>>,
) -> Result<()> {
    let guard = sas_verification.lock().await;
    let sas = guard
        .as_ref()
        .ok_or_else(|| AppError::Other("No active SAS verification".into()))?;
    sas.confirm().await?;
    Ok(())
}

pub(super) async fn reject_verification(
    sas_verification: &Mutex<Option<SasVerification>>,
    verification_request: &Mutex<Option<VerificationRequest>>,
) -> Result<()> {
    if let Some(sas) = sas_verification.lock().await.take() {
        sas.mismatch().await?;
    } else if let Some(request) = verification_request.lock().await.take() {
        request.cancel().await?;
    }
    Ok(())
}
