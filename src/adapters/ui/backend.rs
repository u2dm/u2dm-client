mod art;
mod avatars;
mod queries;

use std::rc::Rc;
use std::sync::Arc;

use slint::{CloseRequestResponse, ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use tokio::sync::{OwnedSemaphorePermit, mpsc, watch};

pub(super) use self::art::{apply_pack_art, apply_pack_avatar};
use self::art::{place_custom_emoji, place_pack_cell};
use self::avatars::apply_avatar_ready;
pub(super) use self::queries::{
    RECENT_SPEAKERS, adopted_room_key, current_edited_row, last_editable_row, recent_speakers,
    row_of_message, selected_room_key, shown_room_log_text, unread_below,
};
use super::call_video;
use super::call_video::Host;
use super::call_window::{self, WindowHooks};
use super::clock::install_clock_invalidation;
use super::decode::{
    DecodeOutcome, MediaSlot, TimelineItemKey, advance_animations, set_animation_tick,
    set_avatar_ready, set_image_ready,
};
use super::dto::{
    BodyLineDto, CallConnectionDto, CallDeviceDto, CallPersonDto, CallTileDto, IgnoredPersonDto,
    InviteeDto, KeywordDto, LogLineDto, MediaFailureKind, MediaState, MemberRowDto, MessageDto,
    PackBrowse, PackRowDto, PackSurface, PackTabDto, RoomDto, RowBody, SpaceChildDto, SpaceDto,
    ThumbUpdate, UserDeviceDto, decode_failure_kind, enrich_to_update,
};
use super::fields::{
    BodyLineFields, BodyPieceFields, CallConnectionFields, CallDeviceFields, CallPersonFields,
    CallTileFields, IgnoredPersonFields, InviteeFields, KeywordFields, LogLineFields,
    MemberRowFields, MessageFields, PackCellFields, PackRowFields, PackTabFields, PollAnswerFields,
    ReactionFields, ReactorFields, RoomFields, SpaceChildFields, SpaceFields, UserDeviceFields,
};
use super::handoff::follow_handoffs;
use super::multiplex::spawn_event_multiplexer;
use super::notices::{NoticeWords, follow_activations};
use super::props::UiProps;
use super::reconcile::{reorder_rows, timeline_row_of};
use super::reduce::{dispatch_effect, set_pack_browse};
use super::rows::locate_row;
use super::schema::model_props;
use super::session::begin_session;
use super::splice_model::SpliceModel;
use super::spoilers::reveal_row;
use super::window_events::install_window_events;
use crate::commands::effects::Effect;
use crate::commands::view::AppViewState;
use crate::config::APP_ID;
use crate::domain::timeline::EnrichmentDelta;
use crate::ports::call::CallVideoPort;
use crate::ports::handoff::HandoffPort;
use crate::ports::media::MediaCache;
use crate::ports::notifier::NotifierPort;

pub trait UiBackend: Sized + 'static {
    type Window: ComponentHandle + UiProps + 'static;
    type CallWindow: ComponentHandle + 'static;
    type Message: MessageFields<Self> + Clone + From<MessageDto> + 'static;
    type BodyLine: BodyLineFields<Self> + Clone + From<BodyLineDto> + 'static;
    type BodyPiece: BodyPieceFields<Self> + Clone + 'static;
    type Reaction: ReactionFields<Self> + Clone + 'static;
    type Reactor: ReactorFields<Self> + Clone + 'static;
    type PollAnswer: PollAnswerFields<Self> + Clone + 'static;
    type Room: RoomFields<Self> + Clone + PartialEq + From<RoomDto> + 'static;
    type Space: SpaceFields<Self> + Clone + PartialEq + From<SpaceDto> + 'static;
    type SpaceChild: SpaceChildFields<Self> + Clone + PartialEq + From<SpaceChildDto> + 'static;
    type MemberRow: MemberRowFields<Self> + Clone + PartialEq + From<MemberRowDto> + 'static;
    type Invitee: InviteeFields<Self> + Clone + PartialEq + From<InviteeDto> + 'static;
    type LogLine: LogLineFields<Self> + Clone + From<LogLineDto> + 'static;
    type CallTile: CallTileFields<Self> + Clone + PartialEq + From<CallTileDto> + 'static;
    type CallPerson: CallPersonFields<Self> + Clone + PartialEq + From<CallPersonDto> + 'static;
    type CallConnection: CallConnectionFields<Self>
        + Clone
        + PartialEq
        + From<CallConnectionDto>
        + 'static;
    type CallDevice: CallDeviceFields<Self> + Clone + PartialEq + From<CallDeviceDto> + 'static;
    type UserDevice: UserDeviceFields<Self> + Clone + PartialEq + From<UserDeviceDto> + 'static;
    type Keyword: KeywordFields<Self> + Clone + PartialEq + From<KeywordDto> + 'static;
    type IgnoredPerson: IgnoredPersonFields<Self>
        + Clone
        + PartialEq
        + From<IgnoredPersonDto>
        + 'static;
    type PackRow: PackRowFields<Self> + Clone + From<PackRowDto> + 'static;
    type PackCell: PackCellFields<Self> + Clone + 'static;
    type PackTab: PackTabFields<Self> + Clone + From<PackTabDto> + 'static;
    type Draft: Clone + 'static;

    fn models() -> Rc<Models<Self>>;
    fn attach_models(window: &Self::Window, models: &Models<Self>);
    fn bind_browse_packs(window: &Self::Window, browse: impl Fn(PackSurface, &str, i32) + 'static);
    fn bind_reveal_spoilers(window: &Self::Window, reveal: impl Fn(&str) + 'static);
    fn bind_window_hooks(window: &Self::Window, hooks: WindowHooks);
    fn bind_call_preview_seen(window: &Self::Window, seen: impl Fn(&str, i32, i32, bool) + 'static);
    fn new_call_window(main: &Self::Window, hooks: WindowHooks) -> Option<Self::CallWindow>;
    fn call_window() -> Option<Self::CallWindow>;
    fn keep_call_window(window: Option<Self::CallWindow>) -> Option<Self::CallWindow>;
    fn notification_text(window: &Self::Window, words: &NoticeWords<'_>) -> SharedString;

    fn with_timeline<R>(f: impl FnOnce(&SpliceModel<Self::Message>) -> R) -> R {
        f(&Self::models().timeline)
    }
}

macro_rules! declare_models {
    ($($field:ident $row:ident $model:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        pub struct Models<B: UiBackend> {
            $(pub $field: Rc<$model<B::$row>>,)*
        }

        impl<B: UiBackend> Default for Models<B> {
            fn default() -> Self {
                Self { $($field: Rc::default(),)* }
            }
        }
    };
}

model_props!(declare_models);

pub fn identify_the_app() {
    if let Err(e) = slint::set_xdg_app_id(APP_ID) {
        tracing::warn!("the window carries no app id, so no desktop entry claims it: {e}");
    }
}

pub fn spawn_event_handler<B: UiBackend>(
    window: &B::Window,
    ui_rx: mpsc::Receiver<Effect>,
    view_rx: watch::Receiver<Arc<AppViewState>>,
    media_cache: Arc<dyn MediaCache>,
    notifier: Arc<dyn NotifierPort>,
    handoff: &dyn HandoffPort,
    call_video: Option<Arc<dyn CallVideoPort>>,
) {
    begin_session::<B>(window);
    call_video::install(call_video);
    B::bind_window_hooks(window, call_window::hooks::<B>(window, Host::Main));
    window.window().on_close_requested(|| {
        call_window::dismiss::<B>();
        CloseRequestResponse::HideWindow
    });
    let previewing = window.as_weak();
    B::bind_call_preview_seen(window, move |feed, width, height, shown| {
        if let Some(window) = previewing.upgrade() {
            call_video::preview_seen::<B>(&window, feed, width, height, shown);
        }
    });
    follow_activations::<B>(window, notifier.as_ref());
    follow_handoffs::<B>(window, handoff);

    install_render_hooks::<B>(window.as_weak());
    install_clock_invalidation::<B>(Arc::clone(&media_cache));
    install_window_events::<B>(window);

    let media = Arc::clone(&media_cache);
    let browsing = window.as_weak();
    B::bind_browse_packs(window, move |surface, query, emote_columns| {
        if let Some(window) = browsing.upgrade() {
            let browse = PackBrowse::new(surface, query, emote_columns);
            set_pack_browse::<B>(&window, browse, media.as_ref());
        }
    });

    let media = Arc::clone(&media_cache);
    B::bind_reveal_spoilers(window, move |unique_id| {
        reveal_spoilers::<B>(unique_id, media.as_ref());
    });

    let weak = window.as_weak();
    spawn_event_multiplexer(ui_rx, view_rx, media_cache, move |event, media, permit| {
        post_effect::<B>(&weak, media, Arc::clone(&notifier), event, permit);
    });
}

pub fn reorder_spaces<B: UiBackend>(from: usize, to: usize) {
    reorder_rows(&B::models().spaces, from, to);
}

pub struct UiEventContext<'a, B: UiBackend> {
    pub models: &'a Models<B>,
    pub media: &'a dyn MediaCache,
    pub notifier: &'a dyn NotifierPort,
}

pub fn in_event_loop<B: UiBackend, T: Send + 'static>(
    window: &B::Window,
    what: &'static str,
    act: fn(&B::Window, T),
) -> impl Fn(T) + Send + Sync + 'static {
    let weak = window.as_weak();
    move |value| {
        let queued = weak.upgrade_in_event_loop(move |window| act(&window, value));
        if let Err(e) = queued {
            tracing::debug!("{what} could not reach the event loop: {e}");
        }
    }
}

pub fn on_main<B: UiBackend>(
    main: &B::Window,
    act: impl Fn(&B::Window) + 'static,
) -> impl Fn() + 'static {
    let weak = main.as_weak();
    move || {
        if let Some(main) = weak.upgrade() {
            act(&main);
        }
    }
}

fn post_effect<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    media: Arc<dyn MediaCache>,
    notifier: Arc<dyn NotifierPort>,
    event: Effect,
    permit: OwnedSemaphorePermit,
) {
    weak.upgrade_in_event_loop(move |w| {
        let models = B::models();
        let ctx = UiEventContext::<B> {
            models: &models,
            media: media.as_ref(),
            notifier: notifier.as_ref(),
        };
        dispatch_effect::<B>(&w, event, &ctx);
        drop(permit);
    })
    .ok();
}

