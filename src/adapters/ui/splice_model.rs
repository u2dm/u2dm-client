use std::any::Any;
use std::cell::RefCell;

use slint::{Model, ModelNotify, ModelTracker};

pub struct SpliceModel<T> {
    rows: RefCell<Vec<T>>,
    notify: ModelNotify,
}

impl<T> Default for SpliceModel<T> {
    fn default() -> Self {
        Self {
            rows: RefCell::new(Vec::new()),
            notify: ModelNotify::default(),
        }
    }
}

impl<T: Clone + 'static> SpliceModel<T> {
    pub fn set_vec(&self, rows: Vec<T>) {
        *self.rows.borrow_mut() = rows;
        self.notify.reset();
    }

    pub fn insert_rows(&self, at: usize, rows: Vec<T>) {
        let count = rows.len();
        if count == 0 {
            return;
        }
        let at = {
            let mut all = self.rows.borrow_mut();
            let at = at.min(all.len());
            let tail = all.split_off(at);
            all.extend(rows);
            all.extend(tail);
            at
        };
        self.notify.row_added(at, count);
    }

    pub fn remove(&self, row: usize) -> Option<T> {
        let removed = {
            let mut all = self.rows.borrow_mut();
            (row < all.len()).then(|| all.remove(row))
        };
        if removed.is_some() {
            self.notify.row_removed(row, 1);
        }
        removed
    }

    pub fn truncate(&self, length: usize) {
        let removed = {
            let mut all = self.rows.borrow_mut();
            let removed = all.len().saturating_sub(length);
            all.truncate(length);
            removed
        };
        if removed > 0 {
            self.notify.row_removed(length, removed);
        }
    }
}

impl<T: Clone + 'static> Model for SpliceModel<T> {
    type Data = T;

    fn row_count(&self) -> usize {
        self.rows.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<T> {
        self.rows.borrow().get(row).cloned()
    }

    fn set_row_data(&self, row: usize, data: T) {
        let replaced = self
            .rows
            .borrow_mut()
            .get_mut(row)
            .map(|slot| *slot = data)
            .is_some();
        if replaced {
            self.notify.row_changed(row);
        }
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
