use std::collections::{HashMap, VecDeque};
use std::env;
use std::fmt::{self, Write};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use tokio::sync::watch;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use crate::domain::room::RoomId;
use crate::domain::room_log::{LogLevel, LogLine, RoomLog};
use crate::ports::room_log::RoomLogPort;

const CAPTURE_ENV: &str = "U2DM_ROOM_LOG";
const CAPTURED_BY_DEFAULT: &str = "u2dm=debug,matrix_sdk=debug,matrix_sdk_crypto=info,\
                                   matrix_sdk_sqlite=info,matrix_sdk_common=info";
const LINES_PER_ROOM: usize = 1500;
const LINES_IN_ALL_ROOMS: usize = 30_000;
const ROOM_FIELDS: [&str; 2] = ["room_id", "room"];
const MESSAGE_FIELD: &str = "message";
const BRIDGED_LOG_FIELDS: &str = "log.";
const ROOM_ID_SIGIL: char = '!';
const SPAN_SEPARATOR: &str = ":";

pub struct RoomLogs {
    store: Mutex<Store>,
    captured: watch::Sender<u64>,
}

impl RoomLogs {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(Store::default()),
            captured: watch::Sender::new(0),
        }
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn keep(&self, room: &str, line: Unnumbered) {
        self.store().keep(room, line);
        self.captured
            .send_modify(|captured| *captured = captured.wrapping_add(1));
    }
}

impl RoomLogPort for RoomLogs {
    fn read(&self, room_id: &RoomId) -> RoomLog {
        self.store().read(room_id)
    }

    fn changes(&self) -> watch::Receiver<u64> {
        self.captured.subscribe()
    }

    fn forget(&self) {
        self.store().forget();
    }
}

pub fn capture<S>(logs: Arc<RoomLogs>) -> impl Layer<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup> + 'static,
{
    Capture { logs }.with_filter(capture_filter())
}

fn capture_filter() -> EnvFilter {
    let directives = env::var(CAPTURE_ENV).unwrap_or_else(|_| CAPTURED_BY_DEFAULT.to_owned());
    EnvFilter::builder().parse_lossy(directives)
}

struct Unnumbered {
    at: SystemTime,
    level: LogLevel,
    target: &'static str,
    text: String,
}

#[derive(Default)]
struct Kept {
    lines: VecDeque<Arc<LogLine>>,
    dropped: u64,
}

impl Kept {
    fn drop_oldest(&mut self) -> bool {
        let dropped = self.lines.pop_front().is_some();
        if dropped {
            self.dropped = self.dropped.saturating_add(1);
        }
        dropped
    }
}

#[derive(Default)]
struct Store {
    rooms: HashMap<Arc<str>, Kept>,
    held: usize,
    next_seq: u64,
}

impl Store {
    fn keep(&mut self, room: &str, line: Unnumbered) {
        let Unnumbered {
            at,
            level,
            target,
            text,
        } = line;
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        let kept = self.rooms.entry(Arc::from(room)).or_default();
        kept.lines.push_back(Arc::new(LogLine {
            seq,
            at,
            level,
            target,
            text,
        }));
        self.held = self.held.saturating_add(1);
        if kept.lines.len() > LINES_PER_ROOM && kept.drop_oldest() {
            self.held = self.held.saturating_sub(1);
        }
        while self.held > LINES_IN_ALL_ROOMS && self.drop_oldest_anywhere() {
            self.held = self.held.saturating_sub(1);
        }
    }

    fn drop_oldest_anywhere(&mut self) -> bool {
        self.rooms
            .values_mut()
            .filter(|kept| !kept.lines.is_empty())
            .min_by_key(|kept| kept.lines.front().map(|line| line.seq))
            .is_some_and(Kept::drop_oldest)
    }

    fn read(&self, room: &str) -> RoomLog {
        self.rooms
            .get(room)
            .map_or_else(RoomLog::default, |kept| RoomLog {
                lines: kept.lines.iter().map(Arc::clone).collect(),
                dropped: kept.dropped,
            })
    }

    fn forget(&mut self) {
        self.rooms.clear();
        self.held = 0;
    }
}

struct Capture {
    logs: Arc<RoomLogs>,
}

struct RoomScope(Arc<str>);

