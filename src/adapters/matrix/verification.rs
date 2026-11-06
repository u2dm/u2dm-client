mod flow;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use matrix_sdk::encryption::verification::{SasVerification, VerificationRequest};
use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::ruma::OwnedUserId;
use matrix_sdk::ruma::events::key::verification::VerificationMethod;
use matrix_sdk::ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEvent;
use matrix_sdk::ruma::events::room::member::MembershipState;
use matrix_sdk::ruma::events::room::message::{MessageType, OriginalSyncRoomMessageEvent};
use matrix_sdk::{Client, Room};
use tokio::sync::{Mutex, mpsc};

use self::flow::{is_settled, run_verification};
use super::session::{ClientHandle, SessionResource};
use super::user_info::{own_user, parse_user};
use crate::domain::user_info::UserId;
use crate::domain::verification::VerificationEvent;
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
    let is_self = request.is_self_verification();
    if request.we_started() {
        if is_self {
            tracing::info!(%flow_id, "asked the account's other sessions to verify this one");
        } else {
            tracing::info!(%flow_id, user = %sender, "asked the user to verify in your direct chat");
        }
        return VerificationEvent::RequestSent {
            to: sender,
            is_self,
        };
    }
    tracing::info!(%sender, is_self, %flow_id, "verification request received");
    VerificationEvent::Requested { sender, is_self }
}

async fn verify_this_session(
    client: &Client,
    verification_request: &Mutex<Option<VerificationRequest>>,
    outgoing: &Mutex<Option<mpsc::Sender<VerificationRequest>>>,
) -> Result<()> {
    let queue = request_queue(verification_request, outgoing).await?;
    let encryption = client.encryption();
    let can_answer = encryption
        .has_devices_to_verify_against()
        .await
        .map_err(AppError::other)?;
    if !can_answer {
        return Err(AppError::Other(
            "no other session of this account can verify this one".into(),
        ));
    }
    let identity = encryption
        .get_user_identity(&own_user(client)?)
        .await
        .map_err(AppError::other)?
        .ok_or_else(|| AppError::Other("the account has no identity to verify against".into()))?;
    let request = identity
        .request_verification_with_methods(vec![VerificationMethod::SasV1])
        .await
        .map_err(|e| AppError::Other(format!("could not send the verification request: {e}")))?;
    hand_over(&queue, request).await
}

async fn verify_user(
    client: &Client,
    user_id: &UserId,
    verification_request: &Mutex<Option<VerificationRequest>>,
    outgoing: &Mutex<Option<mpsc::Sender<VerificationRequest>>>,
) -> Result<()> {
    let queue = request_queue(verification_request, outgoing).await?;
    let user = parse_user(user_id)?;
    let encryption = client.encryption();
    let can_sign = encryption
        .cross_signing_status()
        .await
        .is_some_and(|status| status.has_user_signing);
    if !can_sign {
        return Err(AppError::Other(
            "this session holds no user-signing key, so it cannot verify anyone".into(),
        ));
    }
    let room = client.get_dm_room(&user).ok_or_else(|| {
        AppError::Other("there is no direct chat with this person to send the request in".into())
    })?;
    ensure_partner_in(&room, &user).await?;
    let identity = match encryption.get_user_identity(&user).await {
        Ok(Some(identity)) => identity,
        Ok(None) => encryption
            .request_user_identity(&user)
            .await?
            .ok_or_else(|| AppError::Other("this person has no cross-signing identity".into()))?,
        Err(e) => return Err(AppError::other(e)),
    };
    if identity.is_verified() {
        return Err(AppError::Other("this person is already verified".into()));
    }
    let request = identity
        .request_verification_with_methods(vec![VerificationMethod::SasV1])
        .await
        .map_err(|e| AppError::Other(format!("could not send the verification request: {e}")))?;
    tracing::info!(room_id = %room.room_id(), %user, "sent a verification request to the direct chat");
    hand_over(&queue, request).await
}

async fn request_queue(
    verification_request: &Mutex<Option<VerificationRequest>>,
    outgoing: &Mutex<Option<mpsc::Sender<VerificationRequest>>>,
) -> Result<mpsc::Sender<VerificationRequest>> {
    if verification_request.lock().await.is_some() {
        return Err(AppError::Other("a verification is already running".into()));
    }
    outgoing
        .lock()
        .await
        .clone()
        .ok_or_else(|| AppError::Other("verification requests are not followed".into()))
}

async fn hand_over(
    queue: &mpsc::Sender<VerificationRequest>,
    request: VerificationRequest,
) -> Result<()> {
    if let Err(refused) = queue.try_send(request) {
        refused.into_inner().cancel().await.ok();
        return Err(AppError::Other(
            "the verification request could not be followed".into(),
        ));
    }
    Ok(())
}

async fn ensure_partner_in(room: &Room, user: &OwnedUserId) -> Result<()> {
    match room.get_member(user).await {
        Ok(Some(member))
            if !matches!(
                member.membership(),
                MembershipState::Join | MembershipState::Invite
            ) =>
        {
            Err(AppError::PartnerLeftChat)
        }
        Ok(_) => Ok(()),
        Err(e) => Err(AppError::other(e)),
    }
}

async fn pending(
    verification_request: &Mutex<Option<VerificationRequest>>,
) -> Result<VerificationRequest> {
    verification_request
        .lock()
        .await
        .clone()
        .ok_or_else(|| AppError::Other("No pending verification request".into()))
}

async fn accept_verification(
    verification_request: &Mutex<Option<VerificationRequest>>,
) -> Result<()> {
    pending(verification_request).await?.accept().await?;
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
    pending(verification_request).await?.cancel().await?;
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

    async fn verify_user(&self, user_id: &UserId) -> Result<()> {
        verify_user(
            &self.matrix.client().await?,
            user_id,
            &self.request,
            &self.outgoing,
        )
        .await
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
