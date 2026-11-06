mod call_ports;
mod crypto_ports;
mod media_ports;
mod people_ports;
mod room_ports;
mod timeline_port;
mod timeline_tasks;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};
use url::Url;

use super::account::AccountBook;
use super::encryption::EncryptionBook;
use super::identity::IdentityBook;
use super::{data, login, logout, media, powers, presence, receipts, sync, typing, user_info};
use crate::adapters::draft_size::RoomFacts;
use crate::adapters::markdown::RoomMentions;
use crate::domain::auth::{AuthMethod, LoginCredentials, OAuthLoginData, ServerInfo, Session};
use crate::domain::departure::Removal;
use crate::domain::event_size::{Encryption, EventIds};
use crate::domain::invite::Invite;
use crate::domain::link::LauncherSafeUrl;
use crate::domain::message::TimelineMessage;
use crate::domain::room::{Room, RoomId, Space};
use crate::domain::sync::SyncEvent;
use crate::domain::timeline::TimelineUpdate;
use crate::domain::user_info::RoomMembership;
use crate::domain::verification::VerificationEvent;
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    AuthPort, AuthenticatedSession, CleanupReport, DeviceRemoval, InterruptedLogin,
    LocalDataOwnership, ProgressSink, RestoreOutcome, RestoreStep, SessionPort, StoreAdoption,
    SyncSink,
};

pub struct DemoMatrix;

#[async_trait]
impl AuthPort for DemoMatrix {
    async fn discover_auth(&self, homeserver: &str, _passphrase: &str) -> Result<ServerInfo> {
        login::pause().await;
        let (auth_methods, unsupported_flows) = login::requested().map_or_else(
            || (vec![AuthMethod::Password], Vec::new()),
            |demo| (demo.methods.clone(), demo.unsupported_flows.clone()),
        );
        Ok(ServerInfo {
            auth_methods,
            unsupported_flows,
            homeserver_url: format!("https://{homeserver}"),
            sync: sync::support(),
        })
    }

    async fn login_password(&self, _creds: LoginCredentials) -> Result<Session> {
        login::pause().await;
        Ok(data::session())
    }

    async fn login_oauth_start(&self) -> Result<OAuthLoginData> {
        demo_oauth_start().await
    }

    async fn login_oauth_finish(&self) -> Result<Session> {
        unavailable_if(!login::oauth_succeeds(), "OAuth login")?;
        login::pause().await;
        Ok(oauth_session())
    }

    async fn adopt_session(
        &self,
        session: &Session,
        _passphrase: &str,
    ) -> Result<Box<dyn StoreAdoption>> {
        login::pause().await;
        Ok(Box::new(DemoAdoption {
            session: session.clone(),
            txn: "demo".to_owned(),
        }))
    }

    async fn cancel_oauth(&self) {}

    async fn restore_session(
        &self,
        session: &Session,
        _passphrase: &str,
        on_progress: ProgressSink,
    ) -> Result<RestoreOutcome> {
        for step in [RestoreStep::Connecting, RestoreStep::RestoringAuth] {
            on_progress(step);
            login::pause().await;
        }
        if login::identity_diverged() {
            tracing::info!(
                "demo mode: the homeserver publishes another signing key for this device"
            );
            return Ok(RestoreOutcome::Diverged(logout::answer().await));
        }
        Ok(RestoreOutcome::Restored(Box::new(authenticated(
            session.clone(),
        ))))
    }

    async fn reauthenticate(
        &self,
        prior: &Session,
        _passphrase: &str,
        _creds: LoginCredentials,
    ) -> Result<AuthenticatedSession> {
        login::pause().await;
        Ok(authenticated(prior.clone()))
    }

    async fn reauth_oauth_start(
        &self,
        _prior: &Session,
        _passphrase: &str,
    ) -> Result<OAuthLoginData> {
        demo_oauth_start().await
    }

    async fn reauth_oauth_finish(&self, prior: &Session) -> Result<AuthenticatedSession> {
        unavailable_if(!login::oauth_succeeds(), "OAuth login")?;
        login::pause().await;
        Ok(authenticated(prior.clone()))
    }