fn install_render_hooks<B: UiBackend>(weak: slint::Weak<B::Window>) {
    set_animation_tick(tick_animations::<B>);

    set_image_ready({
        let weak = weak.clone();
        move |slot, outcome| {
            apply_thumbnail_ready::<B>(slot, outcome);
            if let Some(w) = weak.upgrade() {
                w.window().request_redraw();
            }
        }
    });

    set_avatar_ready(move |slots, outcome| {
        apply_avatar_ready::<B>(&weak, slots, outcome);
        if let Some(w) = weak.upgrade() {
            w.window().request_redraw();
        }
    });
}

pub fn enrich_message<B: UiBackend>(
    entry: &mut B::Message,
    delta: &EnrichmentDelta,
    media: &dyn MediaCache,
) {
    let update = enrich_to_update(delta, media);
    match update.thumbnail {
        ThumbUpdate::Ready(image) => show_thumbnail::<B>(entry, image),
        ThumbUpdate::Failed(reason) => show_media_failure::<B>(entry, reason),
        ThumbUpdate::Unchanged => {}
    }
    if let Some(image) = update.avatar {
        entry.set_avatar(image);
        entry.set_has_avatar(true);
    }
    if let Some(pronouns) = update.pronouns {
        entry.set_pronouns(pronouns);
    }
}

