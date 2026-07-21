use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};

use crate::commands::{AppViewState, Effect};
use crate::ports::output::{AppOutputPort, ViewMutation};

pub struct UiEventOutput {
    ui_tx: mpsc::Sender<Effect>,
    view_tx: watch::Sender<Arc<AppViewState>>,
}

impl UiEventOutput {
    pub fn new(ui_tx: mpsc::Sender<Effect>, view_tx: watch::Sender<Arc<AppViewState>>) -> Self {
        Self { ui_tx, view_tx }
    }
}

#[async_trait]
impl AppOutputPort for UiEventOutput {
    fn publish(&self, mutate: ViewMutation) {
        self.view_tx.send_modify(|snapshot| {
            let mut next = (**snapshot).clone();
            mutate(&mut next);
            *snapshot = Arc::new(next);
        });
    }

    async fn emit(&self, effect: Effect) {
        if let Err(e) = self.ui_tx.send(effect).await {
            tracing::debug!("failed to send UI effect: {e}");
        }
    }

    fn emit_now(&self, effect: Effect) {
        if let Err(e) = self.ui_tx.try_send(effect) {
            tracing::debug!("failed to send UI effect: {e}");
        }
    }
}