    fn local_data_ownership(&self) -> LocalDataOwnership {
        LocalDataOwnership::Exclusive
    }

    async fn interrupted_logins(&self) -> Result<Vec<InterruptedLogin>> {
        Ok(Vec::new())
    }

    async fn unwind_login(&self, _txn: &str) -> CleanupReport {
        CleanupReport::default()
    }

    async fn settle_login(&self, _txn: &str) -> CleanupReport {
        CleanupReport::default()
    }

    async fn mark_rolled_back(&self, _txn: &str) -> Result<()> {
        Ok(())
    }

    async fn forget_login(&self, _txn: &str) {}
}

struct DemoAdoption {
    session: Session,
    txn: String,
}

#[async_trait]
impl StoreAdoption for DemoAdoption {
    fn transaction(&self) -> &str {
        &self.txn
    }

    async fn credentials_staged(&self) -> Result<()> {
        Ok(())
    }

    async fn credentials_written(&self) -> Result<()> {
        Ok(())
    }

    async fn rolling_back(&self) -> Result<()> {
        Ok(())
    }

    async fn logout(&self) -> DeviceRemoval {
        logout::answer().await
    }

    async fn commit(self: Box<Self>) -> AuthenticatedSession {
        authenticated(self.session)
    }

    async fn unwind(self: Box<Self>) -> CleanupReport {
        CleanupReport::default()
    }
}

fn oauth_session() -> Session {
    Session {
        client_id: Some("demo-oauth-client".to_owned()),
        ..data::session()
    }
}

fn authenticated(session: Session) -> AuthenticatedSession {
    let authed = Arc::new(DemoAuthed::default());
    let sync = Arc::clone(&authed);
    let timeline = Arc::clone(&authed);
    let pinned = Arc::clone(&authed);
    let media = Arc::clone(&authed);
    let verification = Arc::clone(&authed);
    let identity = Arc::clone(&authed);
    let encryption = Arc::clone(&authed);
    let space_order = Arc::clone(&authed);
    let space_index = Arc::clone(&authed);
    let invites = Arc::clone(&authed);
    let leaves = Arc::clone(&authed);
    let upgrades = Arc::clone(&authed);
    let room_previews = Arc::clone(&authed);
    let room_creation = Arc::clone(&authed);
    let invite_people = Arc::clone(&authed);
    let typing = Arc::clone(&authed);
    let presence = Arc::clone(&authed);
    let call_signals = Arc::clone(&authed);
    let rtc_membership = Arc::clone(&authed);
    let push = Arc::clone(&authed);
    let room_info = Arc::clone(&authed);
    let user_info = Arc::clone(&authed);
    let image_packs = Arc::clone(&authed);
    let account = Arc::clone(&authed);
    AuthenticatedSession {
        session,
        sync,
        timeline,
        pinned,
        media,
        verification,
        identity,
        encryption,
        space_order,
        space_index,
        invites,
        leaves,
        upgrades,
        room_previews,
        room_creation,
        invite_people,
        typing,
        presence,
        call_signals,
        rtc_membership,
        push,
        room_info,
        user_info,
        image_packs,
        account,
        lifecycle: authed,
    }
}

struct ActiveRoom {
    room_id: RoomId,
    live: bool,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
    prepended: usize,
    receipts: Vec<receipts::Receipt>,
}

type SharedActiveRoom = Arc<Mutex<Option<ActiveRoom>>>;

#[derive(Default, Clone)]
struct RoomLists {
    joined: Vec<RoomId>,
    overrides: data::RoomOverrides,
}

impl RoomLists {
    fn rooms(&self) -> Vec<Arc<Room>> {
        data::rooms_with(&self.joined, &self.overrides)
    }

    fn spaces(&self) -> Vec<Space> {
        data::spaces_with(&self.joined, &self.overrides)
    }

    fn invites(&self) -> Vec<Arc<Invite>> {
        data::invites_with(&self.joined, &self.overrides)
    }

    fn removals(&self) -> Vec<Arc<Removal>> {
        data::removals_with(&self.overrides)
    }
}

