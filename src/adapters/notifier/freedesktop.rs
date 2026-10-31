use std::collections::HashMap;
use std::future;
use std::pin::Pin;

use futures_util::{Stream, StreamExt};
use tokio::sync::mpsc;
use zbus::message::Message;
use zbus::zvariant::Value;
use zbus::{Connection, Proxy};

use crate::config::APP_ID;
use crate::domain::room::RoomId;
use crate::ports::notifier::{
    ActivationSink, DesktopNotice, NoticeAction, NoticeActivation, NotifierPort, RingNotice,
};

const SERVICE: &str = "org.freedesktop.Notifications";
const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
const APP_NAME: &str = "U2DM";
const CATEGORY: &str = "im.received";
const RING_CATEGORY: &str = "call.incoming";
const MESSAGE_SOUND: &str = "message-new-instant";
const RING_SOUND: &str = "phone-incoming-call";
const DEFAULT_ACTION: &str = "default";
const DECLINE_ACTION: &str = "decline";
const ANSWER_ACTION: &str = "answer";
const BODY_MARKUP: &str = "body-markup";
const NORMAL_URGENCY: u8 = 1;
const CRITICAL_URGENCY: u8 = 2;
const SERVER_DEFAULT_TIMEOUT: i32 = -1;
const NEVER_EXPIRES: i32 = 0;
const NO_ICON: &str = "";
const NOTHING_REPLACED: u32 = 0;
const ACTIVATION_TOKEN: &str = "ActivationToken";
const ACTION_INVOKED: &str = "ActionInvoked";
const NOTIFICATION_CLOSED: &str = "NotificationClosed";

enum Request {
    Show(DesktopNotice),
    Withdraw(RoomId),
    WithdrawAll,
    Ring(RingNotice),
    StopRinging(RoomId),
    Follow(ActivationSink),
}

pub(super) struct FreedesktopNotifier {
    requests: mpsc::UnboundedSender<Request>,
}

impl FreedesktopNotifier {
    pub(super) fn new() -> Self {
        let (requests, requests_rx) = mpsc::unbounded_channel();
        tokio::spawn(serve(requests_rx));
        Self { requests }
    }

    fn ask(&self, request: Request) {
        if self.requests.send(request).is_err() {
            tracing::debug!("the desktop notification worker is gone");
        }
    }
}

impl NotifierPort for FreedesktopNotifier {
    fn show(&self, notice: DesktopNotice) {
        self.ask(Request::Show(notice));
    }

    fn withdraw(&self, key: &RoomId) {
        self.ask(Request::Withdraw(key.clone()));
    }

    fn withdraw_all(&self) {
        self.ask(Request::WithdrawAll);
    }

    fn ring(&self, notice: RingNotice) {
        self.ask(Request::Ring(notice));
    }

    fn stop_ringing(&self, key: &RoomId) {
        self.ask(Request::StopRinging(key.clone()));
    }

    fn on_activation(&self, sink: ActivationSink) {
        self.ask(Request::Follow(sink));
    }
}

enum Signal {
    Token { id: u32, token: String },
    Invoked { id: u32, action: String },
    Closed { id: u32 },
}

type Signals = Pin<Box<dyn Stream<Item = Signal> + Send>>;

enum Bus {
    Unopened,
    Open { proxy: Proxy<'static>, markup: bool },
    Unavailable,
}

#[derive(Default)]
struct Shown {
    ids: HashMap<RoomId, u32>,
    rooms: HashMap<u32, RoomId>,
    tokens: HashMap<u32, String>,
}

impl Shown {
    fn replaced_by(&self, key: &RoomId) -> u32 {
        self.ids.get(key).copied().unwrap_or(NOTHING_REPLACED)
    }

    fn record(&mut self, key: RoomId, id: u32) {
        if let Some(previous) = self.ids.insert(key.clone(), id)
            && previous != id
        {
            self.rooms.remove(&previous);
            self.tokens.remove(&previous);
        }
        self.rooms.insert(id, key);
    }

    fn take(&mut self, key: &RoomId) -> Option<u32> {
        let id = self.ids.remove(key)?;
        self.rooms.remove(&id);
        self.tokens.remove(&id);
        Some(id)
    }

