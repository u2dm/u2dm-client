use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};
use tokio::task::spawn_blocking;
use tokio::time::sleep;

use super::{DemoAuthed, unavailable_if};
use crate::adapters::demo::{
    audio, data, media, message_menu, pinned, powers, settings, stickers, videos,
};
use crate::adapters::video;
use crate::domain::image_pack::PackId;
use crate::domain::media::{MediaRendition, MediaUsage, WaveformNeed};
use crate::domain::message::{PinChange, PinnedMessage};
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::matrix::{ImagePackPort, MediaPort, PackCatalog, PinnedPort};

#[async_trait]
impl PinnedPort for DemoAuthed {
    async fn subscribe_pinned(
        &self,
        room_id: &RoomId,
        pinned_tx: mpsc::Sender<Vec<PinnedMessage>>,
    ) -> Result<()> {
        pinned::pause_arrival().await;
        let forward = self.forward_pins(room_id, &pinned_tx);
        let repinned = pinned::scenario()
            .repins
            .then(|| data::latest_pinnable(room_id))
            .flatten();
        match repinned {
            Some(newest) => {
                tokio::select! {
                    () = forward => {}
                    () = self.repin(room_id, &newest.event_id) => {}
                }
            }
            None => forward.await,
        }
        Ok(())
    }

    async fn change_pin(&self, room_id: &RoomId, event_id: &str, change: PinChange) -> Result<()> {
        message_menu::pause().await;
        unavailable_if(!powers::messages().pin, "pinning in this room")?;
        unavailable_if(message_menu::scenario().pin_fails, "changing pins")?;
        tracing::debug!(%room_id, event_id, ?change, "demo: changing a pin");
        self.change_board(room_id, |pins| match change {
            PinChange::Pin => {
                if !pins.iter().any(|pin| pin == event_id) {
                    pins.push(event_id.to_owned());
                }
            }
            PinChange::Unpin => pins.retain(|pin| pin != event_id),
        });
        Ok(())
    }
}

impl DemoAuthed {
    fn pin_board(&self, room_id: &RoomId) -> Option<watch::Receiver<Vec<String>>> {
        let mut boards = self.pin_boards.lock().ok()?;
        let board = boards.entry(room_id.clone()).or_insert_with(|| {
            let pins = data::pinned_messages(room_id)
                .into_iter()
                .map(|pin| pin.event_id)
                .collect();
            watch::channel(pins).0
        });
        Some(board.subscribe())
    }

    pub(super) fn change_board(&self, room_id: &RoomId, change: impl FnOnce(&mut Vec<String>)) {
        drop(self.pin_board(room_id));
        if let Ok(boards) = self.pin_boards.lock()
            && let Some(board) = boards.get(room_id)
        {
            board.send_modify(change);
        }
    }

    fn pinned_from(&self, room_id: &RoomId, pins: &[String]) -> Vec<PinnedMessage> {
        let history = self.room_history(room_id);
        pins.iter()
            .filter_map(|pin| {
                history
                    .iter()
                    .rev()
                    .find(|message| message.event_id.as_deref() == Some(pin.as_str()))
            })
            .filter_map(data::pinned_message)
            .collect()
    }

    async fn forward_pins(&self, room_id: &RoomId, pinned_tx: &mpsc::Sender<Vec<PinnedMessage>>) {
        let Some(mut board) = self.pin_board(room_id) else {
            return;
        };
        loop {
            let pins = board.borrow_and_update().clone();
            if pinned_tx
                .send(self.pinned_from(room_id, &pins))
                .await
                .is_err()
            {
                return;
            }
            tokio::select! {
                changed = board.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                () = pinned_tx.closed() => return,
            }
        }
    }

    async fn repin(&self, room_id: &RoomId, event_id: &str) {
        loop {
            sleep(pinned::REPIN_INTERVAL).await;
            self.change_board(room_id, |pins| {
                if pins.iter().any(|pin| pin == event_id) {
                    pins.retain(|pin| pin != event_id);
                } else {
                    pins.push(event_id.to_owned());
                }
            });
        }
    }
}

