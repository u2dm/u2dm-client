use std::sync::Arc;

use async_trait::async_trait;

use super::journal::{Journal, Selected};
use crate::commands::effects::Effect;
use crate::commands::view::AppViewState;
use crate::ports::output::{AppOutputPort, ViewMutation};

pub struct ProbeOutput {
    inner: Arc<dyn AppOutputPort>,
    journal: Arc<Journal>,
    selected: Arc<Selected>,
}

impl ProbeOutput {
    pub fn new(
        inner: Arc<dyn AppOutputPort>,
        journal: Arc<Journal>,
        selected: Arc<Selected>,
    ) -> Self {
        Self {
            inner,
            journal,
            selected,
        }
    }
}

#[async_trait]
impl AppOutputPort for ProbeOutput {
    fn publish(&self, mutate: ViewMutation) {
        self.inner.publish(mutate);
    }

    fn replace(&self, state: AppViewState) {
        self.inner.replace(state);
    }

    async fn emit(&self, effect: Effect) {
        self.journal.record_effect(&effect);
        self.selected.observe(&effect);
        self.inner.emit(effect).await;
    }
}
