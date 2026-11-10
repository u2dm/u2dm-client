use async_trait::async_trait;
use tokio::sync::mpsc;

use super::{DemoAuthed, unavailable_if};
use crate::adapters::demo::settings;
use crate::domain::server_rules::{Keyword, RoomClass, RoomDefault, ServerRules};
use crate::error::Result;
use crate::ports::matrix::ServerRulesPort;

async fn may_write_rule() -> Result<()> {
    settings::pause_rule_write().await;
    unavailable_if(settings::rule_writes_fail(), "changing notification rules")
}

#[async_trait]
impl ServerRulesPort for DemoAuthed {
    async fn follow(&self, rules_tx: mpsc::Sender<ServerRules>) -> Result<()> {
        let mut rules = self.server_rules.subscribe();
        loop {
            let now = rules.borrow_and_update().clone();
            if rules_tx.send(now).await.is_err() || rules.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    async fn set_account_notifications(&self, on: bool) -> Result<()> {
        may_write_rule().await?;
        tracing::info!(on, "demo: the account's notifications change");
        self.server_rules
            .echo(move |rules| rules.account_notifications = Some(on));
        Ok(())
    }

    async fn set_room_default(&self, rooms: RoomClass, mode: RoomDefault) -> Result<()> {
        may_write_rule().await?;
        tracing::info!(?rooms, ?mode, "demo: a room default changes");
        self.server_rules.echo(move |rules| match rooms {
            RoomClass::Direct => rules.direct = mode,
            RoomClass::Group => rules.group = mode,
        });
        Ok(())
    }

    async fn add_keyword(&self, keyword: &Keyword) -> Result<()> {
        may_write_rule().await?;
        tracing::info!(keyword = keyword.as_str(), "demo: a keyword rule is added");
        let keyword = keyword.as_str().to_owned();
        self.server_rules
            .echo(move |rules| rules.keywords.push(keyword));
        Ok(())
    }

    async fn remove_keyword(&self, keyword: &str) -> Result<()> {
        may_write_rule().await?;
        tracing::info!(keyword, "demo: a keyword rule is removed");
        let keyword = keyword.to_owned();
        self.server_rules
            .echo(move |rules| rules.keywords.retain(|listed| *listed != keyword));
        Ok(())
    }
}
