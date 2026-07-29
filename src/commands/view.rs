use std::path::PathBuf;
use std::sync::Arc;

use super::messages::UserMessage;
use crate::domain::models::{ConnectionStatus, LoginMethod, RoomList, Space};

#[derive(Clone, Default)]
pub struct AppViewState {
    pub lifecycle: LifecycleView,
    pub connection: ConnectionStatus,
    pub directory: DirectoryView,
    pub pagination: PaginationView,
    pub toast: Toast,
}

impl AppViewState {
    pub fn logged_out() -> Self {
        Self {
            lifecycle: LifecycleView {
                step: LoginStep::Homeserver,
                ..LifecycleView::default()
            },
            ..Self::default()
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum Toast {
    #[default]
    None,
    Error(UserMessage),
    FileSaved(String),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct PaginationView {
    pub generation: i32,
    pub backwards_loading: bool,
    pub forwards_loading: bool,
    pub new_messages: u32,
}

impl PaginationView {
    pub fn retarget(&mut self, generation: i32) {
        if self.generation != generation {
            *self = Self {
                generation,
                ..Self::default()
            };
        }
    }
}

#[derive(Clone, Default)]
pub struct LifecycleView {
    pub step: LoginStep,
    pub activity: LoginActivity,
    pub messages: Vec<UserMessage>,
    pub method: LoginMethod,
    pub resolved_homeserver: String,
    pub user_id: String,
    pub avatar_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum LoginStep {
    #[default]
    Loading,
    Homeserver,
    Credentials,
    LoggedIn,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum LoginActivity {
    #[default]
    Idle,
    LoadingSession,
    OpeningStore,
    Connecting,
    RestoringAuth,
    CheckingServer,
    LoggingIn,
    OpeningBrowser,
    WaitingAuth,
    Syncing,
    CleaningUp,
}

#[derive(Clone)]
pub struct DirectoryView {
    pub rooms: RoomList,
    pub spaces: Arc<[Space]>,
    pub subspaces: Arc<[Space]>,
    pub space_id: String,
    pub subspace_id: String,
}

impl Default for DirectoryView {
    fn default() -> Self {
        Self {
            rooms: Arc::from(Vec::new()),
            spaces: Arc::from(Vec::new()),
            subspaces: Arc::from(Vec::new()),
            space_id: String::new(),
            subspace_id: String::new(),
        }
    }
}
