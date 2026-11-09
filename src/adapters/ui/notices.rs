use std::sync::Arc;

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::UserAttentionType;
use slint::{ComponentHandle, Window};

use super::activation::raise;
use super::backend::{UiBackend, in_event_loop};
use super::dto::preview_line;
use super::present::{ServiceKind, service_kind, service_target};
use super::props::{IntProp, StringProp, UiProps};
use crate::commands::view::{AppViewState, NotificationsView, ShownNotice};
use crate::domain::member_name::NamedUser;
use crate::domain::message::MessagePreviewKind;
use crate::domain::push::{NoticeContent, PushNotice};
use crate::ports::notifier::{DesktopNotice, NoticeAction, NoticeActivation, NotifierPort};

pub enum NoticeWords<'a> {
    Message {
        kind: MessagePreviewKind,
        body: &'a str,
        sender: &'a str,
    },
    HiddenMessage,
    Service {
        kind: ServiceKind,
        actor: &'a str,
        target: &'a str,
    },
    Invite {
        inviter: &'a str,
    },
    Ring {
        video: bool,
        caller: &'a str,
    },
}

pub fn apply_notifications<B: UiBackend>(
    w: &B::Window,
    last_view: Option<&AppViewState>,
    next: &NotificationsView,
    notifier: &dyn NotifierPort,
) {
    let last = last_view.map(|view| &view.notifications);
    if last.is_some_and(|last| Arc::ptr_eq(&last.shown, &next.shown)) {
        return;
    }
    let before = last.map_or(&[][..], |last| &last.shown[..]);
    for gone in before.iter().filter(|gone| !lists_room(&next.shown, gone)) {
        notifier.withdraw(&gone.notice.room_id);
    }
    let fresh: Vec<&ShownNotice> = next
        .shown
        .iter()
        .filter(|shown| !before.iter().any(|old| old.serial == shown.serial))
        .collect();
    for shown in &fresh {
        notifier.show(compose::<B>(w, &shown.notice));
    }
    if !fresh.is_empty() {
        let highlight = fresh.iter().any(|shown| shown.notice.tweaks.highlight);
        ask_for_attention(w.window(), highlight);
    }
}

fn lists_room(shown: &[ShownNotice], notice: &ShownNotice) -> bool {
    shown
        .iter()
        .any(|other| other.notice.room_id == notice.notice.room_id)
}

fn compose<B: UiBackend>(w: &B::Window, notice: &PushNotice) -> DesktopNotice {
    let body = match &notice.content {
        NoticeContent::Message { sender, kind, body } => {
            let preview = preview_line(body);
            let sender = sender.as_ref().map(NamedUser::shown).unwrap_or_default();
            B::notification_text(
                w,
                &NoticeWords::Message {
                    kind: *kind,
                    body: &preview,
                    sender: &sender,
                },
            )
        }
        NoticeContent::HiddenMessage => B::notification_text(w, &NoticeWords::HiddenMessage),
        NoticeContent::Service { actor, event } => B::notification_text(
            w,
            &NoticeWords::Service {
                kind: service_kind(event),
                actor: &actor.shown(),
                target: &service_target(event),
            },
        ),
        NoticeContent::Invite { inviter } => B::notification_text(
            w,
            &NoticeWords::Invite {
                inviter: &inviter.as_ref().map(NamedUser::shown).unwrap_or_default(),
            },
        ),
    };
    DesktopNotice {
        key: notice.room_id.clone(),
        summary: notice.room_name.clone(),
        body: body.to_string(),
        open_label: w.get_string(StringProp::NotificationOpenLabel).to_string(),
        sound: notice.tweaks.sound,
    }
}

fn ask_for_attention(window: &Window, highlight: bool) {
    let attention = if highlight {
        UserAttentionType::Critical
    } else {
        UserAttentionType::Informational
    };
    let asked = window
        .with_winit_window(|winit| winit.request_user_attention(Some(attention)))
        .is_some();
    tracing::debug!(highlight, asked, "asked for the user's attention");
}

pub fn follow_activations<B: UiBackend>(window: &B::Window, notifier: &dyn NotifierPort) {
    notifier.on_activation(Arc::new(in_event_loop::<B, NoticeActivation>(
        window,
        "a notification click",
        |window, activation| act_on_notification::<B>(window, &activation),
    )));
}

pub fn act_on_notification<B: UiBackend>(window: &B::Window, activation: &NoticeActivation) {
    match activation.action {
        NoticeAction::Open => open_from_notification::<B>(window, activation),
        NoticeAction::Answer => {
            open_from_notification::<B>(window, activation);
            answer_from_notification(window, activation);
        }
        NoticeAction::Decline => decline_from_notification(window, activation),
    }
}

fn answer_from_notification(window: &impl UiProps, activation: &NoticeActivation) {
    tracing::debug!(room_id = %activation.key, "a ringing notification was answered");
    window.set_text(StringProp::CallRoom, activation.key.as_ref());
    window.bump(IntProp::CallAnswers);
}

fn decline_from_notification(window: &impl UiProps, activation: &NoticeActivation) {
    tracing::debug!(room_id = %activation.key, "a ringing notification was declined");
    window.set_text(StringProp::CallRoom, activation.key.as_ref());
    window.bump(IntProp::CallDeclines);
}

fn open_from_notification<B: UiBackend>(window: &B::Window, activation: &NoticeActivation) {
    tracing::debug!(room_id = %activation.key, token = activation.token.is_some(), "a notification was clicked");
    raise(window.window(), activation.token.as_deref());
    window.set_text(StringProp::NotificationRoom, activation.key.as_ref());
    window.bump(IntProp::NotificationOpens);
    window.window().request_redraw();
}
