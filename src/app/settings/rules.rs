use std::future::Future;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::change::{Change, after_echo, after_write, is_wanted};
use crate::app::event::{ActionOutcome, AppEvent, RuleSlot, SettingsEvent};
use crate::app::input::EventSender;
use crate::app::task_group::TaskGroup;
use crate::commands::view::{AccountRule, KeywordRow, NotificationRulesView};
use crate::domain::server_rules::{Keyword, RoomClass, RoomDefault, ServerRules};
use crate::error::Result;
use crate::ports::matrix::ServerRulesPort;

const RULES_CHANNEL_CAP: usize = 4;

struct KeywordChange {
    keyword: String,
    change: Change<bool>,
}

pub(super) struct NotificationRules {
    events: EventSender,
    follow: TaskGroup,
    writes: TaskGroup,
    server: Option<ServerRules>,
    account: Option<Change<bool>>,
    direct: Option<Change<RoomDefault>>,
    group: Option<Change<RoomDefault>>,
    keywords: Vec<KeywordChange>,
    requests: u64,
    showing: bool,
    failed: bool,
}

impl NotificationRules {
    pub(super) fn new(events: EventSender) -> Self {
        Self {
            events,
            follow: TaskGroup::new("notification-rules-follow"),
            writes: TaskGroup::new("notification-rules-writes"),
            server: None,
            account: None,
            direct: None,
            group: None,
            keywords: Vec::new(),
            requests: 0,
            showing: false,
            failed: false,
        }
    }

    pub(super) fn start(&mut self, port: Arc<dyn ServerRulesPort>) {
        self.showing = true;
        self.failed = false;
        self.follow.cancel_and_detach();
        self.follow.spawn_follow(
            &self.events,
            RULES_CHANNEL_CAP,
            move |rules_tx| async move { follow_rules(port.as_ref(), rules_tx).await },
            |rules| AppEvent::Settings(SettingsEvent::RulesChanged(Box::new(rules))),
        );
    }

    pub(super) fn stop(&mut self) {
        self.showing = false;
        self.failed = false;
        self.follow.cancel_and_detach();
    }

