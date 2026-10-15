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
use crate::ports::notifier::{ActivationSink, DesktopNotice, NoticeActivation, NotifierPort};

const SERVICE: &str = "org.freedesktop.Notifications";
const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
const APP_NAME: &str = "U2DM";
const CATEGORY: &str = "im.received";
const MESSAGE_SOUND: &str = "message-new-instant";
const DEFAULT_ACTION: &str = "default";
const BODY_MARKUP: &str = "body-markup";
const NORMAL_URGENCY: u8 = 1;
const SERVER_DEFAULT_TIMEOUT: i32 = -1;
const NO_ICON: &str = "";
const NOTHING_REPLACED: u32 = 0;
const ACTIVATION_TOKEN: &str = "ActivationToken";
const ACTION_INVOKED: &str = "ActionInvoked";
const NOTIFICATION_CLOSED: &str = "NotificationClosed";

enum Request {
    Show(DesktopNotice),
    Withdraw(RoomId),
    WithdrawAll,
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
    sink: Option<ActivationSink>,
    signals: Option<Signals>,
}

async fn serve(mut requests: mpsc::UnboundedReceiver<Request>) {
    let mut worker = Worker {
        bus: Bus::Unopened,
        shown: Shown::default(),
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
                let ids: Vec<u32> = self.shown.rooms.keys().copied().collect();
                self.shown = Shown::default();
                for id in ids {
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
                }
            }
            Signal::Invoked { id, action } => self.invoked(id, &action),
            Signal::Closed { id } => self.shown.forget(id),
        }
    }

    fn invoked(&mut self, id: u32, action: &str) {
        if action != DEFAULT_ACTION {
            return;
        }
        let Some(key) = self.shown.rooms.get(&id).cloned() else {
            return;
        };
        let token = self.shown.tokens.remove(&id);
        tracing::debug!(room_id = %key, id, token = token.is_some(), "a desktop notification was clicked");
        if let Some(sink) = &self.sink {
            sink(NoticeActivation { key, token });
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
    let body = if markup {
        escape_markup(&notice.body)
    } else {
        notice.body.clone()
    };
    let actions = [DEFAULT_ACTION, notice.open_label.as_str()];
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("urgency", Value::U8(NORMAL_URGENCY));
    hints.insert("category", Value::from(CATEGORY));
    hints.insert("desktop-entry", Value::from(APP_ID));
    if notice.sound {
        hints.insert("sound-name", Value::from(MESSAGE_SOUND));
    } else {
        hints.insert("suppress-sound", Value::Bool(true));
    }
    proxy
        .call(
            "Notify",
            &(
                APP_NAME,
                replaces,
                NO_ICON,
                notice.summary.as_str(),
                body.as_str(),
                &actions[..],
                hints,
                SERVER_DEFAULT_TIMEOUT,
            ),
        )
        .await
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
