use slint::{Image, SharedString};

use super::message::MediaState;
use crate::adapters::ui::decode::{
    AvatarSlot, Decoded, MediaSlot, load_avatar_async, peek_thumbnail, record_pack_cell_need,
};
use crate::adapters::ui::emote::slug;
use crate::adapters::ui::schema::{define_ui_enum, pack_surfaces, variant_named};
use crate::domain::image_pack::{ImagePack, ImageUsage, PackId, PackImage, PackOrigin, PackTitle};
use crate::ports::media::MediaCache;

pack_surfaces!(define_ui_enum PackSurface;);

pack_surfaces!(variant_named pack_surface_named PackSurface;);

const EMOTE_SUGGESTION_LIMIT: usize = 20;

const SLUG_DELIMITER: char = '/';

#[derive(Clone, PartialEq, Eq)]
pub struct PackBrowse {
    pub surface: PackSurface,
    pub needle: String,
    pub columns: i32,
}

impl PackBrowse {
    pub fn new(surface: PackSurface, query: &str, columns: i32) -> Self {
        Self {
            surface,
            needle: pack_needle(query),
            columns: columns.max(1),
        }
    }
}

#[derive(Clone)]
pub struct PackCellDto {
    pub key: SharedString,
    pub pack_id: SharedString,
    pub shortcode: SharedString,
    pub label: SharedString,
    pub image: Option<Image>,
    pub media_state: MediaState,
    pub awaited_mxc: Option<String>,
}

pub struct PackRowDto {
    pub title: SharedString,
    pub untitled: bool,
    pub pack_id: SharedString,
    pub origin: PackOrigin,
    pub is_header: bool,
    pub cells: Vec<PackCellDto>,
}

pub struct PackTabDto {
    pub id: SharedString,
    pub title: SharedString,
    pub untitled: bool,
    pub header_row: i32,
    pub icon: Option<Image>,
    pub has_icon: bool,
    pub icon_cell_key: SharedString,
    pub avatar: Option<PackAvatarDto>,
}

pub struct PackAvatarDto {
    pub downloading_mxc: Option<String>,
    pub fallback_cell_key: SharedString,
}

struct PackIcon {
    image: Option<Image>,
    cell_key: SharedString,
    avatar: Option<PackAvatarDto>,
}

pub struct PackGrid {
    pub rows: Vec<PackRowDto>,
    pub packs: Vec<PackTabDto>,
    pub has_packs: bool,
}

pub const CELL_KEY_SEPARATOR: char = '\u{1}';

pub fn cell_key(pack: &PackId, shortcode: &str) -> String {
    format!("{pack}{CELL_KEY_SEPARATOR}{shortcode}")
}

pub fn cell_pack(key: &str) -> &str {
    key.split_once(CELL_KEY_SEPARATOR)
        .map_or(key, |(pack, _)| pack)
}

pub fn pack_needle(query: &str) -> String {
    query.trim().to_lowercase()
}

pub fn pack_grid(packs: &[ImagePack], browse: &PackBrowse, media: &dyn MediaCache) -> PackGrid {
    match browse.surface {
        PackSurface::StickerGrid => sectioned_grid(
            packs,
            &browse.needle,
            ImageUsage::offers_sticker,
            browse.columns,
            media,
        ),
        PackSurface::EmoteGrid => sectioned_grid(
            packs,
            &browse.needle,
            ImageUsage::offers_emoticon,
            browse.columns,
            media,
        ),
        PackSurface::EmoteSuggestions => suggestion_list(packs, &browse.needle, media),
    }
}

fn sectioned_grid(
    packs: &[ImagePack],
    needle: &str,
    offered: fn(ImageUsage) -> bool,
    columns: i32,
    media: &dyn MediaCache,
) -> PackGrid {
    let per_row = usize::try_from(columns.max(1)).unwrap_or(1);
    let mut grid = PackGrid {
        rows: Vec::new(),
        packs: Vec::new(),
        has_packs: packs
            .iter()
            .any(|pack| pack.images.iter().any(|image| offered(image.usage))),
    };

    for pack in packs {
        let whole_pack = needle.is_empty() || pack_title_matches(pack, needle);
        let cells: Vec<PackCellDto> = pack
            .images
            .iter()
            .filter(|image| offered(image.usage))
            .filter(|image| whole_pack || pack_image_matches(image, needle))
            .map(|image| pack_cell(pack, image, media))
            .collect();
        if cells.is_empty() {
            continue;
        }

        let id = SharedString::from(pack.id.as_ref());
        let (title, untitled) = pack_title(pack);
        let icon = pack_icon(pack, &cells, media);
        grid.packs.push(PackTabDto {
            id: id.clone(),
            title: title.clone(),
            untitled,
            header_row: i32::try_from(grid.rows.len()).unwrap_or(0),
            has_icon: icon.image.is_some(),
            icon: icon.image,
            icon_cell_key: icon.cell_key,
            avatar: icon.avatar,
        });
        grid.rows.push(PackRowDto {
            title,
            untitled,
            pack_id: id.clone(),
            origin: pack.origin,
            is_header: true,
            cells: Vec::new(),
        });
        for chunk in cells.chunks(per_row) {
            grid.rows.push(PackRowDto {
                title: SharedString::new(),
                untitled: false,
                pack_id: id.clone(),
                origin: pack.origin,
                is_header: false,
                cells: chunk.to_vec(),
            });
        }
    }

    grid
}

