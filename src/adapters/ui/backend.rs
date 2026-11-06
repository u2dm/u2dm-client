use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::rc::Rc;
use std::sync::Arc;

use slint::{CloseRequestResponse, ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use tokio::sync::{OwnedSemaphorePermit, mpsc, watch};

use super::call_video::Host;
use super::call_window::{self, WindowHooks};
use super::clock::install_clock_invalidation;
use super::decode::{
    AvatarSlot, DecodeFailure, DecodeOutcome, MediaSlot, TimelineItemKey, advance_animations,
    request_pack_cell, rows_showing_custom_emoji, set_animation_tick, set_avatar_ready,
    set_image_ready,
};
use super::dto::{
    BodyLineDto, CallConnectionDto, CallDeviceDto, CallPersonDto, CallTileDto, EmojiArt,
    InviteeDto, LogLineDto, MediaFailureKind, MediaState, MemberRowDto, MessageDto, PackBrowse,
    PackRowDto, PackSurface, PackTabDto, RoomDto, RowBody, SpaceChildDto, SpaceDto, ThumbUpdate,
    UserDeviceDto, cell_pack, decode_failure_kind, enrich_to_update,
};
use super::fields::{
    BodyLineFields, BodyPieceFields, CallConnectionFields, CallDeviceFields, CallPersonFields,
    CallTileFields, InviteeFields, LogLineFields, MemberRowFields, MessageFields, PackCellFields,
    PackRowFields, PackTabFields, PollAnswerFields, ReactionFields, ReactorFields, RoomFields,
    SpaceChildFields, SpaceFields, UserDeviceFields,
};
use super::handoff::follow_handoffs;
use super::multiplex::spawn_event_multiplexer;
use super::notices::{NoticeWords, follow_activations};
use super::present::room_log_text;
use super::props::{ImageProp, IntProp, StringProp, UiProps};
use super::reconcile::{
    forget_pack_avatar, pack_cell_row, pack_tab_row, reorder_rows, shows_pack_avatar,
    timeline_row_of,
};
use super::reduce::{dispatch_effect, is_latest_adoption, set_pack_browse};
use super::router::EditedRow;
use super::rows::{locate_row, patch_first, patch_rows_by_id, patch_rows_where, row_index};
use super::schema::model_props;
use super::session::{begin_session, from_snapshot};
use super::splice_model::SpliceModel;
use super::spoilers::reveal_row;
use super::window_events::install_window_events;
use super::{call, call_video};
use crate::commands::effects::Effect;
use crate::commands::view::AppViewState;
use crate::config::APP_ID;
use crate::domain::room::RoomId;
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

pub fn selected_room_key<B: UiBackend>(weak: &slint::Weak<B::Window>) -> Option<(RoomId, i32)> {
    let w = weak.upgrade()?;
    let room_id = w.get_string(StringProp::SelectedRoomId).to_string();
    if room_id.is_empty() {
        return None;
    }
    Some((RoomId::new(room_id), w.get_int(IntProp::SelectedGeneration)))
}

pub fn adopted_room_key<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    seen_timeline_token: i32,
) -> Option<(RoomId, i32)> {
    selected_room_key::<B>(weak)
        .filter(|(_, generation)| is_latest_adoption(*generation, seen_timeline_token))
}

pub fn unread_below<B: UiBackend>(first_unseen_row: i32) -> u32 {
    let from = usize::try_from(first_unseen_row).unwrap_or(0);
    let below = B::with_timeline(|timeline| {
        timeline.count_tail(from, |entry: &B::Message| entry.counts_as_unread())
    });
    u32::try_from(below).unwrap_or(u32::MAX)
}

pub fn last_editable_row<B: UiBackend>() -> i32 {
    row_index(B::with_timeline(|timeline| {
        timeline.last_position(|entry: &B::Message| entry.text_editable())
    }))
}

pub const RECENT_SPEAKERS: usize = 12;

