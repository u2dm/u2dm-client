mod commands;
mod convert;
mod diff;
mod filter;
mod members;
mod pinned;
mod poll_ends;
mod poll_sends;
mod polls;
mod subscribe;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use matrix_sdk::attachment::AttachmentConfig;
use matrix_sdk::send_queue::{LocalEchoContent, SendHandle};
use matrix_sdk::{Client, Room};
use matrix_sdk::room::reply::{EnforceThread, Reply};
use matrix_sdk::ruma::events::AnyMessageLikeEventContent;
use matrix_sdk::ruma::events::room::message::{
    AddMentions, RoomMessageEventContent, RoomMessageEventContentWithoutRelation,
    TextMessageEventContent,
};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{IdParseError, OwnedEventId};
use mime::Mime;
use serde_json::{Map, Value};
use tokio::fs;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::spawn_blocking;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use self::commands::Commands;
use self::members::Members;
pub(super) use self::pinned::MatrixPinned;
use self::poll_ends::EndingPolls;
use self::subscribe::subscribe_timeline;
use super::attachment;
use super::media::MediaService;
use super::profile::PronounCache;
use super::session::ClientHandle;
use crate::domain::media::OutgoingAttachment;
use crate::domain::poll::PollDraft;
use crate::domain::room::RoomId;
use crate::domain::timeline::{TimelineCommand, TimelineFocus, TimelineUpdate};
use crate::error::{AppError, Result};
use crate::ports::matrix::{AttachmentHandoff, TimelinePort};

const ENRICH_INFLIGHT: usize = 8;
const PRE_QUEUE_LOOKUP_WAIT: Duration = Duration::from_secs(5);
const RELATION_FIELD: &str = "m.relates_to";

pub(super) struct TimelineContext<'a> {
    pub(super) client: &'a Client,
    pub(super) media: &'a Arc<MediaService>,
    pub(super) pronouns: &'a Arc<PronounCache>,
    pub(super) members: &'a Arc<Members>,
    pub(super) ending: &'a EndingPolls,
    pub(super) own_user_id: Option<&'a str>,
    pub(super) focused: bool,
    pub(super) first_unread: Option<&'a str>,
    pub(super) timeline_tx: &'a mpsc::Sender<TimelineUpdate>,
    pub(super) enrich: &'a EnrichmentPool,
}

pub(super) struct InflightEnrichment {
    revision: u64,
    fingerprint: u64,
    cancel: CancellationToken,
}

pub(super) type InflightEntries = HashMap<String, InflightEnrichment>;
pub(super) type InflightMap = Arc<StdMutex<InflightEntries>>;

pub(super) struct EnrichmentClaim {
    pub(super) revision: u64,
    pub(super) fingerprint: u64,
    pub(super) cancel: CancellationToken,
}

pub(super) struct EnrichmentPool {
    pub(super) tracker: TaskTracker,
    pub(super) token: CancellationToken,
    pub(super) inflight: InflightMap,
    pub(super) semaphore: Arc<Semaphore>,
    next_revision: AtomicU64,
}

impl EnrichmentPool {
    pub(super) fn new() -> Self {
        Self {
            tracker: TaskTracker::new(),
            token: CancellationToken::new(),
            inflight: Arc::new(StdMutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(ENRICH_INFLIGHT)),
            next_revision: AtomicU64::new(0),
        }
    }

    pub(super) fn claim(
        &self,
        unique_id: &str,
        fingerprint: u64,
        has_work: bool,
    ) -> Option<EnrichmentClaim> {
        let Ok(mut inflight) = self.inflight.lock() else {
            return None;
        };

        match inflight.remove(unique_id) {
            Some(same_revision) if same_revision.fingerprint == fingerprint => {
                inflight.insert(unique_id.to_owned(), same_revision);
                return None;
            }
            Some(superseded) => superseded.cancel.cancel(),
            None => {}
        }

        has_work.then(|| self.begin(&mut inflight, unique_id, fingerprint))
    }

    fn begin(
        &self,
        inflight: &mut InflightEntries,
        unique_id: &str,
        fingerprint: u64,
    ) -> EnrichmentClaim {
        let revision = self.next_revision.fetch_add(1, Ordering::Relaxed);
        let cancel = self.token.child_token();
        inflight.insert(
            unique_id.to_owned(),
            InflightEnrichment {
                revision,
                fingerprint,
                cancel: cancel.clone(),
            },
        );
        EnrichmentClaim {
            revision,
            fingerprint,
            cancel,
        }
    }

