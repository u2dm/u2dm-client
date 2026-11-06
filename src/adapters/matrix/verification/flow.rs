use std::future::ready;
use std::pin::pin;

use futures_util::{Stream, StreamExt, stream};
use matrix_sdk::encryption::verification::{
    CancelInfo, Emoji, EmojiShortAuthString, SasState, SasVerification, Verification,
    VerificationRequest, VerificationRequestState,
};
use matrix_sdk::ruma::events::key::verification::cancel::CancelCode;
use tokio::sync::{Mutex, mpsc};
use tokio::time::timeout;

use super::UNANSWERED_REQUEST_TIMEOUT;
use crate::domain::verification::{VerificationCancellation, VerificationEmoji, VerificationEvent};

pub(super) fn is_settled(state: &VerificationRequestState) -> bool {
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

pub(super) async fn run_verification(
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