pub fn recent_speakers<B: UiBackend>(limit: usize) -> Vec<String> {
    let mut speakers: Vec<String> = Vec::new();
    B::with_timeline(|timeline| {
        timeline.visit_newest_first(|entry: &B::Message| {
            let speaker = entry.sender_id();
            if entry.counts_as_unread() && !speakers.iter().any(|known| known == speaker) {
                speakers.push(speaker.to_owned());
            }
            if speakers.len() >= limit {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
    });
    speakers
}

pub fn row_of_message<B: UiBackend>(event_id: &str, local_id: &str) -> i32 {
    let names = |entry: &B::Message| {
        (!event_id.is_empty() && entry.event_id() == event_id)
            || (!local_id.is_empty() && entry.local_id() == local_id)
    };
    row_index(B::with_timeline(|timeline| timeline.last_position(names)))
}

pub fn shown_room_log_text() -> String {
    from_snapshot(|view| view.room_log.clone())
        .flatten()
        .map(|shown| room_log_text(&shown))
        .unwrap_or_default()
}

pub fn current_edited_row<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    unique_id: &str,
    timeline_token: i32,
) -> Option<EditedRow> {
    if unique_id.is_empty() {
        return None;
    }
    let w = weak.upgrade()?;
    if w.get_int(IntProp::TimelineToken) != timeline_token {
        return None;
    }
    B::with_timeline(|timeline| {
        let entry = timeline.row_data(indexed_timeline_row::<B>(timeline, unique_id)?)?;
        Some(EditedRow {
            event_id: entry.event_id().to_owned(),
            local_id: entry.local_id().to_owned(),
        })
    })
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
    if shown.media_state() == MediaState::Ready.ui_name()
        && rebuilt.media_state() == MediaState::Idle.ui_name()
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

fn place_custom_emoji<B: UiBackend>(
    mxc: &str,
    art: Result<&Image, DecodeFailure>,
) -> Option<usize> {
    let rows = rows_showing_custom_emoji(mxc);
    B::with_timeline(|timeline| {
        let mut placed = None;
        for unique_id in &rows {
            let Some(row) = indexed_timeline_row::<B>(timeline, unique_id) else {
                continue;
            };
            let Some(entry) = timeline.row_data(row) else {
                continue;
            };
            let in_chips = patch_reaction_art::<B>(&entry.reactions(), mxc, art);
            let in_detail = patch_reaction_art::<B>(&entry.all_reactions(), mxc, art);
            let in_body = patch_body_emoji::<B>(&entry.body_lines(), mxc, art);
            if in_chips || in_detail || in_body {
                placed = Some(row);
            }
        }
        placed
    })
}

fn patch_body_emoji<B: UiBackend>(
    lines: &ModelRc<B::BodyLine>,
    mxc: &str,
    art: Result<&Image, DecodeFailure>,
) -> bool {
    let mut patched = false;
    for line in lines.iter() {
        patched |= patch_rows_where(
            &line.pieces(),
            |piece| piece.key() == mxc,
            |piece| match art {
                Ok(image) => {
                    piece.set_image(image.clone());
                    piece.set_art(EmojiArt::Ready);
                }
                Err(_) => piece.set_art(EmojiArt::Failed),
            },
        );
    }
    patched
}

fn patch_reaction_art<B: UiBackend>(
    reactions: &ModelRc<B::Reaction>,
    key: &str,
    art: Result<&Image, DecodeFailure>,
) -> bool {
    patch_first(
        reactions,
        |reaction| reaction.key() == key,
        |reaction| match art {
            Ok(image) => {
                reaction.set_image(image.clone());
                reaction.set_art(EmojiArt::Ready);
            }
            Err(_) => reaction.set_art(EmojiArt::Failed),
        },
    )
}

fn place_pack_cell<B: UiBackend>(key: &str, art: Option<&Image>) -> Option<usize> {
    let row = pack_cell_row(key)?;
    let entry = B::models().pack_rows.row_data(row)?;
    patch_pack_cell::<B>(&entry, key, art).then_some(row)
}

fn patch_pack_cell<B: UiBackend>(row: &B::PackRow, key: &str, art: Option<&Image>) -> bool {
    patch_first(
        &row.cells(),
        |cell| cell.key() == key,
        |cell| match art {
            Some(art) => {
                cell.set_image(art.clone());
                cell.set_media_state(MediaState::Ready);
            }
            None => cell.set_media_state(MediaState::Failed),
        },
    )
}

fn adopt_pack_icon<B: UiBackend>(pack_id: &str, image: &Image) {
    let Some(row) = pack_tab_row(pack_id) else {
        return;
    };
    let packs = &B::models().pack_tabs;
    let Some(updated) = packs
        .row_data(row)
        .and_then(|tab| pack_with_icon::<B>(&tab, pack_id, image))
    else {
        return;
    };
    packs.remove(row);
    packs.insert(row, updated);
}

fn pack_with_icon<B: UiBackend>(
    pack: &B::PackTab,
    pack_id: &str,
    image: &Image,
) -> Option<B::PackTab> {
    if pack.has_icon() || pack.id() != pack_id {
        return None;
    }
    let mut updated = pack.clone();
    updated.set_icon(image.clone());
    updated.set_has_icon(true);
    Some(updated)
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

pub(super) fn apply_pack_art<B: UiBackend>(key: &str, art: Option<&Image>) {
    if place_pack_cell::<B>(key, art).is_none() {
        tracing::debug!(key, "dropped a decoded image with no live sticker cell");
    }
    let pack = cell_pack(key);
    if let Some(image) = art
        && !shows_pack_avatar(pack)
    {
        adopt_pack_icon::<B>(pack, image);
    }
}

pub(super) fn apply_pack_avatar<B: UiBackend>(pack_id: &str, art: Option<&Image>) {
    match art {
        Some(image) => adopt_pack_icon::<B>(pack_id, image),
        None => {
            if let Some(first_cell) = forget_pack_avatar(pack_id) {
                request_pack_cell(&first_cell);
            }
        }
    }
}

fn apply_pack_avatars<B: UiBackend>(slots: &[AvatarSlot], art: Option<&Image>) {
    for slot in slots {
        if let AvatarSlot::Pack(pack_id) = slot {
            apply_pack_avatar::<B>(pack_id, art);
        }
    }
}

#[derive(Default)]
struct AvatarTargets<'a> {
    messages: HashSet<&'a str>,
    reactors_by_message: HashMap<&'a str, HashSet<&'a str>>,
    rooms: HashSet<&'a str>,
    spaces: HashSet<&'a str>,
    space_children: HashSet<&'a str>,
    members: HashSet<&'a str>,
    readers: HashSet<&'a str>,
    mentions: HashSet<&'a str>,
    invitees: HashSet<&'a str>,
    call_faces: HashSet<&'a str>,
    singles: HashSet<ImageProp>,
}

fn group_slots(slots: &[AvatarSlot]) -> AvatarTargets<'_> {
    let mut targets = AvatarTargets::default();
    for slot in slots {
        match slot {
            AvatarSlot::Message(item) => targets.messages.extend([item.unique_id()]),
            AvatarSlot::Reactor { item, user_id } => targets
                .reactors_by_message
                .entry(item.unique_id())
                .or_default()
                .extend([user_id.as_str()]),
            AvatarSlot::Room(id) => targets.rooms.extend([id.as_str()]),
            AvatarSlot::Space(id) => targets.spaces.extend([id.as_str()]),
            AvatarSlot::SpaceChild(id) => targets.space_children.extend([id.as_str()]),
            AvatarSlot::Member(user_id) => targets.members.extend([user_id.as_str()]),
            AvatarSlot::Reader(user_id) => targets.readers.extend([user_id.as_str()]),
            AvatarSlot::Mention(user_id) => targets.mentions.extend([user_id.as_str()]),
            AvatarSlot::Invitee(user_id) => targets.invitees.extend([user_id.as_str()]),
            AvatarSlot::SelectedRoom => targets.singles.extend([ImageProp::SelectedRoomAvatar]),
            AvatarSlot::RoomInfo => targets.singles.extend([ImageProp::RoomInfoAvatar]),
            AvatarSlot::UserInfo => targets.singles.extend([ImageProp::UserInfoAvatar]),
            AvatarSlot::Invite => targets.singles.extend([ImageProp::InviteAvatar]),
            AvatarSlot::Removal => targets.singles.extend([ImageProp::RemovalAvatar]),
            AvatarSlot::RoomPreview => targets.singles.extend([ImageProp::RoomPreviewAvatar]),
            AvatarSlot::Ring => targets.singles.extend([ImageProp::RingAvatar]),
            AvatarSlot::CallFace(user_id) => targets.call_faces.extend([user_id.as_str()]),
            AvatarSlot::User => targets.singles.extend([ImageProp::UserAvatar]),
            AvatarSlot::AttachmentPreview { .. } => {
                targets.singles.extend([ImageProp::AttachmentPreview]);
            }
            AvatarSlot::Pack(_) => {}
        }
    }
    targets
}

fn indexed_timeline_row<B: UiBackend>(
    timeline: &SpliceModel<B::Message>,
    unique_id: &str,
) -> Option<usize> {
    locate_row(
        timeline,
        &B::Message::unique_id,
        unique_id,
        timeline_row_of(unique_id)?,
    )
}

fn patch_message_avatars<B: UiBackend>(
    timeline: &SpliceModel<B::Message>,
    unique_ids: &HashSet<&str>,
    image: &Image,
) {
    for unique_id in unique_ids {
        let Some(row) = indexed_timeline_row::<B>(timeline, unique_id) else {
            continue;
        };
        let Some(mut entry) = timeline.row_data(row) else {
            continue;
        };
        entry.set_avatar(image.clone());
        entry.set_has_avatar(true);
        timeline.set_row_data(row, entry);
    }
}

fn patch_reactor_avatars<B: UiBackend>(
    timeline: &SpliceModel<B::Message>,
    reactors_by_message: &HashMap<&str, HashSet<&str>>,
    image: &Image,
) {
    for (unique_id, user_ids) in reactors_by_message {
        let Some(entry) =
            indexed_timeline_row::<B>(timeline, unique_id).and_then(|row| timeline.row_data(row))
        else {
            continue;
        };
        for user_id in user_ids {
            patch_reactor_avatar::<B>(&entry, user_id, image);
        }
    }
}

fn patch_reactor_avatar<B: UiBackend>(entry: &B::Message, user_id: &str, image: &Image) {
    for reaction in entry.reactions().iter() {
        patch_first(
            &reaction.avatars(),
            |face| face.user_id() == user_id && !face.has_avatar(),
            |face| {
                face.set_avatar(image.clone());
                face.set_has_avatar(true);
            },
        );
    }
}

fn apply_avatar_ready<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    slots: &[AvatarSlot],
    outcome: DecodeOutcome<'_>,
) {
    let image = match outcome {
        DecodeOutcome::Ready(image) => image,
        DecodeOutcome::Failed(_) => {
            apply_pack_avatars::<B>(slots, None);
            return;
        }
        DecodeOutcome::Deferred => return,
    };
    apply_pack_avatars::<B>(slots, Some(image));
    let targets = group_slots(slots);
    if !targets.singles.is_empty()
        && let Some(w) = weak.upgrade()
    {
        for single in &targets.singles {
            w.set_image(*single, Some(image.clone()));
        }
    }
    let models = B::models();
    patch_message_avatars::<B>(&models.timeline, &targets.messages, image);
    patch_reactor_avatars::<B>(&models.timeline, &targets.reactors_by_message, image);
    let show_room = |entry: &mut B::Room| {
        entry.set_avatar(image.clone());
        entry.set_has_avatar(true);
    };
    patch_rows_by_id(&*models.rooms, &targets.rooms, &B::Room::id, show_room);
    patch_rows_by_id(&*models.rooms, &targets.spaces, &B::Room::id, show_room);
    for spaces in [&models.spaces, &models.subspaces] {
        patch_rows_by_id(&**spaces, &targets.spaces, &B::Space::id, |entry| {
            entry.set_avatar(image.clone());
            entry.set_has_avatar(true);
        });
    }
    if !targets.space_children.is_empty() {
        patch_rows_by_id(
            &*models.space_children,
            &targets.space_children,
            &B::SpaceChild::id,
            |entry| {
                entry.set_avatar(image.clone());
                entry.set_has_avatar(true);
            },
        );
    }
    patch_person_avatars::<B>(&targets, image);
    call::show_faces::<B>(&targets.call_faces, image);
}

fn patch_person_avatars<B: UiBackend>(targets: &AvatarTargets<'_>, image: &Image) {
    let models = B::models();
    for (rows, ids) in [
        (&models.room_members, &targets.members),
        (&models.readers, &targets.readers),
        (&models.mentions, &targets.mentions),
    ] {
        if !ids.is_empty() {
            patch_rows_by_id(&**rows, ids, &B::MemberRow::user_id, |entry| {
                entry.set_avatar(image.clone());
                entry.set_has_avatar(true);
            });
        }
    }
    if !targets.invitees.is_empty() {
        for rows in [&models.invitee_results, &models.invitee_picks] {
            patch_rows_by_id(&**rows, &targets.invitees, &B::Invitee::user_id, |entry| {
                entry.set_avatar(image.clone());
                entry.set_has_avatar(true);
            });
        }
    }
}