    pub(super) fn invalidate(&self, unique_id: &str) {
        if let Ok(mut inflight) = self.inflight.lock()
            && let Some(abandoned) = inflight.remove(unique_id)
        {
            abandoned.cancel.cancel();
        }
    }

    pub(super) fn finish(
        inflight: &StdMutex<InflightEntries>,
        unique_id: &str,
        finished_revision: u64,
    ) {
        if let Ok(mut inflight) = inflight.lock()
            && inflight
                .get(unique_id)
                .is_some_and(|current| current.revision == finished_revision)
        {
            inflight.remove(unique_id);
        }
    }
}

impl Drop for EnrichmentPool {
    fn drop(&mut self) {
        self.token.cancel();
        self.tracker.close();
    }
}

pub(super) struct MatrixTimeline {
    matrix: Arc<ClientHandle>,
    pronouns: Arc<PronounCache>,
}

struct QueueableAttachment {
    filename: String,
    content_type: Mime,
    data: Vec<u8>,
    config: AttachmentConfig,
}

async fn queued_send(room: &Room, local_id: &str) -> Result<SendHandle> {
    let txn = local_id
        .strip_prefix(convert::LOCAL_ID_PREFIX)
        .unwrap_or(local_id);
    let (echoes, _updates) = room
        .send_queue()
        .subscribe()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    echoes
        .into_iter()
        .find(|echo| echo.transaction_id.as_str() == txn)
        .and_then(|echo| match echo.content {
            LocalEchoContent::Event { send_handle, .. } => Some(send_handle),
            LocalEchoContent::React { .. } | LocalEchoContent::Redaction { .. } => None,
        })
        .ok_or_else(|| AppError::Other(format!("no queued send for {local_id}")))
}

async fn queue(room: &Room, content: AnyMessageLikeEventContent) -> Result<()> {
    room.send_queue()
        .send(content)
        .await
        .map(|_handle| ())
        .map_err(|e| AppError::Other(e.to_string()))
}

fn reply_relation(in_reply_to: &str) -> Result<Reply> {
    let event_id: OwnedEventId = in_reply_to
        .try_into()
        .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;
    Ok(Reply {
        event_id,
        enforce_thread: EnforceThread::MaybeThreaded,
        add_mentions: AddMentions::Yes,
    })
}

async fn reply_event(
    room: &Room,
    content: RoomMessageEventContentWithoutRelation,
    in_reply_to: &str,
) -> Result<RoomMessageEventContent> {
    let reply = reply_relation(in_reply_to)?;
    timeout(PRE_QUEUE_LOOKUP_WAIT, room.make_reply_event(content, reply))
        .await
        .map_err(|_| {
            AppError::Other(format!(
                "the replied-to event {in_reply_to} did not arrive in time"
            ))
        })?
        .map_err(|e| AppError::Other(e.to_string()))
}

async fn with_resolved_reply(
    room: &Room,
    config: AttachmentConfig,
    in_reply_to: &str,
) -> Result<AttachmentConfig> {
    let reply = reply_event(
        room,
        RoomMessageEventContentWithoutRelation::text_plain(""),
        in_reply_to,
    )
    .await?;
    let relation = Raw::new(&reply)?
        .get_field::<Value>(RELATION_FIELD)?
        .ok_or_else(|| AppError::Other(format!("the reply to {in_reply_to} has no relation")))?;
    Ok(config
        .mentions(reply.mentions)
        .extra_content(Some(relation_content(relation))))
}

fn relation_content(relation: Value) -> Map<String, Value> {
    [(RELATION_FIELD.to_owned(), relation)]
        .into_iter()
        .collect()
}

async fn upload_limit(client: &Client) -> Option<u64> {
    let lookup = client.load_or_fetch_max_upload_size();
    match timeout(PRE_QUEUE_LOOKUP_WAIT, lookup).await {
        Ok(Ok(limit)) => Some(u64::from(limit)),
        Ok(Err(e)) => {
            tracing::warn!("sending without the server's upload limit, which failed to load: {e}");
            None
        }
        Err(_) => {
            tracing::warn!(
                waited = ?PRE_QUEUE_LOOKUP_WAIT,
                "sending without the server's upload limit, which did not arrive in time"
            );
            None
        }
    }
}

