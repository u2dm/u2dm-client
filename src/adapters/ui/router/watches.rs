use tokio::sync::watch;

use super::{RoomKey, optional_room};
use crate::commands::ui::{
    ComposerActivity, TimelineVisibility, UserActivity, ViewportChanged, WindowAttention,
    WindowFocus,
};

pub fn scroll_position(
    scroll_tx: &watch::Sender<ViewportChanged>,
    key: RoomKey,
    at_bottom: bool,
    unread_below: u32,
) {
    let Some((room_id, generation)) = key else {
        return;
    };
    let update = ViewportChanged {
        room_id,
        generation,
        at_bottom,
        unread_below,
    };
    if scroll_tx.send(update).is_err() {
        tracing::debug!("scroll position receiver closed");
    }
}

pub fn timeline_visibility(visibility_tx: &watch::Sender<TimelineVisibility>, visible: bool) {
    let visibility = if visible {
        TimelineVisibility::Visible
    } else {
        TimelineVisibility::Hidden
    };
    if visibility_tx.send(visibility).is_err() {
        tracing::debug!("timeline visibility receiver closed");
    }
}

pub fn window_focus(attention_tx: &watch::Sender<WindowAttention>, attended: bool) {
    let focus = if attended {
        WindowFocus::Focused
    } else {
        WindowFocus::Unfocused
    };
    attention_tx.send_if_modified(|attention| {
        let changed = attention.focus != focus;
        attention.focus = focus;
        changed
    });
}

pub fn user_activity(attention_tx: &watch::Sender<WindowAttention>, active: bool) {
    let activity = if active {
        UserActivity::Active
    } else {
        UserActivity::Idle
    };
    attention_tx.send_if_modified(|attention| {
        let changed = attention.activity != activity;
        attention.activity = activity;
        changed
    });
}

pub fn composer_changed(
    composer_tx: &watch::Sender<ComposerActivity>,
    room_id: String,
    composing: bool,
    edits: i32,
) {
    let activity = ComposerActivity {
        room_id: optional_room(room_id),
        composing,
        edits,
    };
    if composer_tx.send(activity).is_err() {
        tracing::debug!("composer activity receiver closed");
    }
}