fn show_thumbnail<B: UiBackend>(entry: &mut B::Message, image: Image) {
    entry.set_thumbnail(image);
    entry.set_media_state(MediaState::Ready);
}

pub fn keep_shown_enrichment<B: UiBackend>(shown: &B::Message, rebuilt: &mut B::Message) {
    if shown.has_avatar() && !rebuilt.has_avatar() {
        rebuilt.set_avatar(shown.avatar());
        rebuilt.set_has_avatar(true);
    }
    let rebuilt_state = rebuilt.media_state();
    if shown.media_state() == MediaState::Ready.ui_name()
        && (rebuilt_state == MediaState::Idle.ui_name()
            || rebuilt_state == MediaState::NotLoaded.ui_name())
    {
        show_thumbnail::<B>(rebuilt, shown.thumbnail());
    }
    let pronouns = shown.pronouns();
    if !pronouns.is_empty() && rebuilt.pronouns().is_empty() {
        rebuilt.set_pronouns(pronouns);
    }
}

fn show_media_failure<B: UiBackend>(entry: &mut B::Message, reason: MediaFailureKind) {
    entry.set_media_state(MediaState::Failed);
    entry.set_media_failure(reason);
}

fn tick_animations<B: UiBackend>() {
    advance_animations(&mut |slot, hint, frame| match slot {
        MediaSlot::Thumbnail(item) => patch_timeline_row::<B>(item.unique_id(), hint, |entry| {
            show_thumbnail::<B>(entry, frame);
        }),
        MediaSlot::PackCell(key) => place_pack_cell::<B>(key, Some(&frame)),
        MediaSlot::CustomEmoji(mxc) => place_custom_emoji::<B>(mxc, Ok(&frame)),
    });
}