async fn queueable_attachment(
    room: &Room,
    attachment: &OutgoingAttachment,
) -> Result<QueueableAttachment> {
    let picked = &attachment.picked;
    if let Some(limit) = upload_limit(&room.client()).await
        && picked.size > limit
    {
        return Err(AppError::AttachmentTooLarge { limit });
    }

    let content_type = attachment::content_type(picked, attachment.as_document);
    let mut config =
        AttachmentConfig::new().info(attachment::attachment_info(picked, &content_type));
    if let Some(caption) = attachment.caption.as_deref() {
        config = config.caption(Some(TextMessageEventContent::plain(caption)));
    }
    if let Some(in_reply_to) = attachment.reply_to.as_deref() {
        config = with_resolved_reply(room, config, in_reply_to).await?;
    }

    let data = fs::read(&picked.path).await?;
    let thumbnail = match attachment::thumbnail_source(picked, &content_type) {
        Some(source) => {
            let bytes = if source == picked.path {
                data.clone()
            } else {
                fs::read(&source).await?
            };
            spawn_blocking(move || attachment::make_thumbnail(&bytes))
                .await
                .ok()
                .flatten()
        }
        None => None,
    };

    Ok(QueueableAttachment {
        filename: picked.filename.clone(),
        content_type,
        data,
        config: config.thumbnail(thumbnail),
    })
}

async fn queue_attachment(
    room: &Room,
    attachment: &OutgoingAttachment,
    abandon: &CancellationToken,
) -> Result<AttachmentHandoff> {
    let queueable = tokio::select! {
        biased;
        () = abandon.cancelled() => return Ok(AttachmentHandoff::Abandoned),
        queueable = queueable_attachment(room, attachment) => queueable?,
    };

    tracing::info!(
        room_id = %room.room_id(),
        filename = queueable.filename,
        content_type = %queueable.content_type,
        size = attachment.picked.size,
        "queueing an attachment"
    );
    room.send_queue()
        .send_attachment(
            queueable.filename,
            queueable.content_type,
            queueable.data,
            queueable.config,
        )
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(AttachmentHandoff::Queued)
}

impl MatrixTimeline {
    pub(super) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self {
            matrix,
            pronouns: Arc::new(PronounCache::default()),
        }
    }
}

#[async_trait]
impl TimelinePort for MatrixTimeline {
    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        focus: TimelineFocus,
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
        close: CancellationToken,
    ) -> Result<()> {
        tracing::info!(%room_id, ?focus, "subscribing to timeline");
        subscribe_timeline(
            &self.matrix.client().await?,
            self.matrix.media(),
            &self.pronouns,
            room_id,
            &focus,
            timeline_tx,
            Commands::new(cmd_rx, close),
        )
        .await
    }

    async fn send_text(&self, room_id: &RoomId, body: &str) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        let content = RoomMessageEventContent::text_plain(body);
        queue(&room, content.into()).await
    }

    async fn send_reply(&self, room_id: &RoomId, body: &str, in_reply_to: &str) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        let content = RoomMessageEventContentWithoutRelation::text_plain(body);
        let content = reply_event(&room, content, in_reply_to).await?;
        queue(&room, content.into()).await
    }

    async fn send_poll(&self, room_id: &RoomId, draft: &PollDraft) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        tracing::info!(
            %room_id,
            answers = draft.answers().len(),
            "queueing a poll"
        );
        queue(&room, polls::start_content(draft)?).await
    }

    async fn send_attachment(
        &self,
        room_id: &RoomId,
        attachment: &OutgoingAttachment,
        abandon: CancellationToken,
    ) -> Result<AttachmentHandoff> {
        let room = self.matrix.room(room_id).await?;
        queue_attachment(&room, attachment, &abandon).await
    }

    async fn resend(&self, room_id: &RoomId, local_id: &str) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        queued_send(&room, local_id)
            .await?
            .unwedge()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        room.send_queue().set_enabled(true);
        Ok(())
    }

    async fn discard_send(&self, room_id: &RoomId, local_id: &str) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        queued_send(&room, local_id)
            .await?
            .abort()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        room.send_queue().set_enabled(true);
        Ok(())
    }
}
