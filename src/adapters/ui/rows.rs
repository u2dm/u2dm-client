use slint::{Model, VecModel};

pub fn patch_rows<T: Clone + 'static>(
    model: &VecModel<T>,
    matches: impl Fn(&T) -> bool,
    apply: impl Fn(&mut T),
) {
    for row in 0..model.row_count() {
        let Some(entry) = model.row_data(row) else {
            continue;
        };
        if matches(&entry) {
            let mut updated = entry;
            apply(&mut updated);
            model.set_row_data(row, updated);
        }
    }
}

pub fn locate_row<T: Clone + 'static>(
    model: &VecModel<T>,
    entry_id: &dyn Fn(&T) -> String,
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
