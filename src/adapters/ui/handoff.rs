use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use super::activation::raise;
use super::backend::UiBackend;
use super::props::{IntProp, StringProp, UiProps};
use crate::ports::handoff::{Handoff, HandoffPort};

pub fn follow_handoffs<B: UiBackend>(window: &B::Window, handoff: &dyn HandoffPort) {
    let weak = window.as_weak();
    handoff.on_handoff(Arc::new(move |handoff: Handoff| {
        let queued = weak.upgrade_in_event_loop(move |window| {
            take_handoff::<B>(&window, &handoff);
        });
        if let Err(e) = queued {
            tracing::debug!("a hand-off could not reach the event loop: {e}");
        }
    }));
}

fn take_handoff<B: UiBackend>(window: &B::Window, handoff: &Handoff) {
    tracing::info!(
        link = handoff.link.is_some(),
        token = handoff.token.is_some(),
        "another launch handed over to this one"
    );
    raise(window.window(), handoff.token.as_deref());
    let Some(link) = handoff.link.as_deref() else {
        return;
    };
    window.set_string(StringProp::ExternalLink, SharedString::from(link));
    let opens = window.get_int(IntProp::ExternalLinkOpens).wrapping_add(1);
    window.set_int(IntProp::ExternalLinkOpens, opens);
    window.window().request_redraw();
}
