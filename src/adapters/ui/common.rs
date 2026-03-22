use std::collections::HashMap;

use slint::{Model, VecModel};

use crate::domain::models::{Room, TimelineMessage, TimelinePatch};

pub enum Status {
    CheckingServer,
    LoggingIn,
    OpeningBrowser,
    FileSaved,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CheckingServer => "checking-server",
            Self::LoggingIn => "logging-in",
            Self::OpeningBrowser => "opening-browser",
            Self::FileSaved => "file-saved",
        }
    }
}

pub enum LoginStep {
    Homeserver,
    Credentials,
    LoggedIn,
}

impl LoginStep {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Homeserver => "homeserver",
            Self::Credentials => "credentials",
            Self::LoggedIn => "logged-in",
        }
    }
}

pub enum VerifyStep {
    Requested,
    Emojis,
    Confirming,
    Done,
    Cancelled,
}

impl VerifyStep {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Emojis => "emojis",
            Self::Confirming => "confirming",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

pub fn apply_timeline_patch<T: Clone + 'static>(
    model: &VecModel<T>,
    patch: TimelinePatch,
    convert: &dyn Fn(&TimelineMessage) -> T,
) {
    match patch {
        TimelinePatch::Reset(messages) => {
            let entries: Vec<T> = messages.iter().map(convert).collect();
            model.set_vec(entries);
        }
        TimelinePatch::Append(messages) => {
            for m in &messages {
                model.push(convert(m));
            }
        }
        TimelinePatch::PushFront(m) => {
            model.insert(0, convert(&m));
        }
        TimelinePatch::PushBack(m) => {
            model.push(convert(&m));
        }
        TimelinePatch::Insert { index, message } => {
            let idx = index.min(model.row_count());
            model.insert(idx, convert(&message));
        }
        TimelinePatch::Set { index, message } => {
            if index < model.row_count() {
                model.set_row_data(index, convert(&message));
            }
        }
        TimelinePatch::Remove { index } => {
            if index < model.row_count() {
                model.remove(index);
            }
        }
        TimelinePatch::PopFront => {
            if model.row_count() > 0 {
                model.remove(0);
            }
        }
        TimelinePatch::PopBack => {
            let count = model.row_count();
            if count > 0 {
                model.remove(count - 1);
            }
        }
        TimelinePatch::Truncate { length } => {
            while model.row_count() > length {
                model.remove(model.row_count() - 1);
            }
        }
        TimelinePatch::Clear => {
            model.set_vec(Vec::new());
        }
        TimelinePatch::Batch(patches) => {
            for p in patches {
                apply_timeline_patch(model, p, convert);
            }
        }
    }
}

pub fn apply_rooms<T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    rooms: &[Room],
    convert: &dyn Fn(&Room) -> T,
    get_id: &dyn Fn(&T) -> &str,
) {
    let new_by_id: HashMap<&str, (usize, &Room)> = rooms
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id.as_ref(), (i, r)))
        .collect();

    let mut i = 0;
    while i < model.row_count() {
        let keep = model
            .row_data(i)
            .is_some_and(|entry| new_by_id.contains_key(get_id(&entry)));

        if keep {
            i += 1;
        } else {
            model.remove(i);
        }
    }

    for idx in 0..rooms.len() {
        let Some(room) = rooms.get(idx) else { continue };
        let new_entry = convert(room);

        if idx < model.row_count() {
            let same_id = model
                .row_data(idx)
                .is_some_and(|entry| get_id(&entry) == &*room.id);

            if same_id {
                if model.row_data(idx).as_ref() != Some(&new_entry) {
                    model.set_row_data(idx, new_entry);
                }
            } else {
                model.insert(idx, new_entry);
            }
        } else {
            model.push(new_entry);
        }
    }

    while model.row_count() > rooms.len() {
        model.remove(model.row_count() - 1);
    }
}