    pub(super) fn forget_session(&mut self) {
        self.showing = false;
        self.failed = false;
        self.follow.cancel_and_detach();
        self.writes.cancel_and_detach();
        self.server = None;
        self.account = None;
        self.direct = None;
        self.group = None;
        self.keywords.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.follow.shutdown(), self.writes.shutdown());
    }

    pub(super) fn followed(&mut self, rules: ServerRules) {
        self.account = after_echo(self.account, rules.account_notifications);
        self.direct = after_echo(self.direct, Some(rules.direct));
        self.group = after_echo(self.group, Some(rules.group));
        self.keywords.retain(|pending| {
            pending.change.in_flight()
                || rules.keywords.contains(&pending.keyword) != pending.change.target()
        });
        self.server = Some(rules);
    }

    pub(super) fn set_account(&mut self, port: Arc<dyn ServerRulesPort>, on: bool) {
        let Some(listed) = self
            .server
            .as_ref()
            .and_then(|server| server.account_notifications)
        else {
            tracing::debug!("this account has no notification switch to change");
            return;
        };
        if !is_wanted(self.account, listed, on) {
            return;
        }
        let request = self.issue();
        self.account = Some(Change::Requested {
            request,
            target: on,
        });
        self.spawn_write(request, RuleSlot::Account, async move {
            port.set_account_notifications(on).await
        });
    }

    pub(super) fn set_room_default(
        &mut self,
        port: Arc<dyn ServerRulesPort>,
        rooms: RoomClass,
        mode: RoomDefault,
    ) {
        let Some(listed) = self.server.as_ref().map(|server| match rooms {
            RoomClass::Direct => server.direct,
            RoomClass::Group => server.group,
        }) else {
            return;
        };
        if !is_wanted(*self.default_of(rooms), listed, mode) {
            return;
        }
        let request = self.issue();
        *self.default_of(rooms) = Some(Change::Requested {
            request,
            target: mode,
        });
        self.spawn_write(request, RuleSlot::RoomDefault(rooms), async move {
            port.set_room_default(rooms, mode).await
        });
    }

    pub(super) fn add_keyword(&mut self, port: Arc<dyn ServerRulesPort>, keyword: Keyword) {
        let text = keyword.as_str().to_owned();
        if self.lists(&text) || self.is_changing(&text) {
            return;
        }
        let request = self.note_keyword(&text, true);
        self.spawn_write(request, RuleSlot::Keyword(text), async move {
            port.add_keyword(&keyword).await
        });
    }

    pub(super) fn remove_keyword(&mut self, port: Arc<dyn ServerRulesPort>, keyword: String) {
        if !self.lists(&keyword) || self.is_changing(&keyword) {
            return;
        }
        let request = self.note_keyword(&keyword, false);
        let text = keyword.clone();
        self.spawn_write(request, RuleSlot::Keyword(keyword), async move {
            port.remove_keyword(&text).await
        });
    }

    pub(super) fn written(&mut self, request: u64, slot: &RuleSlot, outcome: ActionOutcome) {
        let server = self.server.as_ref();
        match slot {
            RuleSlot::Account => {
                let listed = server.and_then(|server| server.account_notifications);
                self.account = after_write(self.account, request, outcome, listed);
            }
            RuleSlot::RoomDefault(RoomClass::Direct) => {
                let listed = server.map(|server| server.direct);
                self.direct = after_write(self.direct, request, outcome, listed);
            }
            RuleSlot::RoomDefault(RoomClass::Group) => {
                let listed = server.map(|server| server.group);
                self.group = after_write(self.group, request, outcome, listed);
            }
            RuleSlot::Keyword(keyword) => self.keyword_written(request, keyword, outcome),
        }
    }

    pub(super) fn show_failure(&mut self) -> bool {
        self.failed = self.showing;
        self.showing
    }

    pub(super) fn view(&self) -> NotificationRulesView {
        let Some(server) = &self.server else {
            return NotificationRulesView::default();
        };
        NotificationRulesView {
            known: true,
            failed: self.failed,
            account: server
                .account_notifications
                .map_or(AccountRule::Missing, |listed| {
                    if self.account.map_or(listed, Change::target) {
                        AccountRule::On
                    } else {
                        AccountRule::Off
                    }
                }),
            direct: self.direct.map_or(server.direct, Change::target),
            group: self.group.map_or(server.group, Change::target),
            keywords: self.keyword_rows(server),
        }
    }

    fn issue(&mut self) -> u64 {
        self.failed = false;
        self.requests = self.requests.wrapping_add(1);
        self.requests
    }

    fn default_of(&mut self, rooms: RoomClass) -> &mut Option<Change<RoomDefault>> {
        match rooms {
            RoomClass::Direct => &mut self.direct,
            RoomClass::Group => &mut self.group,
        }
    }

    fn lists(&self, keyword: &str) -> bool {
        self.server
            .as_ref()
            .is_some_and(|server| server.keywords.iter().any(|listed| listed == keyword))
    }

    fn is_changing(&self, keyword: &str) -> bool {
        self.keywords
            .iter()
            .any(|pending| pending.keyword == keyword)
    }

    fn note_keyword(&mut self, keyword: &str, listed: bool) -> u64 {
        let request = self.issue();
        self.keywords.push(KeywordChange {
            keyword: keyword.to_owned(),
            change: Change::Requested {
                request,
                target: listed,
            },
        });
        request
    }

    fn keyword_written(&mut self, request: u64, keyword: &str, outcome: ActionOutcome) {
        let listed = Some(self.lists(keyword));
        self.keywords.retain_mut(|pending| {
            if pending.keyword != keyword {
                return true;
            }
            match after_write(Some(pending.change), request, outcome, listed) {
                Some(change) => {
                    pending.change = change;
                    true
                }
                None => false,
            }
        });
    }

    fn keyword_rows(&self, server: &ServerRules) -> Vec<KeywordRow> {
        let is_going = |keyword: &String| {
            self.keywords
                .iter()
                .any(|pending| pending.keyword == *keyword && !pending.change.target())
        };
        let listed = server
            .keywords
            .iter()
            .filter(|keyword| !is_going(keyword))
            .map(|keyword| KeywordRow {
                keyword: keyword.clone(),
                busy: self.is_changing(keyword),
            });
        let arriving = self
            .keywords
            .iter()
            .filter(|pending| {
                pending.change.target() && !server.keywords.contains(&pending.keyword)
            })
            .map(|pending| KeywordRow {
                keyword: pending.keyword.clone(),
                busy: true,
            });
        listed.chain(arriving).collect()
    }

    fn spawn_write<W>(&mut self, request: u64, slot: RuleSlot, work: W)
    where
        W: Future<Output = Result<()>> + Send + 'static,
    {
        self.writes.spawn_reporting(&self.events, async move {
            let outcome = ActionOutcome::of(work.await, |e| {
                tracing::warn!("a notification rule was not changed: {e}");
            });
            AppEvent::Settings(SettingsEvent::RuleWritten {
                request,
                slot,
                outcome,
            })
        });
    }
}

async fn follow_rules(port: &dyn ServerRulesPort, rules_tx: mpsc::Sender<ServerRules>) {
    if let Err(e) = port.follow(rules_tx).await {
        tracing::warn!("failed to follow this account's notification rules: {e}");
    }
}
