use std::collections::HashSet;
use std::sync::Arc;

use matrix_sdk::ruma::OwnedEventId;
use matrix_sdk::{Client, Room};
use matrix_sdk_ui::timeline::{Timeline, TimelineItem};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tracing::Instrument;

use crate::adapters::matrix::media::{MediaService, ThumbnailRequest};
use crate::adapters::matrix::profile::PronounCache;
use crate::adapters::matrix::timeline::convert::event_media;
use crate::adapters::matrix::timeline::emoji_files::{Settled, download_emoji_files};
use crate::adapters::matrix::timeline::members::{Batch, resolve_members};
use crate::adapters::matrix::timeline::{EnrichmentClaim, EnrichmentPool, TimelineContext};
use crate::domain::media::{ContentKey, ThumbnailOutcome};
use crate::domain::message::TimelineMessage;
use crate::domain::timeline::{EnrichmentDelta, TimelinePatch, TimelineUpdate};

pub(super) const REPLY_FETCH_INFLIGHT: usize = 4;

pub(super) const MEMBER_BATCHES: usize = 4;

fn needs_pronouns(msg: &TimelineMessage, pronouns: &PronounCache) -> bool {
    !msg.is_own && pronouns.needs_fetch(&msg.sender)
}

struct EnrichmentJob {
    unique_id: String,
    thumbnail: Option<ThumbnailRequest>,
    sender_avatar: Option<String>,
    pronouns_of: Option<String>,
}

impl EnrichmentJob {
    fn of(item: &TimelineItem, msg: &TimelineMessage, ctx: &TimelineContext<'_>) -> Self {
        let media = ctx.media;
        Self {
            unique_id: msg.unique_id.clone(),
            thumbnail: ThumbnailRequest::of(msg, event_media(item))
                .filter(|request| media.needs_thumbnail_download(&request.content)),
            sender_avatar: msg
                .sender_avatar_url
                .clone()
                .filter(|mxc| media.needs_avatar_download(mxc)),
            pronouns_of: needs_pronouns(msg, ctx.pronouns).then(|| msg.sender.clone()),
        }
    }

    fn has_work(&self) -> bool {
        self.thumbnail.is_some() || self.sender_avatar.is_some() || self.pronouns_of.is_some()
    }

    async fn fetch_thumbnail(&self, client: &Client, media: &MediaService) -> ThumbnailOutcome {
        match &self.thumbnail {
            Some(request) => media.enrich_thumbnail(client, request).await,
            None => ThumbnailOutcome::Unchanged,
        }
    }

    async fn fetch_avatar(&self, client: &Client, media: &MediaService) -> Option<String> {
        let mxc = self.sender_avatar.as_deref()?;
        media.enrich_avatar(client, mxc).await
    }

    async fn resolve_pronouns(
        &self,
        client: &Client,
        pronouns: &PronounCache,
    ) -> Option<Vec<String>> {
        let sender = self.pronouns_of.as_deref()?;
        let resolved = pronouns.resolve(client, sender).await;
        (!resolved.is_empty()).then_some(resolved)
    }

    fn thumbnail_content(&self) -> Option<ContentKey> {
        self.thumbnail
            .as_ref()
            .map(|request| request.content.clone())
    }
}

