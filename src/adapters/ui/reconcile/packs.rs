use std::collections::HashMap;
use std::mem;

use slint::SharedString;

use crate::adapters::ui::dto::{PackGrid, PackRowDto};
use crate::adapters::ui::session::with_session;
use crate::domain::image_pack::PackOrigin;

#[derive(Default)]
pub struct PackIndex {
    row_of_cell: HashMap<String, usize>,
    row_of_pack: HashMap<String, usize>,
    row_shapes: Vec<PackRowShape>,
    awaited_downloads: HashMap<String, String>,
    pack_avatars: HashMap<String, PackAvatarWatch>,
}

struct PackAvatarWatch {
    downloading_mxc: Option<String>,
    fallback_cell_key: String,
}

#[derive(PartialEq)]
struct PackRowShape {
    is_header: bool,
    title: SharedString,
    untitled: bool,
    pack_id: SharedString,
    origin: PackOrigin,
    cell_keys: Vec<SharedString>,
}

impl PackRowShape {
    fn of(row: &PackRowDto) -> Self {
        Self {
            is_header: row.is_header,
            title: row.title.clone(),
            untitled: row.untitled,
            pack_id: row.pack_id.clone(),
            origin: row.origin,
            cell_keys: row.cells.iter().map(|cell| cell.key.clone()).collect(),
        }
    }
}

pub struct RowReplacements(Vec<bool>);

impl RowReplacements {
    pub fn must_replace(&self, row: usize) -> bool {
        self.0.get(row).copied().unwrap_or(true)
    }
}

pub fn index_pack_grid(grid: &PackGrid) -> RowReplacements {
    with_session(|session| {
        let index = &mut session.pack_index;
        let shapes: Vec<PackRowShape> = grid.rows.iter().map(PackRowShape::of).collect();
        let replaced = RowReplacements(
            shapes
                .iter()
                .enumerate()
                .map(|(row, shape)| index.row_shapes.get(row) != Some(shape))
                .collect(),
        );
        let mut previously_awaited = mem::take(&mut index.awaited_downloads);
        index.row_of_cell.clear();
        index.row_of_pack.clear();
        for (row, entry) in grid.rows.iter().enumerate() {
            for cell in &entry.cells {
                index.row_of_cell.insert(cell.key.to_string(), row);
                let awaited = if replaced.must_replace(row) {
                    cell.awaited_mxc.clone()
                } else {
                    previously_awaited.remove(cell.key.as_str())
                };
                if let Some(mxc) = awaited {
                    index.awaited_downloads.insert(cell.key.to_string(), mxc);
                }
            }
        }
        index.pack_avatars.clear();
        for (row, pack) in grid.packs.iter().enumerate() {
            index.row_of_pack.insert(pack.id.to_string(), row);
            if let Some(avatar) = &pack.avatar {
                let watch = PackAvatarWatch {
                    downloading_mxc: avatar.downloading_mxc.clone(),
                    fallback_cell_key: avatar.fallback_cell_key.to_string(),
                };
                index.pack_avatars.insert(pack.id.to_string(), watch);
            }
        }
        index.row_shapes = shapes;
        replaced
    })
}

pub fn retain_awaited_downloads(mut still_awaited: impl FnMut(&str, &str) -> bool) {
    let mut awaited = with_session(|session| mem::take(&mut session.pack_index.awaited_downloads));
    awaited.retain(|key, mxc| still_awaited(key, mxc));
    with_session(|session| session.pack_index.awaited_downloads = awaited);
}

pub fn shows_pack_avatar(pack_id: &str) -> bool {
    with_session(|session| session.pack_index.pack_avatars.contains_key(pack_id))
}

pub fn forget_pack_avatar(pack_id: &str) -> Option<String> {
    with_session(|session| session.pack_index.pack_avatars.remove(pack_id))
        .map(|watch| watch.fallback_cell_key)
}

pub fn downloading_pack_avatars() -> Vec<(String, String)> {
    with_session(|session| {
        session
            .pack_index
            .pack_avatars
            .iter()
            .filter_map(|(pack, watch)| Some((pack.clone(), watch.downloading_mxc.clone()?)))
            .collect()
    })
}

pub fn pack_avatar_downloaded(pack_id: &str) {
    with_session(|session| {
        if let Some(watch) = session.pack_index.pack_avatars.get_mut(pack_id) {
            watch.downloading_mxc = None;
        }
    });
}

pub fn pack_cell_row(key: &str) -> Option<usize> {
    with_session(|session| session.pack_index.row_of_cell.get(key).copied())
}

pub fn pack_tab_row(pack_id: &str) -> Option<usize> {
    with_session(|session| session.pack_index.row_of_pack.get(pack_id).copied())
}
