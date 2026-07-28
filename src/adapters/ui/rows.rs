use std::collections::HashSet;

use slint::{Model, VecModel};

pub fn patch_first_row<T: Clone + 'static>(
    model: &VecModel<T>,
    matches: impl Fn(&T) -> bool,
    apply: impl FnOnce(&mut T),
) {
    for row in 0..model.row_count() {
        let Some(entry) = model.row_data(row) else {
            continue;
        };
        if matches(&entry) {
            let mut updated = entry;
            apply(&mut updated);
            model.set_row_data(row, updated);
            return;
        }
    }
}

pub fn patch_rows_by_id<T: Clone + 'static>(
    model: &VecModel<T>,
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

pub fn locate_row<T: Clone + 'static>(
    model: &VecModel<T>,
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