fn current_lists(lists: &Mutex<RoomLists>) -> RoomLists {
    lists.lock().map(|lists| lists.clone()).unwrap_or_default()
}

#[derive(Clone, Copy)]
enum ListKind {
    Removals,
    Rooms,
    Invites,
    Spaces,
}

const EVERY_LIST: [ListKind; 4] = [
    ListKind::Removals,
    ListKind::Rooms,
    ListKind::Invites,
    ListKind::Spaces,
];

fn announce_lists(sink: &SyncSink, lists: &Mutex<RoomLists>, kinds: &[ListKind]) {
    let lists = current_lists(lists);
    for kind in kinds {
        sink(match kind {
            ListKind::Removals => SyncEvent::Removals(lists.removals().into()),
            ListKind::Rooms => SyncEvent::Rooms(lists.rooms().into()),
            ListKind::Invites => SyncEvent::Invites(lists.invites().into()),
            ListKind::Spaces => SyncEvent::Spaces(lists.spaces().into()),
        });
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
enum OutgoingFlow {
    #[default]
    None,
    Session,
    Person(String),
}

#[derive(Default)]
struct DemoAuthed {
    active: SharedActiveRoom,
    sent: AtomicU64,
    own_sends: Mutex<HashMap<RoomId, Vec<TimelineMessage>>>,
    arrived: Mutex<HashMap<RoomId, Vec<TimelineMessage>>>,
    verification_tx: Mutex<Option<mpsc::UnboundedSender<VerificationEvent>>>,
    sas_started: Arc<AtomicBool>,
    outgoing: Arc<Mutex<OutgoingFlow>>,
    flows: Arc<AtomicU64>,
    sync_sink: Mutex<Option<SyncSink>>,
    lists: Arc<Mutex<RoomLists>>,
    pin_boards: Mutex<HashMap<RoomId, watch::Sender<Vec<String>>>>,
    deleted: Arc<Mutex<HashSet<String>>>,
    ignored: Mutex<HashSet<String>>,
    memberships: Mutex<HashMap<(RoomId, String), RoomMembership>>,
    creations: AtomicU64,
    shared_packs: Mutex<HashMap<String, bool>>,
    identity: IdentityBook,
    encryption: EncryptionBook,
    account: Arc<AccountBook>,
    verified_people: user_info::VerifiedPeople,
    typing_notices: typing::NoticeClock,
    presence: presence::PresenceBook,
}

#[async_trait]
impl SessionPort for DemoAuthed {
    async fn subscribe_session_changes(
        &self,
        _session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()> {
        Ok(())
    }

    async fn fetch_user_avatar(&self) -> Result<Option<PathBuf>> {
        Ok(media::user_avatar_path())
    }

    async fn logout(&self) -> DeviceRemoval {
        logout::answer().await
    }

    async fn suspend(&self) {}

    async fn clear_store(&self) -> CleanupReport {
        CleanupReport::default()
    }
}

async fn demo_oauth_start() -> Result<OAuthLoginData> {
    let page = login::sign_in_page().ok_or_else(|| unavailable("OAuth login"))?;
    login::pause().await;
    let page = Url::parse(page).map_err(AppError::other)?;
    Ok(OAuthLoginData {
        auth_url: LauncherSafeUrl::sign_in_page(page)?,
    })
}

fn with_room_facts<T>(room_id: &RoomId, measure: impl FnOnce(&RoomFacts<'_>) -> T) -> T {
    let session = data::session();
    measure(&RoomFacts {
        room_id: room_id.as_ref(),
        sender: &session.user_id,
        device_id: &session.device_id,
        encryption: Encryption::when(data::room_is_encrypted(room_id)),
        event_ids: EventIds::Hashed,
        room_mentions: RoomMentions::when(powers::messages().notify_room),
    })
}

fn unavailable(action: &str) -> AppError {
    AppError::Other(format!("{action} is not available in demo mode"))
}

fn unavailable_if(refused: bool, action: &str) -> Result<()> {
    if refused {
        return Err(unavailable(action));
    }
    Ok(())
}