    fn forget(&mut self, id: u32) {
        self.tokens.remove(&id);
        if let Some(key) = self.rooms.remove(&id)
            && self.ids.get(&key) == Some(&id)
        {
            self.ids.remove(&key);
        }
    }
}

struct Worker {
    bus: Bus,
    shown: Shown,
    rings: Shown,
    sink: Option<ActivationSink>,
    signals: Option<Signals>,
}

async fn serve(mut requests: mpsc::UnboundedReceiver<Request>) {
    let mut worker = Worker {
        bus: Bus::Unopened,
        shown: Shown::default(),
        rings: Shown::default(),
        sink: None,
        signals: None,
    };
    loop {
        tokio::select! {
            request = requests.recv() => match request {
                Some(request) => worker.handle(request).await,
                None => return,
            },
            signal = next_signal(&mut worker.signals) => match signal {
                Some(signal) => worker.signalled(signal),
                None => worker.signals = None,
            },
        }
    }
}

async fn next_signal(signals: &mut Option<Signals>) -> Option<Signal> {
    match signals.as_mut() {
        Some(signals) => signals.next().await,
        None => future::pending().await,
    }
}

impl Worker {
    async fn handle(&mut self, request: Request) {
        match request {
            Request::Show(notice) => self.show(notice).await,
            Request::Withdraw(key) => {
                if let Some(id) = self.shown.take(&key) {
                    self.close(id).await;
                }
            }
            Request::WithdrawAll => {
                let ids: Vec<u32> = self
                    .shown
                    .rooms
                    .keys()
                    .chain(self.rings.rooms.keys())
                    .copied()
                    .collect();
                self.shown = Shown::default();
                self.rings = Shown::default();
                for id in ids {
                    self.close(id).await;
                }
            }
            Request::Ring(notice) => self.ring(notice).await,
            Request::StopRinging(key) => {
                if let Some(id) = self.rings.take(&key) {
                    self.close(id).await;
                }
            }
            Request::Follow(sink) => self.sink = Some(sink),
        }
    }

    async fn open(&mut self) {
        if !matches!(self.bus, Bus::Unopened) {
            return;
        }
        self.bus = match connect().await {
            Ok(bus) => bus,
            Err(e) => {
                tracing::warn!("desktop notifications are unavailable: {e}");
                Bus::Unavailable
            }
        };
    }

    async fn show(&mut self, notice: DesktopNotice) {
        self.open().await;
        let Bus::Open { proxy, markup } = &self.bus else {
            return;
        };
        let replaces = self.shown.replaced_by(&notice.key);
        let shown = notify(proxy, &notice, replaces, *markup).await;
        match shown {
            Ok(id) => {
                tracing::debug!(room_id = %notice.key, id, replaces, sound = notice.sound, "showed a desktop notification");
                self.shown.record(notice.key, id);
                self.follow_signals().await;
            }
            Err(e) => tracing::warn!("a desktop notification could not be shown: {e}"),
        }
    }

    async fn ring(&mut self, notice: RingNotice) {
        self.open().await;
        let Bus::Open { proxy, markup } = &self.bus else {
            return;
        };
        let replaces = self.rings.replaced_by(&notice.key);
        match notify_ring(proxy, &notice, replaces, *markup).await {
            Ok(id) => {
                tracing::debug!(room_id = %notice.key, id, replaces, sound = notice.sound, "showed a ringing notification");
                self.rings.record(notice.key, id);
                self.follow_signals().await;
            }
            Err(e) => tracing::warn!("a ringing notification could not be shown: {e}"),
        }
    }

    async fn close(&mut self, id: u32) {
        let Bus::Open { proxy, .. } = &self.bus else {
            return;
        };
        let closed: zbus::Result<()> = proxy.call("CloseNotification", &(id,)).await;
        match closed {
            Ok(()) => tracing::debug!(id, "withdrew a desktop notification"),
            Err(e) => tracing::debug!(id, "a desktop notification could not be withdrawn: {e}"),
        }
    }

    async fn follow_signals(&mut self) {
        if self.signals.is_some() {
            return;
        }
        let Bus::Open { proxy, .. } = &self.bus else {
            return;
        };
        match proxy.receive_all_signals().await {
            Ok(messages) => {
                self.signals = Some(Box::pin(
                    messages.filter_map(|message| future::ready(signal_of(&message))),
                ));
            }
            Err(e) => tracing::debug!("not following notification clicks: {e}"),
        }
    }

    fn signalled(&mut self, signal: Signal) {
        match signal {
            Signal::Token { id, token } => {
                if self.shown.rooms.contains_key(&id) {
                    self.shown.tokens.insert(id, token);
                } else if self.rings.rooms.contains_key(&id) {
                    self.rings.tokens.insert(id, token);
                }
            }
            Signal::Invoked { id, action } => self.invoked(id, &action),
            Signal::Closed { id } => {
                self.shown.forget(id);
                self.rings.forget(id);
            }
        }
    }

