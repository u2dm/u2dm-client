use std::sync::Arc;

use slint::ComponentHandle;

use super::activation::raise;
use super::backend::{UiBackend, in_event_loop};
use super::props::{IntProp, StringProp, UiProps};
use crate::ports::handoff::{Handoff, HandoffPort};

pub fn follow_handoffs<B: UiBackend>(window: &B::Window, handoff: &dyn HandoffPort) {
    handoff.on_handoff(Arc::new(in_event_loop::<B, Handoff>(
        window,
        "a hand-off",
        |window, handoff| take_handoff::<B>(window, &handoff),
    )));
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
    window.set_text(StringProp::ExternalLink, link);
    window.bump(IntProp::ExternalLinkOpens);
    window.window().request_redraw();
}
