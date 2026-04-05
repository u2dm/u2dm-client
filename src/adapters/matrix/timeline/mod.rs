mod convert;
mod diff;
mod filter;
mod subscribe;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use matrix_sdk::Client;
use matrix_sdk::ruma::events::room::MediaSource;
pub(super) use subscribe::subscribe_timeline;

pub(super) struct TimelineContext<'a> {
    pub(super) client: &'a Client,
    pub(super) media_dir: &'a Path,
    pub(super) media_sources: &'a StdMutex<HashMap<String, MediaSource>>,
    pub(super) materialized: &'a StdMutex<HashMap<String, PathBuf>>,
    pub(super) own_user_id: Option<&'a str>,
}
