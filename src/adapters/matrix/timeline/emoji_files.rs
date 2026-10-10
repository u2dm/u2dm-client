use std::collections::HashSet;
use std::sync::Mutex as StdMutex;

use futures_util::{StreamExt, stream};
use matrix_sdk::Client;
use matrix_sdk::ruma::{MxcUri, OwnedMxcUri};
use tokio::sync::mpsc;

use crate::adapters::matrix::media::MediaService;
use crate::domain::message::CustomEmoji;

const DOWNLOADS_INFLIGHT: usize = 4;

pub(super) struct Settled {
    mxc: String,
    on_disk: bool,
}

#[derive(Default)]
struct Store {
    unavailable: HashSet<String>,
    wanted: HashSet<String>,
    downloading: HashSet<String>,
}

#[derive(Default)]
pub(in crate::adapters::matrix) struct EmojiFiles {
    store: StdMutex<Store>,
}

impl EmojiFiles {
    pub(super) fn state(&self, key: &str, media: &MediaService) -> Option<CustomEmoji> {
        if !<&MxcUri>::from(key).is_valid() {
            return None;
        }
        if media.has_sticker(key) {
            return Some(CustomEmoji::Downloaded);
        }
        let Ok(mut store) = self.store.lock() else {
            return Some(CustomEmoji::Downloading);
        };
        if store.unavailable.contains(key) {
            return Some(CustomEmoji::Unavailable);
        }
        if !store.downloading.contains(key) {
            store.wanted.insert(key.to_owned());
        }
        Some(CustomEmoji::Downloading)
    }

    pub(super) fn take_wanted(&self) -> Vec<String> {
        let Ok(mut store) = self.store.lock() else {
            return Vec::new();
        };
        let wanted: Vec<String> = store.wanted.drain().collect();
        store.downloading.extend(wanted.iter().cloned());
        wanted
    }

    pub(super) fn record(&self, settled: Settled) -> String {
        let Settled { mxc, on_disk } = settled;
        if let Ok(mut store) = self.store.lock() {
            store.downloading.remove(&mxc);
            if !on_disk {
                store.unavailable.insert(mxc.clone());
            }
        }
        mxc
    }
}

pub(super) async fn download_emoji_files(
    client: &Client,
    media: &MediaService,
    wanted: Vec<String>,
    settled: &mpsc::Sender<Settled>,
) {
    let mut downloads = stream::iter(wanted)
        .map(|mxc| async move {
            let on_disk = media
                .fetch_sticker_by_mxc(client, OwnedMxcUri::from(mxc.as_str()))
                .await
                .is_some();
            Settled { mxc, on_disk }
        })
        .buffer_unordered(DOWNLOADS_INFLIGHT);
    while let Some(image) = downloads.next().await {
        if settled.send(image).await.is_err() {
            return;
        }
    }
}
