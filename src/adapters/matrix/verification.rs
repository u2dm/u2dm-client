use std::future::ready;
use std::pin::pin;
use std::time::Duration;

use futures_util::{Stream, StreamExt, stream};
use matrix_sdk::Client;
use matrix_sdk::encryption::verification::{
    CancelInfo, Emoji, EmojiShortAuthString, SasState, SasVerification, VerificationRequest,
    VerificationRequestState,
};
use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::ruma::events::key::verification::cancel::CancelCode;
use matrix_sdk::ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEvent;
use matrix_sdk::ruma::events::room::message::{MessageType, OriginalSyncRoomMessageEvent};
use tokio::sync::{Mutex, mpsc};
use tokio::time::timeout;

use crate::domain::models::{VerificationCancellation, VerificationEmoji, VerificationEvent};
use crate::error::{AppError, Result};

const VERIFICATION_QUEUE: usize = 8;
const UNANSWERED_REQUEST_TIMEOUT: Duration = Duration::from_mins(10);

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

        if is_settled(&request.state()) {
            tracing::info!(%sender, %flow_id, "ignoring a verification request that is already over");
            continue;
        }

        tracing::info!(%sender, is_self, %flow_id, "verification request received");
        *verification_request.lock().await = Some(request.clone());

        verification_tx
            .send(VerificationEvent::Requested { sender, is_self })
            .ok();

        run_verification(
            &request,
            &flow_id,
            &mut rx,
            sas_verification,
            &verification_tx,
        )
        .await;

        *verification_request.lock().await = None;
        *sas_verification.lock().await = None;
    }

    Ok(())
}

fn is_settled(state: &VerificationRequestState) -> bool {
    matches!(
        state,
        VerificationRequestState::Cancelled(_) | VerificationRequestState::Done
    )
}

fn current_then_changes<T: 'static>(
    current: T,
    changes: impl Stream<Item = T>,
) -> impl Stream<Item = T> {
    stream::once(ready(current)).chain(changes)
}

fn request_states(request: &VerificationRequest) -> impl Stream<Item = VerificationRequestState> {
    let changes_from_now_on = request.changes();
    current_then_changes(request.state(), changes_from_now_on)
}

fn sas_states(sas: &SasVerification) -> impl Stream<Item = SasState> {
    let changes_from_now_on = sas.changes();
    current_then_changes(sas.state(), changes_from_now_on)
}

fn send_cancelled(tx: &mpsc::UnboundedSender<VerificationEvent>, reason: VerificationCancellation) {
    tx.send(VerificationEvent::Cancelled(reason)).ok();
}

