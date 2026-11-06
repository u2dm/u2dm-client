use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use slint_interpreter::{ComponentHandle, ComponentInstance, Value};

use super::args::{int_arg, string_arg};
use super::callbacks::bind;
use super::names::{emoji_entry, emoji_row, emoji_store};
use super::set_global;
use super::values::{model_value, struct_value, text_splice_value, text_value};
use crate::adapters::ui::emoji;
use crate::error::Result;

fn emoji_entry_to_value(e: &emoji::EmojiEntry) -> Value {
    let tones: Vec<Value> = e.tones.iter().map(text_value).collect();
    struct_value([
        (emoji_entry::BASE, text_value(&e.base)),
        (emoji_entry::TONES, model_value(tones)),
        (emoji_entry::NAME, text_value(&e.name)),
    ])
}

fn emoji_rows_to_value(rows: &[emoji::EmojiRow]) -> Value {
    let rows: Vec<Value> = rows
        .iter()
        .map(|row| {
            let cells: Vec<Value> = row.cells.iter().map(emoji_entry_to_value).collect();
            struct_value([
                (emoji_row::IS_HEADER, Value::Bool(row.is_header)),
                (
                    emoji_row::GROUP,
                    Value::Number(f64::from(u32::try_from(row.group).unwrap_or(0))),
                ),
                (emoji_row::CELLS, model_value(cells)),
            ])
        })
        .collect();
    model_value(rows)
}

fn header_rows_to_value(header_rows: &[usize]) -> Value {
    model_value(
        header_rows
            .iter()
            .map(|row| Value::Number(f64::from(u32::try_from(*row).unwrap_or(0))))
            .collect(),
    )
}

pub(super) fn setup_emoji_store(inst: &ComponentInstance) -> Result<()> {
    let columns = Rc::new(Cell::new(1_usize));
    let laid_out: Rc<RefCell<HashMap<usize, (Value, Value)>>> = Rc::default();

    let weak = inst.as_weak();
    let layout_columns = Rc::clone(&columns);
    bind(inst, emoji_store::NAME, emoji_store::LAYOUT, move |args| {
        let count = int_arg(args, 0)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(1)
            .max(1);
        layout_columns.set(count);
        let (rows, header_rows) = laid_out
            .borrow_mut()
            .entry(count)
            .or_insert_with(|| {
                let laid = emoji::rows(count);
                (
                    emoji_rows_to_value(&laid.rows),
                    header_rows_to_value(&laid.header_rows),
                )
            })
            .clone();
        if let Some(inst) = weak.upgrade() {
            set_global(&inst, emoji_store::NAME, emoji_store::ROWS, rows);
            set_global(
                &inst,
                emoji_store::NAME,
                emoji_store::HEADER_ROWS,
                header_rows,
            );
        }
        Value::Void
    })?;

    let weak = inst.as_weak();
    bind(inst, emoji_store::NAME, emoji_store::SEARCH, move |args| {
        let rows = emoji::result_rows(&string_arg(args, 0), columns.get());
        if let Some(inst) = weak.upgrade() {
            set_global(
                &inst,
                emoji_store::NAME,
                emoji_store::RESULT_ROWS,
                emoji_rows_to_value(&rows),
            );
        }
        Value::Void
    })?;

    bind(inst, emoji_store::NAME, emoji_store::INSERT, |args| {
        let (inserted, caret) = emoji::insert_at(
            &string_arg(args, 0),
            int_arg(args, 1).unwrap_or_default(),
            &string_arg(args, 2),
        );
        text_splice_value(inserted, caret)
    })
}
