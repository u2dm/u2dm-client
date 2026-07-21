use async_trait::async_trait;

use crate::commands::{AppViewState, Effect};

pub type ViewMutation = Box<dyn FnOnce(&mut AppViewState) + Send>;

#[async_trait]
pub trait AppOutputPort: Send + Sync {
    fn publish(&self, mutate: ViewMutation);
    async fn emit(&self, effect: Effect);
    fn emit_now(&self, effect: Effect);
}