async fn run_verification(
    request: &VerificationRequest,
    flow_id: &str,
    rx: &mut mpsc::Receiver<VerificationRequest>,
    sas_verification: &Mutex<Option<SasVerification>>,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) {
    let flow = drive_flow(request, sas_verification, tx);
    tokio::pin!(flow);

    let mut channel_open = true;
    loop {
        tokio::select! {
            biased;
            () = &mut flow => return,
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

async fn drive_flow(
    request: &VerificationRequest,
    sas_verification: &Mutex<Option<SasVerification>>,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) {
    let mut states = pin!(request_states(request));

    let Ok(arrival) = timeout(UNANSWERED_REQUEST_TIMEOUT, await_sas(&mut states, request)).await
    else {
        tracing::info!("nobody answered the verification request; cancelling it");
        request.cancel().await.ok();
        send_cancelled(tx, VerificationCancellation::TimedOut);
        return;
    };

    let end = match arrival {
        SasArrival::Sas(sas) => follow_sas(sas, &mut states, request, sas_verification, tx).await,
        SasArrival::Ended(end) => end,
    };

    match end {
        FlowEnd::Done => {
            tx.send(VerificationEvent::Done).ok();
        }
        FlowEnd::Cancelled(reason) => send_cancelled(tx, reason),
    }
}

enum FlowEnd {
    Done,
    Cancelled(VerificationCancellation),
}

enum SasArrival {
    Sas(SasVerification),
    Ended(FlowEnd),
}

async fn await_sas(
    states: &mut (impl Stream<Item = VerificationRequestState> + Unpin),
    request: &VerificationRequest,
) -> SasArrival {
    while let Some(state) = states.next().await {
        if let Some(arrival) = request_step(request, state).await {
            return arrival;
        }
    }

    SasArrival::Ended(request_states_ended())
}

async fn request_step(
    request: &VerificationRequest,
    state: VerificationRequestState,
) -> Option<SasArrival> {
    match state {
        VerificationRequestState::Transitioned { verification } => {
            let Some(sas) = verification.sas() else {
                tracing::warn!("the verification chose a method U2DM cannot drive; cancelling");
                request.cancel().await.ok();
                return None;
            };
            tracing::info!("the verification transitioned to SAS");
            Some(SasArrival::Sas(sas))
        }
        VerificationRequestState::Ready { their_methods, .. } => {
            tracing::debug!(
                ?their_methods,
                "the request is ready; waiting for the other device to start the SAS"
            );
            None
        }
        VerificationRequestState::Done => Some(SasArrival::Ended(FlowEnd::Done)),
        VerificationRequestState::Cancelled(info) => {
            report_cancellation("verification request", &info);
            Some(SasArrival::Ended(FlowEnd::Cancelled(cancellation(&info))))
        }
        VerificationRequestState::Created { .. } | VerificationRequestState::Requested { .. } => {
            None
        }
    }
}

async fn follow_sas(
    first: SasVerification,
    states: &mut (impl Stream<Item = VerificationRequestState> + Unpin),
    request: &VerificationRequest,
    sas_verification: &Mutex<Option<SasVerification>>,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) -> FlowEnd {
    let mut sas = first;
    loop {
        *sas_verification.lock().await = Some(sas.clone());
        match race_sas_against_request(&sas, states, tx).await {
            SasRace::Replaced(next) => {
                tracing::info!("the verification switched to another SAS; following that one");
                sas = next;
            }
            SasRace::Ended(FlowEnd::Cancelled(reason)) => {
                return if request_completed(request) {
                    tracing::info!(
                        "the SAS reported a cancellation but the request completed; counting it verified"
                    );
                    FlowEnd::Done
                } else {
                    FlowEnd::Cancelled(reason)
                };
            }
            SasRace::Ended(end) => return end,
        }
    }
}

fn request_completed(request: &VerificationRequest) -> bool {
    matches!(request.state(), VerificationRequestState::Done)
}

enum SasRace {
    Ended(FlowEnd),
    Replaced(SasVerification),
}

async fn race_sas_against_request(
    sas: &SasVerification,
    states: &mut (impl Stream<Item = VerificationRequestState> + Unpin),
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) -> SasRace {
    let driving = drive_sas(sas, tx);
    tokio::pin!(driving);

    loop {
        tokio::select! {
            end = &mut driving => return SasRace::Ended(end),
            state = states.next() => {
                match state {
                    Some(VerificationRequestState::Transitioned { verification }) => {
                        if let Some(next) = verification.sas() {
                            return SasRace::Replaced(next);
                        }
                    }
                    Some(VerificationRequestState::Done) => {
                        tracing::info!("the request reports the verification is done");
                        return SasRace::Ended(FlowEnd::Done);
                    }
                    Some(VerificationRequestState::Cancelled(info)) => {
                        report_cancellation("verification request", &info);
                        return SasRace::Ended(FlowEnd::Cancelled(cancellation(&info)));
                    }
                    Some(_) => {}
                    None => return SasRace::Ended(request_states_ended()),
                }
            }
        }
    }
}

fn request_states_ended() -> FlowEnd {
    tracing::warn!("the verification request stopped reporting its state");
    FlowEnd::Cancelled(VerificationCancellation::Failed)
}

async fn drive_sas(
    sas: &SasVerification,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) -> FlowEnd {
    let mut states = pin!(sas_states(sas));
    let mut announced = AnnouncedSteps::default();

    while let Some(state) = states.next().await {
        tracing::debug!(state = sas_state_name(&state), "the SAS changed state");
        if let Some(end) = step_sas(sas, state, &mut announced, tx).await {
            return end;
        }
    }

    tracing::warn!("the SAS stopped reporting its state");
    FlowEnd::Cancelled(VerificationCancellation::Failed)
}

fn sas_state_name(state: &SasState) -> &'static str {
    match state {
        SasState::Created { .. } => "created",
        SasState::Started { .. } => "started",
        SasState::Accepted { .. } => "accepted",
        SasState::KeysExchanged { .. } => "keys-exchanged",
        SasState::Confirmed => "confirmed",
        SasState::Done { .. } => "done",
        SasState::Cancelled(_) => "cancelled",
    }
}

#[derive(Default)]
struct AnnouncedSteps {
    emojis: bool,
    confirming: bool,
}

async fn step_sas(
    sas: &SasVerification,
    state: SasState,
    announced: &mut AnnouncedSteps,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) -> Option<FlowEnd> {
    match state {
        SasState::Started { .. } => match sas.accept().await {
            Ok(()) => None,
            Err(e) => {
                tracing::warn!("failed to accept the SAS verification: {e}");
                sas.cancel().await.ok();
                Some(FlowEnd::Cancelled(VerificationCancellation::AcceptFailed))
            }
        },
        SasState::KeysExchanged { emojis, decimals } => {
            announce_emojis(sas, emojis.as_ref(), decimals, announced, tx).await;
            None
        }
        SasState::Confirmed => {
            if !announced.confirming {
                announced.confirming = true;
                tx.send(VerificationEvent::Confirming).ok();
            }
            None
        }
        SasState::Done { .. } => Some(FlowEnd::Done),
        SasState::Cancelled(info) => {
            report_cancellation("SAS verification", &info);
            Some(FlowEnd::Cancelled(cancellation(&info)))
        }
        SasState::Created { .. } | SasState::Accepted { .. } => None,
    }
}

async fn announce_emojis(
    sas: &SasVerification,
    emojis: Option<&EmojiShortAuthString>,
    decimals: (u16, u16, u16),
    announced: &mut AnnouncedSteps,
    tx: &mpsc::UnboundedSender<VerificationEvent>,
) {
    let Some(emojis) = emojis else {
        tracing::warn!(
            ?decimals,
            "the SAS agreed on decimals, which U2DM cannot show"
        );
        sas.cancel().await.ok();
        return;
    };
    if !announced.emojis {
        announced.emojis = true;
        tx.send(VerificationEvent::Emojis(domain_emojis(&emojis.emojis)))
            .ok();
    }
}

fn domain_emojis(emojis: &[Emoji; 7]) -> Vec<VerificationEmoji> {
    emojis
        .iter()
        .map(|e| VerificationEmoji {
            symbol: e.symbol.to_owned(),
            description: e.description.to_owned(),
        })
        .collect()
}

fn report_cancellation(what: &str, info: &CancelInfo) {
    tracing::info!(
        by_us = info.cancelled_by_us(),
        code = ?info.cancel_code(),
        reason = info.reason(),
        "{what} cancelled"
    );
}

fn cancellation(info: &CancelInfo) -> VerificationCancellation {
    match info.cancel_code() {
        CancelCode::Timeout => VerificationCancellation::TimedOut,
        CancelCode::Accepted => VerificationCancellation::AcceptedElsewhere,
        CancelCode::MismatchedSas | CancelCode::KeyMismatch => VerificationCancellation::Mismatch,
        CancelCode::User if info.cancelled_by_us() => VerificationCancellation::Declined,
        CancelCode::User => VerificationCancellation::Remote,
        _ => VerificationCancellation::Failed,
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
    let sas = sas_verification.lock().await.clone();
    if let Some(sas) = sas {
        sas.mismatch().await?;
        return Ok(());
    }
    let request = verification_request.lock().await.clone();
    let request =
        request.ok_or_else(|| AppError::Other("No pending verification request".into()))?;
    request.cancel().await?;
    Ok(())
}
