use std::fmt;

use tokio::sync::mpsc;

use super::event::AppEvent;
#[cfg(feature = "demo")]
use super::event::SessionEvent;
use crate::commands::ui::UiCommand;

pub(super) enum Input {
    Ui(UiCommand),
    Internal(AppEvent),
}

impl Input {
    fn label(&self) -> String {
        match self {
            Self::Ui(cmd) => cmd.to_string(),
            Self::Internal(event) => event.label().to_owned(),
        }
    }
}

pub struct Closed;

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the app inbox is closed")
    }
}

fn deliver(tx: &mpsc::UnboundedSender<Input>, input: Input) -> Result<(), Closed> {
    match tx.send(input) {
        Ok(()) => Ok(()),
        Err(mpsc::error::SendError(dropped)) => {
            tracing::debug!(input = %dropped.label(), "the app inbox is closed; dropping input");
            Err(Closed)
        }
    }
}

#[derive(Clone)]
pub struct CommandSender {
    tx: mpsc::UnboundedSender<Input>,
}

impl CommandSender {
    pub fn send(&self, cmd: UiCommand) -> Result<(), Closed> {
        deliver(&self.tx, Input::Ui(cmd))
    }

    #[cfg(feature = "demo")]
    pub fn inject_session_expiry(&self) -> Result<(), Closed> {
        deliver(
            &self.tx,
            Input::Internal(AppEvent::Session(SessionEvent::Expired)),
        )
    }

    #[cfg(feature = "demo")]
    pub fn inject_soft_logout(&self) -> Result<(), Closed> {
        deliver(
            &self.tx,
            Input::Internal(AppEvent::Session(SessionEvent::Suspended)),
        )
    }

    pub(super) fn events(&self) -> EventSender {
        EventSender {
            tx: self.tx.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct EventSender {
    tx: mpsc::UnboundedSender<Input>,
}

impl EventSender {
    pub(super) fn send(&self, event: AppEvent) -> Result<(), Closed> {
        deliver(&self.tx, Input::Internal(event))
    }
}

pub struct Inbox {
    rx: mpsc::UnboundedReceiver<Input>,
}

impl Inbox {
    pub(super) async fn recv(&mut self) -> Option<Input> {
        self.rx.recv().await
    }
}

pub fn channel() -> (CommandSender, Inbox) {
    let (tx, rx) = mpsc::unbounded_channel::<Input>();
    (CommandSender { tx }, Inbox { rx })
}
