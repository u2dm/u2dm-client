use std::collections::HashSet;
use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use crate::adapters::ui::backend::{UiBackend, UiEventContext, apply_pack_art, apply_pack_avatar};
use crate::adapters::ui::decode::{AvatarSlot, load_avatar_async, request_pack_cell};
use crate::adapters::ui::dto::{
    PackArt, PackRowDto, PackTabDto, audio_row_update, pack_art, pack_grid,
};
use crate::adapters::ui::fields::MessageFields;
use crate::adapters::ui::props::{BoolProp, StringProp, UiProps};
use crate::adapters::ui::reconcile::{
    RowReplacements, downloading_pack_avatars, index_pack_grid, pack_avatar_downloaded,
    retain_awaited_downloads,
};
use crate::adapters::ui::rows::patch_rows_by_id;
use crate::adapters::ui::session::with_session;
use crate::adapters::ui::splice_model::SpliceModel;
use crate::adapters::ui::{audio, video};
use crate::commands::view::{AudioView, NowPlaying, PackView, TrackFile, VideoView};
use crate::ports::media::MediaCache;

pub(super) fn apply_packs<B: UiBackend>(
    w: &B::Window,
    last: Option<&PackView>,
    stickers: &PackView,
    media: &dyn MediaCache,
) {
    let catalog_changed = last.is_none_or(|l| {
        !Arc::ptr_eq(&l.packs, &stickers.packs) || l.generation != stickers.generation
    });
    if catalog_changed {
        rebuild_pack_grid::<B>(w, stickers, media);
    } else if last.is_some_and(|l| l.ready_images != stickers.ready_images) {
        settle_pack_downloads::<B>(media);
    }
    w.set_bool(BoolProp::PackRoomEncrypted, stickers.room_encrypted);
    w.set_bool(BoolProp::PackLoading, stickers.loading);
}

pub(super) fn rebuild_pack_grid<B: UiBackend>(
    w: &B::Window,
    packs: &PackView,
    media: &dyn MediaCache,
) {
    let (active, browse) =
        with_session(|session| (session.room.active_generation, session.pack_browse.clone()));
    let Some(browse) = browse else {
        return;
    };
    let for_this_room = packs.generation == active;
    let grid = if for_this_room {
        pack_grid(packs.packs.as_ref(), &browse, media)
    } else {
        pack_grid(&[], &browse, media)
    };
    w.set_pack_surface(browse.surface);
    w.set_bool(BoolProp::PackHasPacks, for_this_room && grid.has_packs);

    let replacements = index_pack_grid(&grid);
    let missing_icons = packs_missing_icons(&grid.packs);
    let models = B::models();
    replace_reshaped_rows::<B>(&models.pack_rows, grid.rows, &replacements);
    models.pack_tabs.set_vec(
        grid.packs
            .into_iter()
            .map(B::PackTab::from)
            .collect::<Vec<_>>(),
    );
    for icon_cell_key in &missing_icons {
        request_pack_cell(icon_cell_key);
    }
    settle_pack_downloads::<B>(media);
}

fn replace_reshaped_rows<B: UiBackend>(
    rows: &SpliceModel<B::PackRow>,
    grid_rows: Vec<PackRowDto>,
    replacements: &RowReplacements,
) {
    let kept = rows.row_count();
    let total = grid_rows.len();
    let mut appended = Vec::new();
    for (row, entry) in grid_rows.into_iter().enumerate() {
        if row >= kept {
            appended.push(B::PackRow::from(entry));
        } else if replacements.must_replace(row) {
            rows.set_row_data(row, B::PackRow::from(entry));
        }
    }
    rows.truncate(total);
    rows.insert_rows(kept, appended);
}

fn settle_pack_downloads<B: UiBackend>(media: &dyn MediaCache) {
    let mut settled = Vec::new();
    retain_awaited_downloads(|key, mxc| match pack_art(key, mxc, media) {
        PackArt::Downloading => true,
        PackArt::Decoding => false,
        art => {
            settled.push((key.to_owned(), art));
            false
        }
    });
    for (key, art) in settled {
        match art {
            PackArt::Ready(image) => apply_pack_art::<B>(&key, Some(&image)),
            PackArt::Failed => apply_pack_art::<B>(&key, None),
            PackArt::Decoding | PackArt::Downloading => {}
        }
    }
    settle_pack_avatars::<B>(media);
}