fn suggestion_list(packs: &[ImagePack], needle: &str, media: &dyn MediaCache) -> PackGrid {
    let (shortcode, wanted_pack) = needle
        .split_once(SLUG_DELIMITER)
        .map_or((needle, None), |(shortcode, pack)| (shortcode, Some(pack)));
    let mut prefixed = Vec::new();
    let mut inner = Vec::new();
    for pack in packs
        .iter()
        .filter(|pack| wanted_pack.is_none_or(|wanted| slug(pack).starts_with(wanted)))
    {
        for image in pack
            .images
            .iter()
            .filter(|image| image.usage.offers_emoticon())
        {
            let lowered = image.shortcode.to_lowercase();
            if lowered.starts_with(shortcode) {
                prefixed.push((pack, image));
            } else if lowered.contains(shortcode) {
                inner.push((pack, image));
            }
        }
    }
    let rows = prefixed
        .into_iter()
        .chain(inner)
        .take(EMOTE_SUGGESTION_LIMIT)
        .map(|(pack, image)| {
            let (title, untitled) = pack_title(pack);
            PackRowDto {
                title,
                untitled,
                pack_id: SharedString::from(pack.id.as_ref()),
                origin: pack.origin,
                is_header: false,
                cells: vec![pack_cell(pack, image, media)],
            }
        })
        .collect();
    PackGrid {
        rows,
        packs: Vec::new(),
        has_packs: packs.iter().any(|pack| {
            pack.images
                .iter()
                .any(|image| image.usage.offers_emoticon())
        }),
    }
}

fn pack_title(pack: &ImagePack) -> (SharedString, bool) {
    match &pack.title {
        PackTitle::Named(name) => (SharedString::from(name), false),
        PackTitle::Personal => (SharedString::new(), true),
    }
}

fn pack_title_matches(pack: &ImagePack, needle: &str) -> bool {
    match &pack.title {
        PackTitle::Named(name) => name.to_lowercase().contains(needle),
        PackTitle::Personal => false,
    }
}

fn pack_icon(pack: &ImagePack, cells: &[PackCellDto], media: &dyn MediaCache) -> PackIcon {
    let first = cells.first();
    let first_cell_key = first.map(|cell| cell.key.clone()).unwrap_or_default();
    let avatar_file = pack
        .avatar
        .as_ref()
        .map(|mxc| (mxc, media.sticker_path(mxc)));
    match avatar_file {
        Some((mxc, path)) if path.is_some() || !media.sticker_failed(mxc) => PackIcon {
            image: load_avatar_async(path.as_deref(), AvatarSlot::Pack(pack.id.to_string())),
            cell_key: SharedString::new(),
            avatar: Some(PackAvatarDto {
                downloading_mxc: path.is_none().then(|| mxc.clone()),
                fallback_cell_key: first_cell_key,
            }),
        },
        _ => PackIcon {
            image: first.and_then(|cell| cell.image.clone()),
            cell_key: first_cell_key,
            avatar: None,
        },
    }
}

fn pack_image_matches(image: &PackImage, needle: &str) -> bool {
    image.shortcode.to_lowercase().contains(needle) || image.body.to_lowercase().contains(needle)
}

pub enum PackArt {
    Ready(Image),
    Failed,
    Decoding,
    Downloading,
}

pub fn pack_art(key: &str, mxc: &str, media: &dyn MediaCache) -> PackArt {
    let path = media.sticker_path(mxc);
    record_pack_cell_need(key, path.as_deref());
    if let Some(path) = path {
        return match peek_thumbnail(&path, &MediaSlot::PackCell(key.to_owned())) {
            Decoded::Ready(decoded) => PackArt::Ready(decoded),
            Decoded::Failed(_) => PackArt::Failed,
            Decoded::Pending => PackArt::Decoding,
        };
    }
    if media.sticker_failed(mxc) {
        PackArt::Failed
    } else {
        PackArt::Downloading
    }
}

fn pack_cell(pack: &ImagePack, image: &PackImage, media: &dyn MediaCache) -> PackCellDto {
    let key = cell_key(&pack.id, &image.shortcode);
    let (art, media_state, awaited_mxc) = match pack_art(&key, &image.mxc, media) {
        PackArt::Ready(decoded) => (Some(decoded), MediaState::Ready, None),
        PackArt::Failed => (None, MediaState::Failed, None),
        PackArt::Decoding => (None, MediaState::Idle, None),
        PackArt::Downloading => (None, MediaState::Idle, Some(image.mxc.clone())),
    };
    PackCellDto {
        key: SharedString::from(&key),
        pack_id: SharedString::from(pack.id.as_ref()),
        shortcode: SharedString::from(&image.shortcode),
        label: SharedString::from(&image.body),
        image: art,
        media_state,
        awaited_mxc,
    }
}