    fn invoked(&mut self, id: u32, action: &str) {
        let (shown, action) = if self.rings.rooms.contains_key(&id) {
            (&mut self.rings, ring_action(action))
        } else {
            (&mut self.shown, message_action(action))
        };
        let Some(action) = action else {
            return;
        };
        let Some(key) = shown.rooms.get(&id).cloned() else {
            return;
        };
        let token = shown.tokens.remove(&id);
        tracing::debug!(room_id = %key, id, ?action, token = token.is_some(), "a desktop notification was clicked");
        if let Some(sink) = &self.sink {
            sink(NoticeActivation { key, token, action });
        }
    }
}

async fn connect() -> zbus::Result<Bus> {
    let connection = Connection::session().await?;
    let proxy = Proxy::new_owned(connection, SERVICE, OBJECT_PATH, INTERFACE).await?;
    let capabilities: Vec<String> = proxy.call("GetCapabilities", &()).await?;
    let markup = capabilities
        .iter()
        .any(|capability| capability == BODY_MARKUP);
    tracing::debug!(markup, "connected to the desktop notification server");
    Ok(Bus::Open { proxy, markup })
}

async fn notify(
    proxy: &Proxy<'static>,
    notice: &DesktopNotice,
    replaces: u32,
    markup: bool,
) -> zbus::Result<u32> {
    let call = NotifyCall {
        replaces,
        summary: &notice.summary,
        body: &shown_body(&notice.body, markup),
        actions: &[DEFAULT_ACTION, notice.open_label.as_str()],
        hints: hints(
            NORMAL_URGENCY,
            CATEGORY,
            notice.sound.then_some(MESSAGE_SOUND),
        ),
        timeout: SERVER_DEFAULT_TIMEOUT,
    };
    call.send(proxy).await
}

fn message_action(action: &str) -> Option<NoticeAction> {
    (action == DEFAULT_ACTION).then_some(NoticeAction::Open)
}

fn ring_action(action: &str) -> Option<NoticeAction> {
    match action {
        DEFAULT_ACTION => Some(NoticeAction::Open),
        DECLINE_ACTION => Some(NoticeAction::Decline),
        ANSWER_ACTION => Some(NoticeAction::Answer),
        _ => None,
    }
}

async fn notify_ring(
    proxy: &Proxy<'static>,
    notice: &RingNotice,
    replaces: u32,
    markup: bool,
) -> zbus::Result<u32> {
    let mut actions = vec![
        DEFAULT_ACTION,
        notice.open_label.as_str(),
        DECLINE_ACTION,
        notice.decline_label.as_str(),
    ];
    if let Some(answer_label) = &notice.answer_label {
        actions.extend([ANSWER_ACTION, answer_label.as_str()]);
    }
    let call = NotifyCall {
        replaces,
        summary: &notice.summary,
        body: &shown_body(&notice.body, markup),
        actions: &actions,
        hints: hints(
            CRITICAL_URGENCY,
            RING_CATEGORY,
            notice.sound.then_some(RING_SOUND),
        ),
        timeout: NEVER_EXPIRES,
    };
    call.send(proxy).await
}

struct NotifyCall<'a> {
    replaces: u32,
    summary: &'a str,
    body: &'a str,
    actions: &'a [&'a str],
    hints: HashMap<&'static str, Value<'static>>,
    timeout: i32,
}

impl NotifyCall<'_> {
    async fn send(self, proxy: &Proxy<'static>) -> zbus::Result<u32> {
        proxy
            .call(
                "Notify",
                &(
                    APP_NAME,
                    self.replaces,
                    NO_ICON,
                    self.summary,
                    self.body,
                    self.actions,
                    self.hints,
                    self.timeout,
                ),
            )
            .await
    }
}

fn shown_body(body: &str, markup: bool) -> String {
    if markup {
        escape_markup(body)
    } else {
        body.to_owned()
    }
}

fn hints(
    urgency: u8,
    category: &'static str,
    sound: Option<&'static str>,
) -> HashMap<&'static str, Value<'static>> {
    let mut hints = HashMap::new();
    hints.insert("urgency", Value::U8(urgency));
    hints.insert("category", Value::from(category));
    hints.insert("desktop-entry", Value::from(APP_ID));
    match sound {
        Some(sound) => hints.insert("sound-name", Value::from(sound)),
        None => hints.insert("suppress-sound", Value::Bool(true)),
    };
    hints
}

fn signal_of(message: &Message) -> Option<Signal> {
    let header = message.header();
    let member = header.member()?;
    let body = message.body();
    match member.as_str() {
        ACTIVATION_TOKEN => body
            .deserialize::<(u32, String)>()
            .ok()
            .map(|(id, token)| Signal::Token { id, token }),
        ACTION_INVOKED => body
            .deserialize::<(u32, String)>()
            .ok()
            .map(|(id, action)| Signal::Invoked { id, action }),
        NOTIFICATION_CLOSED => body
            .deserialize::<(u32, u32)>()
            .ok()
            .map(|(id, _)| Signal::Closed { id }),
        _ => None,
    }
}

fn escape_markup(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}
