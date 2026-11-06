use std::ops::ControlFlow;

use slint::Model;

use super::UiBackend;
use super::avatars::indexed_timeline_row;
use crate::adapters::ui::fields::MessageFields;
use crate::adapters::ui::present::room_log_text;
use crate::adapters::ui::props::{IntProp, StringProp, UiProps};
use crate::adapters::ui::reduce::is_latest_adoption;
use crate::adapters::ui::router::EditedRow;
use crate::adapters::ui::rows::row_index;
use crate::adapters::ui::session::from_snapshot;
use crate::domain::room::RoomId;

pub fn selected_room_key<B: UiBackend>(weak: &slint::Weak<B::Window>) -> Option<(RoomId, i32)> {
    let w = weak.upgrade()?;
    let room_id = w.get_string(StringProp::SelectedRoomId).to_string();
    if room_id.is_empty() {
        return None;
    }
    Some((RoomId::new(room_id), w.get_int(IntProp::SelectedGeneration)))
}

pub fn adopted_room_key<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    seen_timeline_token: i32,
) -> Option<(RoomId, i32)> {
    selected_room_key::<B>(weak)
        .filter(|(_, generation)| is_latest_adoption(*generation, seen_timeline_token))
}

pub fn unread_below<B: UiBackend>(first_unseen_row: i32) -> u32 {
    let from = usize::try_from(first_unseen_row).unwrap_or(0);
    let below = B::with_timeline(|timeline| {
        timeline.count_tail(from, |entry: &B::Message| entry.counts_as_unread())
    });
    u32::try_from(below).unwrap_or(u32::MAX)
}

pub fn last_editable_row<B: UiBackend>() -> i32 {
    row_index(B::with_timeline(|timeline| {
        timeline.last_position(|entry: &B::Message| entry.text_editable())
    }))
}

pub const RECENT_SPEAKERS: usize = 12;

pub fn recent_speakers<B: UiBackend>(limit: usize) -> Vec<String> {
    let mut speakers: Vec<String> = Vec::new();
    B::with_timeline(|timeline| {
        timeline.visit_newest_first(|entry: &B::Message| {
            let speaker = entry.sender_id();
            if entry.counts_as_unread() && !speakers.iter().any(|known| known == speaker) {
                speakers.push(speaker.to_owned());
            }
            if speakers.len() >= limit {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
    });
    speakers
}

pub fn row_of_message<B: UiBackend>(event_id: &str, local_id: &str) -> i32 {
    let names = |entry: &B::Message| {
        (!event_id.is_empty() && entry.event_id() == event_id)
            || (!local_id.is_empty() && entry.local_id() == local_id)
    };
    row_index(B::with_timeline(|timeline| timeline.last_position(names)))
}

pub fn shown_room_log_text() -> String {
    from_snapshot(|view| view.room_log.clone())
        .flatten()
        .map(|shown| room_log_text(&shown))
        .unwrap_or_default()
}

pub fn current_edited_row<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    unique_id: &str,
    timeline_token: i32,
) -> Option<EditedRow> {
    if unique_id.is_empty() {
        return None;
    }
    let w = weak.upgrade()?;
    if w.get_int(IntProp::TimelineToken) != timeline_token {
        return None;
    }
    B::with_timeline(|timeline| {
        let entry = timeline.row_data(indexed_timeline_row::<B>(timeline, unique_id)?)?;
        Some(EditedRow {
            event_id: entry.event_id().to_owned(),
            local_id: entry.local_id().to_owned(),
        })
    })
}
