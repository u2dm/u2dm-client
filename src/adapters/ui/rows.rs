use std::collections::HashSet;

use slint::Model;

pub fn patch_rows_by_id<T: Clone + 'static>(
    model: &impl Model<Data = T>,
    ids: &HashSet<&str>,
    row_id: &dyn Fn(&T) -> &str,
    apply: impl Fn(&mut T),
) {
    let mut remaining = ids.len();
    for row in 0..model.row_count() {
        if remaining == 0 {
            return;
        }
        let Some(entry) = model.row_data(row) else {
            continue;
        };
        if ids.contains(row_id(&entry)) {
            remaining -= 1;
            let mut updated = entry;
            apply(&mut updated);
            model.set_row_data(row, updated);
        }
    }
}

pub fn patch_rows_where<T: Clone + 'static>(
    model: &impl Model<Data = T>,
    wanted: impl Fn(&T) -> bool,
    apply: impl Fn(&mut T),
) -> bool {
    let mut patched = false;
    for row in 0..model.row_count() {
        let Some(mut entry) = model.row_data(row) else {
            continue;
        };
        if wanted(&entry) {
            apply(&mut entry);
            model.set_row_data(row, entry);
            patched = true;
        }
    }
    patched
}

pub fn patch_first<T: Clone + 'static>(
    model: &impl Model<Data = T>,
    wanted: impl Fn(&T) -> bool,
    apply: impl FnOnce(&mut T),
) -> bool {
    let Some(row) = model.iter().position(|entry| wanted(&entry)) else {
        return false;
    };
    let Some(mut entry) = model.row_data(row) else {
        return false;
    };
    apply(&mut entry);
    model.set_row_data(row, entry);
    true
}

pub fn locate_row<T: Clone + 'static>(
    model: &impl Model<Data = T>,
    entry_id: &dyn Fn(&T) -> &str,
    unique_id: &str,
    hint: usize,
) -> Option<usize> {
    if let Some(entry) = model.row_data(hint)
        && entry_id(&entry) == unique_id
    {
        return Some(hint);
    }
    (0..model.row_count()).find(|&row| {
        model
            .row_data(row)
            .is_some_and(|e| entry_id(&e) == unique_id)
    })
}