impl<S> Layer<S> for Capture
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut named = NamedRoom::default();
        attrs.record(&mut named);
        scope_span(id, named, &ctx);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let mut named = NamedRoom::default();
        values.record(&mut named);
        scope_span(id, named, &ctx);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut named = NamedRoom::default();
        event.record(&mut named);
        let Some(room) = named.room.or_else(|| scoped_room(event, &ctx)) else {
            return;
        };
        self.logs.keep(&room, describe(event, &ctx, &room));
    }
}

fn scope_span<S>(id: &Id, named: NamedRoom, ctx: &Context<'_, S>)
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    let (Some(room), Some(span)) = (named.room, ctx.span(id)) else {
        return;
    };
    drop(span.extensions_mut().replace(RoomScope(room)));
}

fn scoped_room<S>(event: &Event<'_>, ctx: &Context<'_, S>) -> Option<Arc<str>>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    ctx.event_scope(event)?.find_map(|span| {
        span.extensions()
            .get::<RoomScope>()
            .map(|scope| Arc::clone(&scope.0))
    })
}

fn describe<S>(event: &Event<'_>, ctx: &Context<'_, S>, room: &str) -> Unnumbered
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    let mut text = LineText::new(room);
    event.record(&mut text);
    let metadata = event.metadata();
    Unnumbered {
        at: SystemTime::now(),
        level: level_of(*metadata.level()),
        target: metadata.target(),
        text: text.finish(&span_path(event, ctx)),
    }
}

fn span_path<S>(event: &Event<'_>, ctx: &Context<'_, S>) -> String
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    ctx.event_scope(event).map_or_else(String::new, |scope| {
        scope
            .from_root()
            .map(|span| span.name())
            .collect::<Vec<_>>()
            .join(SPAN_SEPARATOR)
    })
}

fn level_of(level: Level) -> LogLevel {
    match level {
        Level::ERROR => LogLevel::Error,
        Level::WARN => LogLevel::Warn,
        Level::INFO => LogLevel::Info,
        Level::DEBUG => LogLevel::Debug,
        _ => LogLevel::Trace,
    }
}

fn names_a_room(field: &Field) -> bool {
    ROOM_FIELDS.contains(&field.name())
}

fn room_id_in(raw: &str) -> Option<&str> {
    let from_sigil = raw.get(raw.find(ROOM_ID_SIGIL)?..)?;
    let end = from_sigil.find(ends_room_id).unwrap_or(from_sigil.len());
    from_sigil
        .get(..end)
        .filter(|id| id.len() > ROOM_ID_SIGIL.len_utf8())
}

fn ends_room_id(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ',')
}

#[derive(Default)]
struct NamedRoom {
    room: Option<Arc<str>>,
}

impl NamedRoom {
    fn offer(&mut self, value: &str) {
        self.room = room_id_in(value).map(Arc::from);
    }
}

impl Visit for NamedRoom {
    fn record_str(&mut self, field: &Field, value: &str) {
        if self.room.is_none() && names_a_room(field) {
            self.offer(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if self.room.is_none() && names_a_room(field) {
            self.offer(&format!("{value:?}"));
        }
    }
}

struct LineText<'room> {
    room: &'room str,
    message: String,
    fields: String,
}

impl<'room> LineText<'room> {
    fn new(room: &'room str) -> Self {
        Self {
            room,
            message: String::new(),
            fields: String::new(),
        }
    }

    fn note(&mut self, field: &Field, value: &str) {
        let name = field.name();
        let repeats_the_room = names_a_room(field) && room_id_in(value) == Some(self.room);
        if repeats_the_room || name.starts_with(BRIDGED_LOG_FIELDS) {
            return;
        }
        write!(self.fields, " {name}={value}").ok();
    }

    fn finish(self, spans: &str) -> String {
        let Self {
            message, fields, ..
        } = self;
        if spans.is_empty() {
            format!("{message}{fields}")
        } else {
            format!("{spans}: {message}{fields}")
        }
    }
}

impl Visit for LineText<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == MESSAGE_FIELD {
            self.message.push_str(value);
        } else {
            self.note(field, value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == MESSAGE_FIELD {
            write!(self.message, "{value:?}").ok();
        } else {
            self.note(field, &format!("{value:?}"));
        }
    }
}
