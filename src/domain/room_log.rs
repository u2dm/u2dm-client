use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LogLine {
    pub seq: u64,
    pub at: SystemTime,
    pub level: LogLevel,
    pub target: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomLog {
    pub lines: Arc<[Arc<LogLine>]>,
    pub dropped: u64,
}

impl Default for RoomLog {
    fn default() -> Self {
        Self {
            lines: Arc::from(Vec::new()),
            dropped: 0,
        }
    }
}

impl RoomLog {
    pub fn newest(&self) -> Option<u64> {
        self.lines.last().map(|line| line.seq)
    }

    pub fn oldest(&self) -> Option<u64> {
        self.lines.first().map(|line| line.seq)
    }

    pub fn holds_the_same_lines_as(&self, other: &Self) -> bool {
        self.newest() == other.newest() && self.dropped == other.dropped
    }
}