fn patch_timeline_row<B: UiBackend>(
    unique_id: &str,
    hint: usize,
    apply: impl FnOnce(&mut B::Message),
) -> Option<usize> {
    B::with_timeline(|timeline| {
        let hint = timeline_row_of(unique_id).unwrap_or(hint);
        let row = locate_row(timeline, &B::Message::unique_id, unique_id, hint)?;
        let mut entry = timeline.row_data(row)?;
        apply(&mut entry);
        timeline.set_row_data(row, entry);
        Some(row)
    })
}

fn reveal_spoilers<B: UiBackend>(unique_id: &str, media: &dyn MediaCache) {
    let Some(covered) = reveal_row(unique_id) else {
        return;
    };
    let body = covered.revealed(&TimelineItemKey::current(unique_id), media);
    patch_timeline_row::<B>(unique_id, 0, |entry| show_row_body::<B>(entry, body));
}

fn show_row_body<B: UiBackend>(entry: &mut B::Message, body: RowBody) {
    let lines: Vec<B::BodyLine> = body.lines.into_iter().map(B::BodyLine::from).collect();
    entry.set_body(body.rich.plain);
    entry.set_styled(body.rich.styled);
    entry.set_has_links(body.rich.has_links);
    entry.set_mentions_you(body.rich.mentions_you);
    entry.set_body_lines(ModelRc::new(VecModel::from(lines)));
    entry.set_emote_only(body.emote_only);
    entry.set_spoilers(body.spoilers);
}

pub(super) fn apply_thumbnail_ready<B: UiBackend>(slot: &MediaSlot, outcome: DecodeOutcome<'_>) {
    let art = match outcome {
        DecodeOutcome::Ready(image) => Ok(image),
        DecodeOutcome::Failed(failure) => Err(failure),
        DecodeOutcome::Deferred => return,
    };
    match slot {
        MediaSlot::Thumbnail(item) => {
            let unique_id = item.unique_id();
            let placed = patch_timeline_row::<B>(unique_id, 0, |entry| match art {
                Ok(image) => show_thumbnail::<B>(entry, image.clone()),
                Err(failure) => show_media_failure::<B>(entry, decode_failure_kind(failure)),
            });
            if placed.is_none() {
                tracing::debug!(
                    unique_id,
                    "dropped a decoded image with no live timeline row"
                );
            }
        }
        MediaSlot::PackCell(key) => apply_pack_art::<B>(key, art.ok()),
        MediaSlot::CustomEmoji(mxc) => {
            if place_custom_emoji::<B>(mxc, art).is_none() {
                tracing::debug!("dropped a decoded custom emoji with no live row showing it");
            }
        }
    }
}