fn spawn_enrichment(ctx: &TimelineContext<'_>, job: EnrichmentJob, claim: EnrichmentClaim) {
    let client = ctx.client.clone();
    let media = Arc::clone(ctx.media);
    let pronouns = Arc::clone(ctx.pronouns);
    let inflight = Arc::clone(&ctx.enrich.inflight);
    let semaphore = Arc::clone(&ctx.enrich.semaphore);
    let EnrichmentClaim {
        revision,
        fingerprint,
        cancel: token,
    } = claim;
    let tx = ctx.timeline_tx.clone();

    let enrichment = async move {
        let work = async {
            let _permit = semaphore.acquire().await.ok()?;
            let (thumbnail, avatar_mxc, pronouns) = tokio::join!(
                job.fetch_thumbnail(&client, &media),
                job.fetch_avatar(&client, &media),
                job.resolve_pronouns(&client, &pronouns),
            );
            Some(EnrichmentDelta {
                unique_id: job.unique_id.clone(),
                thumbnail_content: job.thumbnail_content(),
                fingerprint,
                thumbnail,
                avatar_mxc,
                pronouns,
            })
        };

        let delta = token.run_until_cancelled(work).await.flatten();

        EnrichmentPool::finish(&inflight, &job.unique_id, revision);

        if let Some(delta) = delta
            && !delta.is_noop()
        {
            drop(
                tx.send(TimelineUpdate::patch(TimelinePatch::Enrich(delta)))
                    .await,
            );
        }
    };
    ctx.enrich.tracker.spawn(enrichment.in_current_span());
}

pub(in crate::adapters::matrix::timeline) fn enrich_message(
    item: &TimelineItem,
    msg: &TimelineMessage,
    ctx: &TimelineContext<'_>,
) {
    let job = EnrichmentJob::of(item, msg, ctx);
    let claim = ctx
        .enrich
        .claim(&msg.unique_id, msg.enrichment_fingerprint(), job.has_work());
    if let Some(claim) = claim {
        spawn_enrichment(ctx, job, claim);
    }
}

pub(in crate::adapters::matrix::timeline) fn enrich_messages<'a>(
    rendered: impl Iterator<Item = (&'a TimelineItem, &'a TimelineMessage)>,
    ctx: &TimelineContext<'_>,
) {
    for (item, msg) in rendered {
        enrich_message(item, msg, ctx);
    }
}

pub(super) fn spawn_reply_detail_fetches(
    items: &[Arc<TimelineItem>],
    timeline: &Arc<Timeline>,
    fetched: &mut HashSet<String>,
    reply_limit: &Arc<Semaphore>,
    side_tasks: &mut JoinSet<()>,
) {
    for item in items {
        let Some(event) = item.as_event() else {
            continue;
        };
        let Some(details) = event.content().in_reply_to() else {
            continue;
        };
        if !details.event.is_unavailable() {
            continue;
        }
        let Some(event_id) = event.event_id().map(ToString::to_string) else {
            continue;
        };
        if !fetched.insert(event_id.clone()) {
            continue;
        }
        let timeline = Arc::clone(timeline);
        let reply_limit = Arc::clone(reply_limit);
        let fetch = async move {
            let Ok(_permit) = reply_limit.acquire().await else {
                return;
            };
            let Ok(id) = OwnedEventId::try_from(event_id.as_str()) else {
                return;
            };
            if let Err(e) = timeline.fetch_details_for_event(&id).await {
                tracing::debug!("failed to fetch reply details: {e}");
            }
        };
        side_tasks.spawn(fetch.in_current_span());
    }
}

pub(super) fn spawn_member_fetch(
    ctx: &TimelineContext<'_>,
    room: &Room,
    side_tasks: &mut JoinSet<()>,
    batches: &mpsc::Sender<Batch>,
) {
    let wanted = ctx.members.take_wanted();
    if wanted.is_empty() {
        return;
    }
    let room = room.clone();
    let client = ctx.client.clone();
    let media = Arc::clone(ctx.media);
    let batches = batches.clone();
    let resolve = async move {
        resolve_members(&room, &client, &media, wanted, &batches).await;
    };
    side_tasks.spawn(resolve.in_current_span());
}

pub(super) fn spawn_emoji_downloads(
    ctx: &TimelineContext<'_>,
    side_tasks: &mut JoinSet<()>,
    settled: &mpsc::Sender<Settled>,
) {
    let wanted = ctx.emoji_files.take_wanted();
    if wanted.is_empty() {
        return;
    }
    let client = ctx.client.clone();
    let media = Arc::clone(ctx.media);
    let settled = settled.clone();
    let download = async move {
        download_emoji_files(&client, &media, wanted, &settled).await;
    };
    side_tasks.spawn(download.in_current_span());
}
