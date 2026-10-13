use std::future::ready;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{Stream, StreamExt, stream};
use matrix_sdk::Client;
use matrix_sdk::encryption::verification::{
    CancelInfo, Emoji, EmojiShortAuthString, SasState, SasVerification, Verification,
    VerificationRequest, VerificationRequestState,
};
use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::ruma::events::key::verification::VerificationMethod;
use matrix_sdk::ruma::events::key::verification::cancel::CancelCode;
use matrix_sdk::ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEvent;
use matrix_sdk::ruma::events::room::message::{MessageType, OriginalSyncRoomMessageEvent};
use tokio::sync::{Mutex, mpsc};
use tokio::time::timeout;

use super::session::{ClientHandle, SessionResource};
use super::user_info::own_user;
use crate::domain::verification::{VerificationCancellation, VerificationEmoji, VerificationEvent};
use crate::error::{AppError, Result};
use crate::ports::matrix::VerificationPort;

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
    outgoing: &mut Option<mpsc::Sender<VerificationRequest>>,
) {
    handler_guards.clear();
    *verification_req_rx = None;

    let (req_tx, rx) = mpsc::channel::<VerificationRequest>(VERIFICATION_QUEUE);
    *outgoing = Some(req_tx.clone());

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

async fn listen_for_verification(
    client: &Client,
    verification_req_rx: &Mutex<Option<mpsc::Receiver<VerificationRequest>>>,
    handler_guards: &Mutex<Vec<EventHandlerDropGuard>>,
    outgoing: &Mutex<Option<mpsc::Sender<VerificationRequest>>>,
    verification_request: &Mutex<Option<VerificationRequest>>,
    sas_verification: &Mutex<Option<SasVerification>>,
    verification_tx: mpsc::UnboundedSender<VerificationEvent>,
) -> Result<()> {
    let mut rx_guard = verification_req_rx.lock().await;
    let mut guards = handler_guards.lock().await;
    let mut sender = outgoing.lock().await;
    setup_verification_handlers(client, &mut rx_guard, &mut guards, &mut sender);
    drop(sender);
    drop(guards);

    let mut rx = rx_guard
        .take()
        .ok_or_else(|| AppError::Other("verification channel not initialized".into()))?;
    drop(rx_guard);

    while let Some(request) = rx.recv().await {
        let sender = request.other_user_id().to_string();
        let flow_id = request.flow_id().to_string();

        if is_settled(&request.state()) {
            tracing::info!(%sender, %flow_id, "ignoring a verification request that is already over");
            continue;
        }

        *verification_request.lock().await = Some(request.clone());

        verification_tx.send(announcement(&request, sender)).ok();

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

fn announcement(request: &VerificationRequest, sender: String) -> VerificationEvent {
    let flow_id = request.flow_id();
    if request.we_started() {
        tracing::info!(%flow_id, "asked the account's other sessions to verify this one");
        return VerificationEvent::RequestSent { own_user: sender };
    }
    let is_self = request.is_self_verification();
    tracing::info!(%sender, is_self, %flow_id, "verification request received");
    VerificationEvent::Requested { sender, is_self }
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
        FlowEnd::Cancelled(reason) => send_cancelled(tx, told_to_the_asker(reason, request)),
    }
}

fn told_to_the_asker(
    reason: VerificationCancellation,
    request: &VerificationRequest,
) -> VerificationCancellation {
    match reason {
        VerificationCancellation::Declined if request.we_started() => {
            VerificationCancellation::Withdrawn
        }
        reason => reason,
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
            transitioned(request, verification).await
        }
        VerificationRequestState::Ready { their_methods, .. } if request.we_started() => {
            tracing::debug!(
                ?their_methods,
                "the other session is ready; starting the SAS"
            );
            start_sas(request).await
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

async fn transitioned(
    request: &VerificationRequest,
    verification: Verification,
) -> Option<SasArrival> {
    let Some(sas) = verification.sas() else {
        tracing::warn!("the verification chose a method U2DM cannot drive; cancelling");
        request.cancel().await.ok();
        return None;
    };
    tracing::info!("the verification transitioned to SAS");
    Some(SasArrival::Sas(sas))
}

async fn start_sas(request: &VerificationRequest) -> Option<SasArrival> {
    if !matches!(request.state(), VerificationRequestState::Ready { .. }) {
        return None;
    }
    let failure = match request.start_sas().await {
        Ok(Some(_)) => return None,
        Ok(None) => "the other session cannot compare emoji".to_owned(),
        Err(e) => e.to_string(),
    };
    tracing::warn!("could not start the SAS: {failure}; cancelling");
    request.cancel().await.ok();
    Some(SasArrival::Ended(FlowEnd::Cancelled(
        VerificationCancellation::Failed,
    )))
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

async fn verify_this_session(
    client: &Client,
    verification_request: &Mutex<Option<VerificationRequest>>,
    outgoing: &Mutex<Option<mpsc::Sender<VerificationRequest>>>,
) -> Result<()> {
    if verification_request.lock().await.is_some() {
        return Err(AppError::Other("a verification is already running".into()));
    }
    let queue = outgoing
        .lock()
        .await
        .clone()
        .ok_or_else(|| AppError::Other("verification requests are not followed".into()))?;
    let encryption = client.encryption();
    let can_answer = encryption
        .has_devices_to_verify_against()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    if !can_answer {
        return Err(AppError::Other(
            "no other session of this account can verify this one".into(),
        ));
    }
    let identity = encryption
        .get_user_identity(&own_user(client)?)
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
        .ok_or_else(|| AppError::Other("the account has no identity to verify against".into()))?;
    let request = identity
        .request_verification_with_methods(vec![VerificationMethod::SasV1])
        .await
        .map_err(|e| AppError::Other(format!("could not send the verification request: {e}")))?;
    if let Err(refused) = queue.try_send(request) {
        refused.into_inner().cancel().await.ok();
        return Err(AppError::Other(
            "the verification request could not be followed".into(),
        ));
    }
    Ok(())
}

async fn accept_verification(
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

async fn confirm_verification(sas_verification: &Mutex<Option<SasVerification>>) -> Result<()> {
    let sas = {
        let guard = sas_verification.lock().await;
        guard
            .clone()
            .ok_or_else(|| AppError::Other("No active SAS verification".into()))?
    };
    sas.confirm().await?;
    Ok(())
}

async fn reject_verification(
    sas_verification: &Mutex<Option<SasVerification>>,
    verification_request: &Mutex<Option<VerificationRequest>>,
) -> Result<()> {
    let sas = sas_verification.lock().await.clone();
    if let Some(sas) = sas {
        sas.mismatch().await?;
        return Ok(());
    }
    cancel_verification(verification_request).await
}

async fn cancel_verification(
    verification_request: &Mutex<Option<VerificationRequest>>,
) -> Result<()> {
    let request = verification_request.lock().await.clone();
    let request =
        request.ok_or_else(|| AppError::Other("No pending verification request".into()))?;
    request.cancel().await?;
    Ok(())
}

pub(super) struct MatrixVerification {
    matrix: Arc<ClientHandle>,
    request: Mutex<Option<VerificationRequest>>,
    sas: Mutex<Option<SasVerification>>,
    incoming: Mutex<Option<mpsc::Receiver<VerificationRequest>>>,
    outgoing: Mutex<Option<mpsc::Sender<VerificationRequest>>>,
    handler_guards: Mutex<Vec<EventHandlerDropGuard>>,
}

impl MatrixVerification {
    pub(super) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self {
            matrix,
            request: Mutex::new(None),
            sas: Mutex::new(None),
            incoming: Mutex::new(None),
            outgoing: Mutex::new(None),
            handler_guards: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl SessionResource for MatrixVerification {
    async fn release(&self) {
        self.handler_guards.lock().await.clear();
        *self.incoming.lock().await = None;
        *self.outgoing.lock().await = None;
    }
}

#[async_trait]
impl VerificationPort for MatrixVerification {
    async fn listen_for_verification(
        &self,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()> {
        listen_for_verification(
            &self.matrix.client().await?,
            &self.incoming,
            &self.handler_guards,
            &self.outgoing,
            &self.request,
            &self.sas,
            tx,
        )
        .await
    }

    async fn verify_this_session(&self) -> Result<()> {
        verify_this_session(&self.matrix.client().await?, &self.request, &self.outgoing).await
    }

    async fn accept_verification(&self) -> Result<()> {
        accept_verification(&self.request).await
    }

    async fn confirm_verification(&self) -> Result<()> {
        confirm_verification(&self.sas).await
    }

    async fn reject_verification(&self) -> Result<()> {
        reject_verification(&self.sas, &self.request).await
    }

    async fn cancel_verification(&self) -> Result<()> {
        cancel_verification(&self.request).await
    }
}
