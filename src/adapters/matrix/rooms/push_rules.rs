use std::sync::Arc;

use async_trait::async_trait;
use matrix_sdk::Client;
use matrix_sdk::notification_settings::{
    IsEncrypted, IsOneToOne, NotificationSettings, RoomNotificationMode,
};
use matrix_sdk::ruma::push::{PredefinedOverrideRuleId, RuleKind};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::adapters::matrix::session::ClientHandle;
use crate::domain::server_rules::{Keyword, RoomClass, RoomDefault, ServerRules};
use crate::error::{AppError, Result};
use crate::ports::matrix::ServerRulesPort;

const EVERY_ENCRYPTION: [IsEncrypted; 2] = [IsEncrypted::Yes, IsEncrypted::No];

fn one_to_one(rooms: RoomClass) -> IsOneToOne {
    IsOneToOne::from(rooms == RoomClass::Direct)
}

async fn room_default(settings: &NotificationSettings, rooms: RoomClass) -> RoomDefault {
    for encrypted in EVERY_ENCRYPTION {
        let mode = settings
            .get_default_room_notification_mode(encrypted, one_to_one(rooms))
            .await;
        if mode != RoomNotificationMode::AllMessages {
            return RoomDefault::MentionsAndKeywords;
        }
    }
    RoomDefault::AllMessages
}

async fn read_rules(settings: &NotificationSettings) -> ServerRules {
    let muted = settings
        .is_push_rule_enabled(RuleKind::Override, PredefinedOverrideRuleId::Master)
        .await;
    ServerRules {
        account_notifications: muted.ok().map(|muted| !muted),
        direct: room_default(settings, RoomClass::Direct).await,
        group: room_default(settings, RoomClass::Group).await,
        keywords: settings.enabled_keywords().await.into_iter().collect(),
    }
}

pub(in crate::adapters::matrix) async fn follow_rules(
    client: &Client,
    rules_tx: mpsc::Sender<ServerRules>,
) {
    let settings = client.notification_settings().await;
    let mut changes = settings.subscribe_to_changes();
    loop {
        if rules_tx.send(read_rules(&settings).await).await.is_err() {
            return;
        }
        if matches!(changes.recv().await, Err(RecvError::Closed)) {
            return;
        }
    }
}

pub(in crate::adapters::matrix) async fn write_account_notifications(
    client: &Client,
    on: bool,
) -> Result<()> {
    client
        .notification_settings()
        .await
        .set_push_rule_enabled(RuleKind::Override, PredefinedOverrideRuleId::Master, !on)
        .await
        .map_err(AppError::other)
}

pub(in crate::adapters::matrix) async fn write_room_default(
    client: &Client,
    rooms: RoomClass,
    mode: RoomDefault,
) -> Result<()> {
    let settings = client.notification_settings().await;
    let target = match mode {
        RoomDefault::AllMessages => RoomNotificationMode::AllMessages,
        RoomDefault::MentionsAndKeywords => RoomNotificationMode::MentionsAndKeywordsOnly,
    };
    for encrypted in EVERY_ENCRYPTION {
        settings
            .set_default_room_notification_mode(encrypted, one_to_one(rooms), target)
            .await
            .map_err(AppError::other)?;
    }
    Ok(())
}

pub(in crate::adapters::matrix) async fn write_keyword_added(
    client: &Client,
    keyword: &Keyword,
) -> Result<()> {
    client
        .notification_settings()
        .await
        .add_keyword(keyword.as_str().to_owned())
        .await
        .map_err(AppError::other)
}

pub(in crate::adapters::matrix) async fn write_keyword_removed(
    client: &Client,
    keyword: &str,
) -> Result<()> {
    client
        .notification_settings()
        .await
        .remove_keyword(keyword)
        .await
        .map_err(AppError::other)
}

pub(in crate::adapters::matrix) struct MatrixServerRules {
    matrix: Arc<ClientHandle>,
}

impl MatrixServerRules {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

#[async_trait]
impl ServerRulesPort for MatrixServerRules {
    async fn follow(&self, rules_tx: mpsc::Sender<ServerRules>) -> Result<()> {
        follow_rules(&self.matrix.client().await?, rules_tx).await;
        Ok(())
    }

    async fn set_account_notifications(&self, on: bool) -> Result<()> {
        write_account_notifications(&self.matrix.client().await?, on).await
    }

    async fn set_room_default(&self, rooms: RoomClass, mode: RoomDefault) -> Result<()> {
        write_room_default(&self.matrix.client().await?, rooms, mode).await
    }

    async fn add_keyword(&self, keyword: &Keyword) -> Result<()> {
        write_keyword_added(&self.matrix.client().await?, keyword).await
    }

    async fn remove_keyword(&self, keyword: &str) -> Result<()> {
        write_keyword_removed(&self.matrix.client().await?, keyword).await
    }
}