fn settle_pack_avatars<B: UiBackend>(media: &dyn MediaCache) {
    for (pack_id, mxc) in downloading_pack_avatars() {
        if let Some(path) = media.sticker_path(&mxc) {
            pack_avatar_downloaded(&pack_id);
            let slot = AvatarSlot::Pack(pack_id.clone());
            if let Some(image) = load_avatar_async(Some(&path), slot) {
                apply_pack_avatar::<B>(&pack_id, Some(&image));
            }
        } else if media.sticker_failed(&mxc) {
            apply_pack_avatar::<B>(&pack_id, None);
        }
    }
}

fn packs_missing_icons(packs: &[PackTabDto]) -> Vec<SharedString> {
    packs
        .iter()
        .filter(|pack| pack.icon.is_none() && !pack.icon_cell_key.is_empty())
        .map(|pack| pack.icon_cell_key.clone())
        .collect()
}

pub(super) fn apply_video<B: UiBackend>(w: &B::Window, view: &VideoView) {
    let VideoView {
        visible,
        loading,
        path,
        error,
    } = view;

    w.set_bool(BoolProp::VideoVisible, *visible);
    w.set_bool(BoolProp::VideoLoading, *loading);
    w.set_video_error(*error);

    match path.as_deref().filter(|_| *visible) {
        Some(path) => {
            audio::pause(w);
            video::open(w, &w.as_weak(), path);
        }
        None => video::close(w),
    }
}

pub(super) fn audio_start(video: &VideoView) -> audio::Start {
    if video.visible {
        audio::Start::Paused
    } else {
        audio::Start::Playing
    }
}

pub(super) fn apply_audio<B: UiBackend>(
    w: &B::Window,
    last: Option<&AudioView>,
    view: &AudioView,
    start: audio::Start,
    ctx: &UiEventContext<'_, B>,
) {
    if let Some(now) = &view.now_playing {
        show_now_playing(w, now, start);
    } else {
        w.set_bool(BoolProp::AudioVisible, false);
        w.set_bool(BoolProp::AudioLoading, false);
        w.set_string(StringProp::AudioEventId, SharedString::new());
        audio::close(w);
    }
    let previous = last.and_then(|l| l.now_playing.as_ref());
    for now in previous.into_iter().chain(view.now_playing.as_ref()) {
        refresh_audio_row::<B>(now, ctx);
    }
}

fn show_now_playing<W>(w: &W, now: &NowPlaying, start: audio::Start)
where
    W: ComponentHandle + UiProps + 'static,
{
    audio::prepare(w, now.request, now.meta.duration);
    w.set_string(StringProp::AudioEventId, SharedString::from(&now.event_id));
    w.set_string(
        StringProp::AudioRoomId,
        SharedString::from(now.room_id.as_ref()),
    );
    w.set_string(StringProp::AudioSender, SharedString::from(&now.sender));
    w.set_string(
        StringProp::AudioTitle,
        SharedString::from(&now.meta.filename),
    );
    w.set_audio_kind(now.meta.kind);
    w.set_bool(BoolProp::AudioLoading, now.file == TrackFile::Downloading);
    w.set_bool(BoolProp::AudioVisible, true);
    if let TrackFile::Ready(path) = &now.file {
        audio::open(w, &w.as_weak(), now.request, path, start);
    }
}

fn refresh_audio_row<B: UiBackend>(now: &NowPlaying, ctx: &UiEventContext<'_, B>) {
    let update = audio_row_update(&now.meta, ctx.media);
    let ids = HashSet::from([now.event_id.as_str()]);
    patch_rows_by_id(
        &*ctx.models.timeline,
        &ids,
        &B::Message::event_id,
        |entry| {
            entry.set_media_state(update.media_state);
            entry.set_media_failure(update.media_failure);
            entry.set_waveform(update.waveform.clone());
        },
    );
}