#[async_trait]
impl MediaPort for DemoAuthed {
    fn set_auto_load(&self, on: bool) {
        media::set_auto_load(on);
    }

    async fn cache_usage(&self) -> Result<MediaUsage> {
        settings::pause_cache().await;
        Ok(media::cache_usage())
    }

    async fn clear_cache(&self, keep: Vec<PathBuf>) -> Result<MediaUsage> {
        settings::pause_cache().await;
        unavailable_if(settings::clearing_fails(), "clearing downloaded media")?;
        tracing::info!(?keep, "demo: the downloaded media is cleared");
        Ok(media::clear_cache(&keep))
    }

    async fn download_media(
        &self,
        _room_id: &RoomId,
        event_id: &str,
        _rendition: MediaRendition,
    ) -> Result<Vec<u8>> {
        let path = media::full_file_path(event_id)
            .ok_or_else(|| AppError::Other(format!("no demo asset for event {event_id}")))?;
        Ok(fs::read(path)?)
    }

    async fn materialize_video(&self, _room_id: &RoomId, event_id: &str) -> Result<PathBuf> {
        videos::pause_download().await;
        media::video_asset_path(event_id)
            .ok_or_else(|| AppError::Other(format!("no demo video for event {event_id}")))
    }

    async fn materialize_audio(
        &self,
        _room_id: &RoomId,
        event_id: &str,
        need: WaveformNeed,
    ) -> Result<PathBuf> {
        audio::pause_download().await;
        let path = media::fetch_audio(event_id)
            .ok_or_else(|| AppError::Other(format!("no demo audio for event {event_id}")))?;
        if need == WaveformNeed::Compute {
            let owned = path.clone();
            let learned = spawn_blocking(move || video::probe_audio(&owned))
                .await
                .ok()
                .flatten()
                .and_then(|probe| probe.waveform);
            if let Some(waveform) = learned {
                media::remember_waveform(event_id, waveform);
            }
        }
        Ok(path)
    }
}

#[async_trait]
impl ImagePackPort for DemoAuthed {
    async fn catalog(&self, room_id: &RoomId) -> Result<PackCatalog> {
        let demo = stickers::scenario();
        stickers::pause_catalog().await;
        unavailable_if(demo.catalog_fails, "loading sticker packs")?;
        let packs = if demo.catalog_is_empty {
            Vec::new()
        } else {
            let shared = self
                .shared_packs
                .lock()
                .map(|shared| shared.clone())
                .unwrap_or_default();
            data::image_packs(room_id, &shared)
        };
        Ok(PackCatalog {
            packs,
            room_encrypted: demo.room_is_encrypted || data::room_is_encrypted(room_id),
        })
    }

    async fn prefetch(&self, mxcs: &[String]) -> usize {
        stickers::pause_prefetch().await;
        media::prefetch_pack_images(mxcs)
    }

    async fn send_sticker(
        &self,
        room_id: &RoomId,
        pack: &PackId,
        shortcode: &str,
        in_reply_to: Option<&str>,
    ) -> Result<()> {
        powers::refuse_unless(powers::messages().send_stickers, "sending stickers")?;
        unavailable_if(stickers::scenario().send_fails, "sending stickers")?;
        let image = data::sticker_image(pack, shortcode)
            .ok_or_else(|| AppError::Other(format!("no demo sticker {pack}/{shortcode}")))?;
        self.append_own_sticker(room_id, &image, in_reply_to).await;
        Ok(())
    }

    async fn set_global(&self, pack: &PackId, global: bool) -> Result<()> {
        unavailable_if(stickers::scenario().global_fails, "sharing image packs")?;
        if !data::is_room_pack(pack) {
            return Err(AppError::Other(format!("no demo room pack {pack}")));
        }
        if let Ok(mut shared) = self.shared_packs.lock() {
            shared.insert(pack.to_string(), global);
        }
        Ok(())
    }
}
