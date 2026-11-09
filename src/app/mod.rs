mod account;
mod active_timeline;
mod attachments;
mod audio;
mod avatars;
mod calls;
mod conclude;
mod credentials;
mod encryption;
mod establish;
mod event;
mod event_source;
mod identity_alerts;
pub mod input;
mod invite_people;
mod invites;
mod lifecycle;
mod link_requests;
mod media;
mod mentions;
mod message_actions;
mod notifications;
mod packs;
mod pinned;
mod polls;
mod presence;
mod readers;
mod recover;
mod removals;
mod room_creation;
mod room_directory;
mod room_info;
mod room_log;
mod room_preview;
mod selection;
mod send_lanes;
mod service;
mod session;
mod session_requests;
mod settings;
mod space_index;
mod space_leave;
mod space_order;
mod submissions;
mod task_group;
mod typing;
mod uia;
mod upgrades;
mod user_info;
mod verification;
mod video;

pub(crate) use service::AppService;

use crate::commands::messages::UserMessage;
use crate::commands::view::Toast;
use crate::ports::output::AppOutputPort;

pub(super) fn show_toast(output: &dyn AppOutputPort, toast: Toast) {
    output.publish(Box::new(move |view| view.toast = toast));
}

pub(super) fn report_in(
    output: &dyn AppOutputPort,
    shown: Option<&mut UserMessage>,
    message: UserMessage,
) {
    match shown {
        Some(error) => *error = message,
        None => show_toast(output, Toast::Error(message)),
    }
}
