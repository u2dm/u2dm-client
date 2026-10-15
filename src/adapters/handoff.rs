#[cfg(unix)]
use std::io::{self, ErrorKind, Read, Write};
#[cfg(unix)]
use std::net::Shutdown;
#[cfg(unix)]
use std::os::unix::net::UnixStream as BlockingStream;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use tokio::time::timeout;

#[cfg(unix)]
use crate::adapters::private_fs;
use crate::ports::handoff::{Handoff, HandoffPort, HandoffSink};

const SOCKET_NAME: &str = "handoff.sock";
#[cfg(unix)]
const HANDOFF_MAX_BYTES: u64 = 16 * 1024;
#[cfg(unix)]
const HANDOFF_READ_WAIT: Duration = Duration::from_secs(2);
#[cfg(unix)]
const HANDOFF_ACK_WAIT: Duration = Duration::from_secs(3);
#[cfg(unix)]
const ACKNOWLEDGED: &[u8] = b"ok\n";

pub enum Forwarding {
    Delivered,
    NoListener,
    Failed(String),
}

pub fn socket_in(data_dir: &Path) -> PathBuf {
    data_dir.join(SOCKET_NAME)
}

pub struct DesktopHandoff {
    socket: PathBuf,
}

pub struct SilentHandoff;

impl HandoffPort for SilentHandoff {
    fn on_handoff(&self, _sink: HandoffSink) {
        tracing::debug!("this process does not own the local data, so it takes no hand-offs");
    }
}

impl DesktopHandoff {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

#[cfg(unix)]
#[derive(Serialize, Deserialize)]
struct Envelope {
    link: Option<String>,
    token: Option<String>,
}

#[cfg(unix)]
pub fn forward(socket: &Path, handoff: &Handoff) -> Forwarding {
    let mut stream = match BlockingStream::connect(socket) {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            return Forwarding::NoListener;
        }
        Err(e) => return Forwarding::Failed(e.to_string()),
    };
    match exchange(&mut stream, handoff) {
        Ok(true) => Forwarding::Delivered,
        Ok(false) => Forwarding::Failed("the running U2DM did not take the hand-off".to_owned()),
        Err(e) => Forwarding::Failed(e.to_string()),
    }
}

#[cfg(not(unix))]
pub fn forward(_socket: &Path, _handoff: &Handoff) -> Forwarding {
    Forwarding::NoListener
}

#[cfg(unix)]
fn exchange(stream: &mut BlockingStream, handoff: &Handoff) -> io::Result<bool> {
    stream.set_read_timeout(Some(HANDOFF_ACK_WAIT))?;
    stream.set_write_timeout(Some(HANDOFF_ACK_WAIT))?;
    let envelope = Envelope {
        link: handoff.link.clone(),
        token: handoff.token.clone(),
    };
    let mut line = serde_json::to_vec(&envelope).map_err(io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.shutdown(Shutdown::Write)?;
    let mut reply = Vec::new();
    let limit = u64::try_from(ACKNOWLEDGED.len()).unwrap_or(u64::MAX);
    Read::take(&*stream, limit).read_to_end(&mut reply)?;
    Ok(reply == ACKNOWLEDGED)
}

#[cfg(unix)]
impl HandoffPort for DesktopHandoff {
    fn on_handoff(&self, sink: HandoffSink) {
        let listener = private_fs::bind_private_socket(&self.socket).and_then(|listener| {
            listener.set_nonblocking(true)?;
            UnixListener::from_std(listener)
        });
        match listener {
            Ok(listener) => {
                tracing::info!(
                    path = %self.socket.display(),
                    "listening for links another launch hands over"
                );
                tokio::spawn(serve(listener, sink));
            }
            Err(e) => tracing::warn!(
                path = %self.socket.display(),
                "cannot listen for links another launch hands over: {e}"
            ),
        }
    }
}

#[cfg(not(unix))]
impl HandoffPort for DesktopHandoff {
    fn on_handoff(&self, _sink: HandoffSink) {
        tracing::debug!(path = %self.socket.display(), "this platform takes no hand-offs");
    }
}

#[cfg(unix)]
async fn serve(listener: UnixListener, sink: HandoffSink) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(receive(stream, Arc::clone(&sink)));
            }
            Err(e) => {
                tracing::warn!("stopped listening for hand-offs: {e}");
                return;
            }
        }
    }
}

#[cfg(unix)]
async fn receive(mut stream: UnixStream, sink: HandoffSink) {
    let envelope = match read_envelope(&mut stream).await {
        Ok(envelope) => envelope,
        Err(reason) => {
            tracing::debug!("dropped a hand-off: {reason}");
            return;
        }
    };
    if let Err(e) = stream.write_all(ACKNOWLEDGED).await {
        tracing::debug!("could not acknowledge a hand-off: {e}");
    }
    sink(Handoff {
        link: envelope.link,
        token: envelope.token,
    });
}

#[cfg(unix)]
async fn read_envelope(stream: &mut UnixStream) -> Result<Envelope, String> {
    let mut line = String::new();
    timeout(
        HANDOFF_READ_WAIT,
        stream.take(HANDOFF_MAX_BYTES).read_to_string(&mut line),
    )
    .await
    .map_err(|_| "it did not arrive in time".to_owned())?
    .map_err(|e| format!("it could not be read: {e}"))?;
    serde_json::from_str(line.trim()).map_err(|e| format!("it is malformed: {e}"))
}
