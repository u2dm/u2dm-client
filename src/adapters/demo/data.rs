use std::collections::{HashMap, HashSet};
use std::fs;
use std::mem;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::dto::{DemoData, RoomDto, SpaceDto, StickerPackDto, UnjoinedDto};
use super::timeline::{self, scenario};
use super::{media, message_menu, polls};
use crate::adapters::markdown::{self, Composed, RoomMentions};
use crate::domain::auth::Session;
use crate::domain::media::{
    AudioKind, AudioMeta, FileMeta, ImageMeta, OutgoingAttachment, VideoMeta,
};
use crate::domain::message::{
    MessageBody, MessagePreviewKind, PinnedMessage, ReadBy, ReplyInfo, RichText, SendState,
    TimelineMessage,
};
use crate::domain::poll::{Poll, PollAnswer, PollDraft, PollStatus};
use crate::domain::room::{NotifyMode, Room, RoomId, Space};
use crate::domain::room_info::{
    MemberRole, Reader, RoomAbout, RosterMember, RosterSection, sort_roster,
};
use crate::domain::space_index::SpaceChild;
use crate::domain::sticker::{StickerImage, StickerPack};
use crate::domain::user_info::{
    IdentityTrust, ModerationPowers, Pronouns, RoomMembership, UserId, UserProfile, localpart,
};

const UNKNOWN_SENDER: &str = "@member:matrix.org";
const SENT_STICKER_EXTENT: u32 = 512;
const STICKER_ASSET_MARKER: char = '#';
const DEMO_VIA: &str = "demo.local";
const COPY_SUFFIX: &str = "-copy";
const OWN_NAME_IN_TIMELINES: &str = "You";
const MATRIX_TO: &str = "https://matrix.to/#/";

static DATA: OnceLock<DemoData> = OnceLock::new();
static LOAD_ERROR: OnceLock<String> = OnceLock::new();

fn data() -> &'static DemoData {
    DATA.get_or_init(load)
}

fn load() -> DemoData {
    let path = media::data_path();
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::error!("demo data {} could not be read: {e}", path.display());
            drop(LOAD_ERROR.set(format!("{} could not be read: {e}", path.display())));
            return DemoData::default();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(data) => data,
        Err(e) => {
            tracing::error!("demo data {} is not valid: {e}", path.display());
            drop(LOAD_ERROR.set(format!("{} is not valid: {e}", path.display())));
            DemoData::default()
        }
    }
}

pub fn load_error() -> Option<&'static str> {
    LOAD_ERROR.get().map(String::as_str)
}

pub fn source_path() -> String {
    media::data_path().display().to_string()
}

pub fn counts() -> (usize, usize, usize) {
    let data = data();
    (data.rooms.len(), data.spaces.len(), data.timelines.len())
}

pub fn own_user() -> &'static str {
    &data().session.user_id
}

pub fn session() -> Session {
    data().session.to_session()
}

#[derive(Default, Clone)]
pub struct RoomOverrides {
    pub left: HashSet<String>,
    pub notify: HashMap<String, NotifyMode>,
    pub read: HashSet<String>,
    pub started_chats: HashMap<String, String>,
}

pub fn rooms_with(joined: &[RoomId], overrides: &RoomOverrides) -> Vec<Arc<Room>> {
    let now = now_ms();
    let data = data();
    data.rooms
        .iter()
        .map(|room| room.to_room(now, overrides.notify.get(&room.id).copied()))
        .chain(
            data.unjoined
                .iter()
                .filter(|entry| !entry.space && was_joined(joined, &entry.id))
                .map(|entry| {
                    let mut room = entry.to_room(now);
                    if let Some(notify) = overrides.notify.get(&entry.id) {
                        room.notify = *notify;
                    }
                    room
                }),
        )
        .chain(
            overrides
                .started_chats
                .iter()
                .map(|(room_id, user_id)| started_chat(room_id, user_id, now)),
        )
        .filter(|room| !overrides.left.contains(room.id.as_ref()))
        .map(|room| read_when_marked(room, &overrides.read))
        .map(Arc::new)
        .collect()
}

fn started_chat(room_id: &str, user_id: &str, now: u64) -> Room {
    Room {
        id: RoomId::new(room_id),
        display_name: person_name(user_id).unwrap_or_else(|| localpart(user_id).to_owned()),
        avatar_mxc: Some(user_id.to_owned()),
        topic: None,
        canonical_alias: None,
        is_direct: true,
        is_encrypted: true,
        poll_permissions: polls::permissions(),
        message_permissions: message_menu::permissions(),
        member_count: 2,
        has_unread: false,
        has_mentions: false,
        has_activity: false,
        notify: NotifyMode::AllMessages,
        last_activity_ts: now,
        last_message_sender: None,
        last_message_kind: MessagePreviewKind::None,
        last_message_body: RichText::plain(String::new()),
        last_message_service: None,
        last_message_is_own: false,
        last_message_edited: false,
    }
}

pub fn started_chat_id(user_id: &str) -> String {
    format!("!chat-{}:{DEMO_VIA}", localpart(user_id))
}

fn read_when_marked(mut room: Room, read: &HashSet<String>) -> Room {
    if read.contains(room.id.as_ref()) {
        room.has_unread = false;
        room.has_mentions = false;
        room.has_activity = false;
    }
    room
}

pub fn room_about(room_id: &RoomId) -> Option<RoomAbout> {
    let data = data();
    if let Some(room) = data.rooms.iter().find(|room| room.id == room_id.as_ref()) {
        return Some(RoomAbout {
            joined_at: room.joined_at(now_ms()),
            link: room_link(room_id, room.alias.as_deref()),
            topic: room.rich_topic(),
        });
    }
    let entry = data
        .unjoined
        .iter()
        .find(|entry| entry.id == room_id.as_ref())?;
    Some(RoomAbout {
        joined_at: None,
        link: room_link(room_id, entry.alias()),
        topic: entry.rich_topic(),
    })
}

pub fn event_link(room_id: &RoomId, event_id: &str) -> String {
    format!("{MATRIX_TO}{room_id}/{event_id}?via={DEMO_VIA}")
}

pub fn room_link_of(room_id: &RoomId) -> Option<String> {
    room_about(room_id).map(|about| about.link)
}

fn room_link(room_id: &RoomId, alias: Option<&str>) -> String {
    match alias.and_then(|alias| alias.strip_prefix('#')) {
        Some(alias) => format!("{MATRIX_TO}%23{alias}"),
        None => format!("{MATRIX_TO}{room_id}?via={DEMO_VIA}"),
    }
}

pub fn roster(room_id: &RoomId, avatar_of: impl Fn(&str) -> String) -> Vec<RosterMember> {
    let data = data();
    let names = people_names(data);
    let own = own_user();
    let fixture = data.rooms.iter().find(|room| room.id == room_id.as_ref());
    let joined_count = fixture.map(|room| room.members).or_else(|| {
        data.unjoined
            .iter()
            .find(|entry| entry.id == room_id.as_ref())
            .map(UnjoinedDto::joined_members)
    });
    let Some(joined_count) = joined_count else {
        return Vec::new();
    };
    let mut joined: Vec<String> = vec![own.to_owned()];
    let mut invited: Vec<String> = Vec::new();
    let mut roles: HashMap<String, MemberRole> = HashMap::new();
    if let Some(room) = fixture {
        let senders = data
            .timelines
            .get(&room.id)
            .into_iter()
            .flatten()
            .map(|message| message.author().0.to_owned());
        for user_id in senders.chain(room.roles.keys().cloned()) {
            if !joined.contains(&user_id) {
                joined.push(user_id);
            }
        }
        roles.extend(
            room.roles
                .iter()
                .map(|(user, role)| (user.clone(), role.to_role())),
        );
        invited.extend(room.invited.iter().cloned());
    }
    let guests = usize::try_from(joined_count)
        .unwrap_or(usize::MAX)
        .saturating_sub(joined.len());
    let mut roster: Vec<RosterMember> = joined
        .into_iter()
        .map(|user_id| (user_id, RosterSection::Joined))
        .chain((1..=guests).map(|n| (guest_id(n), RosterSection::Joined)))
        .chain(
            invited
                .into_iter()
                .map(|user_id| (user_id, RosterSection::Invited)),
        )
        .map(|(user_id, section)| RosterMember {
            display_name: names
                .get(&user_id)
                .cloned()
                .or_else(|| guest_name(&user_id)),
            avatar_mxc: Some(avatar_of(&user_id)),
            role: roles.get(&user_id).copied().unwrap_or(MemberRole::Member),
            section,
            user_id,
        })
        .collect();
    sort_roster(&mut roster);
    roster
}

pub fn profile(
    room_id: &RoomId,
    user_id: &UserId,
    avatar_of: impl Fn(&str) -> String,
) -> UserProfile {
    let member = roster(room_id, &avatar_of)
        .into_iter()
        .find(|member| member.user_id == user_id.as_ref());
    let membership = match member.as_ref().map(|member| member.section) {
        Some(RosterSection::Joined) => RoomMembership::Joined,
        Some(RosterSection::Invited) => RoomMembership::Invited,
        None => RoomMembership::Outside,
    };
    UserProfile {
        user_id: user_id.clone(),
        display_name: member
            .as_ref()
            .and_then(|member| member.display_name.clone()),
        avatar_mxc: member.as_ref().and_then(|member| member.avatar_mxc.clone()),
        role: member
            .as_ref()
            .map_or(MemberRole::Member, |member| member.role),
        membership,
        trust: IdentityTrust::Unverified,
        is_self: user_id.as_ref() == own_user(),
        pronouns: Pronouns::Known(pronouns(user_id)),
        link: user_link(user_id),
        direct_room: None,
        ignored: false,
        powers: ModerationPowers::default(),
    }
}

pub fn readers(
    room_id: &RoomId,
    user_ids: &[String],
    avatar_of: impl Fn(&str) -> String,
) -> Vec<Reader> {
    let members = roster(room_id, &avatar_of);
    user_ids
        .iter()
        .map(|user_id| {
            members
                .iter()
                .find(|member| member.user_id == *user_id)
                .map_or_else(
                    || Reader {
                        user_id: user_id.clone(),
                        display_name: person_name(user_id),
                        avatar_mxc: Some(avatar_of(user_id)),
                        role: role_of(room_id, user_id),
                    },
                    |member| Reader {
                        user_id: member.user_id.clone(),
                        display_name: member.display_name.clone(),
                        avatar_mxc: member.avatar_mxc.clone(),
                        role: member.role,
                    },
                )
        })
        .collect()
}

pub fn role_of(room_id: &RoomId, user_id: &str) -> MemberRole {
    data()
        .rooms
        .iter()
        .find(|room| room.id == room_id.as_ref())
        .and_then(|room| room.roles.get(user_id))
        .map_or(MemberRole::Member, |role| role.to_role())
}

pub fn person_name(user_id: &str) -> Option<String> {
    people_names(data())
        .remove(user_id)
        .or_else(|| guest_name(user_id))
}

pub fn user_link(user_id: &str) -> String {
    format!("{MATRIX_TO}{user_id}")
}

pub fn direct_room_with(user_id: &str) -> Option<RoomId> {
    let own = own_user();
    data()
        .rooms
        .iter()
        .filter(|room| room.is_direct())
        .find_map(|room| {
            let room_id = RoomId::new(&room.id);
            let others: Vec<String> = roster(&room_id, str::to_owned)
                .into_iter()
                .map(|member| member.user_id)
                .filter(|member| member != own)
                .collect();
            (others == [user_id]).then_some(room_id)
        })
}

pub fn guest_user(n: usize) -> String {
    guest_id(n)
}

const GUEST_PREFIX: &str = "@guest-";

fn guest_id(n: usize) -> String {
    format!("{GUEST_PREFIX}{n}:{DEMO_VIA}")
}

fn guest_name(user_id: &str) -> Option<String> {
    let rest = user_id.strip_prefix(GUEST_PREFIX)?;
    let (n, _) = rest.split_once(':')?;
    Some(format!("Guest {n}"))
}

fn people_names(data: &DemoData) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for message in data.timelines.values().flatten() {
        let (sender, name) = message.author();
        if name != OWN_NAME_IN_TIMELINES && !name.is_empty() {
            names
                .entry(sender.to_owned())
                .or_insert_with(|| name.to_owned());
        }
    }
    names
}

pub fn spaces_with(joined: &[RoomId]) -> Vec<Space> {
    let data = data();
    let mut spaces: Vec<Space> = data
        .spaces
        .iter()
        .map(SpaceDto::to_space)
        .chain(
            data.unjoined
                .iter()
                .filter(|entry| entry.space && was_joined(joined, &entry.id))
                .map(UnjoinedDto::to_space),
        )
        .collect();
    let joined_spaces: HashSet<String> = spaces.iter().map(|space| space.id.clone()).collect();
    for space in &mut spaces {
        (space.child_space_ids, space.child_room_ids) = mem::take(&mut space.child_room_ids)
            .into_iter()
            .chain(mem::take(&mut space.child_space_ids))
            .partition(|child| joined_spaces.contains(child));
    }
    spaces
}

fn was_joined(joined: &[RoomId], id: &str) -> bool {
    joined.iter().any(|room| room.as_ref() == id)
}

pub fn space_children(space_id: &RoomId) -> Vec<SpaceChild> {
    let data = data();
    let via = [DEMO_VIA.to_owned()];
    let children: Vec<&String> = if let Some(space) = data
        .spaces
        .iter()
        .find(|space| space.id() == space_id.as_ref())
    {
        space.children().collect()
    } else if let Some(entry) = data
        .unjoined
        .iter()
        .find(|entry| entry.space && entry.id == space_id.as_ref())
    {
        entry.rooms.iter().chain(&entry.spaces).collect()
    } else {
        Vec::new()
    };
    children
        .into_iter()
        .filter_map(|id| space_child(data, id, &via))
        .collect()
}

fn space_child(data: &DemoData, id: &str, via: &[String]) -> Option<SpaceChild> {
    if let Some(room) = data.rooms.iter().find(|room| room.id == id) {
        return Some(room.to_child(via));
    }
    if let Some(space) = data.spaces.iter().find(|space| space.id() == id) {
        return Some(space.to_child(via));
    }
    data.unjoined
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.to_child(via))
}

pub fn is_unjoined_avatar(mxc: &str) -> bool {
    data()
        .unjoined
        .iter()
        .any(|entry| entry.avatar() == Some(mxc))
}

pub fn room_is_encrypted(room_id: &RoomId) -> bool {
    data()
        .rooms
        .iter()
        .any(|room| room.id == room_id.as_ref() && room.encrypted)
}

pub fn sticker_packs(room_id: &RoomId) -> Vec<StickerPack> {
    data()
        .sticker_packs
        .iter()
        .filter(|pack| pack.covers(room_id))
        .map(StickerPackDto::to_pack)
        .collect()
}

pub fn sticker_image(pack_id: &str, shortcode: &str) -> Option<StickerImage> {
    data()
        .sticker_packs
        .iter()
        .find(|pack| pack.id == pack_id)?
        .to_pack()
        .images
        .into_iter()
        .find(|image| image.shortcode == shortcode)
}

pub fn messages(room_id: &RoomId) -> Vec<TimelineMessage> {
    let now = now_ms();
    let mut messages = match data().timelines.get(room_id.as_ref()) {
        Some(timeline) => timeline
            .iter()
            .map(|message| message.to_message(own_user(), now))
            .collect(),
        None => last_message_only(room_id),
    };
    if scenario().history_is_long {
        messages = repeated_history(&messages);
    }
    super::richtext::apply_scenario(&mut messages);
    super::reactions::apply_scenario(&mut messages);
    super::polls::apply_scenario(&mut messages);
    super::receipts::apply_scenario(&mut messages);
    mark_first_unread(room_id, &mut messages);
    messages
}

fn repeated_history(messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    let mut repeated = Vec::with_capacity(messages.len() * timeline::HISTORY_COPIES);
    for copy in 0..timeline::HISTORY_COPIES {
        for message in messages {
            let mut message = message.clone();
            message.unique_id = format!("{}{COPY_SUFFIX}{copy}", message.unique_id);
            message.event_id = message
                .event_id
                .as_ref()
                .map(|id| format!("{id}{COPY_SUFFIX}{copy}"));
            repeated.push(message);
        }
    }
    repeated
}

pub fn pinned_messages(room_id: &RoomId) -> Vec<PinnedMessage> {
    let pins = room_pins(room_id);
    if pins.is_empty() {
        return Vec::new();
    }
    let messages = messages(room_id);
    let mut rows: Vec<usize> = pins
        .iter()
        .filter_map(|pin| {
            messages
                .iter()
                .rposition(|message| fixture_id(message).is_some_and(|id| id == pin))
        })
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows.into_iter()
        .filter_map(|row| messages.get(row))
        .filter_map(pinned_message)
        .collect()
}

pub fn latest_pinnable(room_id: &RoomId) -> Option<PinnedMessage> {
    messages(room_id).iter().rev().find_map(pinned_message)
}

fn room_pins(room_id: &RoomId) -> &'static [String] {
    data()
        .rooms
        .iter()
        .find(|room| room.id == room_id.as_ref())
        .map_or(&[], |room| room.pinned.as_slice())
}

fn fixture_id(message: &TimelineMessage) -> Option<&str> {
    let event_id = message.event_id.as_deref()?;
    let copied = event_id
        .rsplit_once(COPY_SUFFIX)
        .filter(|(_, copy)| copy.parse::<usize>().is_ok());
    Some(copied.map_or(event_id, |(id, _)| id))
}

pub fn pinned_message(message: &TimelineMessage) -> Option<PinnedMessage> {
    if message.body.service().is_some() {
        return None;
    }
    Some(PinnedMessage {
        event_id: message.event_id.clone()?,
        kind: message.body.preview_kind(),
        body: body_preview(&message.body),
    })
}

fn unread_count(room_id: &RoomId, loaded: usize) -> usize {
    if scenario().unread_boundary_is_unresolved {
        return 0;
    }
    if scenario().read_position_precedes_history {
        return loaded;
    }
    if scenario().history_is_long {
        return loaded * timeline::UNREAD_PORTION_NUMERATOR / timeline::UNREAD_PORTION_DENOMINATOR;
    }
    data()
        .rooms
        .iter()
        .find(|room| room.id == room_id.as_ref())
        .map_or(0, |room| usize::try_from(room.unread).unwrap_or(usize::MAX))
}

fn mark_first_unread(room_id: &RoomId, messages: &mut [TimelineMessage]) {
    let unread = unread_count(room_id, messages.len());
    if unread == 0 {
        return;
    }
    let read_up_to = messages.len().saturating_sub(unread);
    if let Some(message) = messages.iter_mut().skip(read_up_to).find(|m| !m.is_own) {
        message.is_first_unread = true;
    }
}

pub fn pronouns(user_id: &str) -> Vec<String> {
    data().pronouns.get(user_id).cloned().unwrap_or_default()
}

pub fn own_composition(composer: &str) -> Composed {
    let notify_room = message_menu::permissions().notify_room;
    markdown::compose(composer, own_user(), RoomMentions::when(notify_room))
}

pub struct SentText {
    pub text: RichText,
    pub mentions_room: bool,
}

pub fn sent_text(composer: &str) -> SentText {
    let sent = own_composition(composer);
    let text = match sent.html {
        Some(html) => RichText::authored(sent.markdown, html, composer.to_owned()),
        None => RichText::plain(sent.markdown),
    };
    SentText {
        text,
        mentions_room: sent.mentions.room,
    }
}

pub fn own_message(
    sequence: u64,
    body: &str,
    reply: Option<ReplyInfo>,
    send_state: SendState,
) -> TimelineMessage {
    let id = format!("demo-sent-{sequence}");
    let settled = send_state == SendState::Sent;
    let sent = sent_text(body);
    TimelineMessage {
        unique_id: id.clone(),
        event_id: settled.then(|| id.clone()),
        local_id: (!settled).then(|| format!("local:{id}")),
        sender_pronouns: Vec::new(),
        sender: own_user().to_owned(),
        sender_display_name: Some("You".to_owned()),
        sender_avatar_url: Some(own_user().to_owned()),
        body: MessageBody::Text(sent.text),
        mentions_room: sent.mentions_room,
        timestamp: now_ms(),
        is_own: true,
        reply,
        edited: false,
        editable: true,
        is_first_unread: false,
        send_state,
        reactions: Vec::new(),
        read_by: ReadBy::default(),
    }
}

pub fn own_poll(sequence: u64, draft: &PollDraft, send_state: SendState) -> TimelineMessage {
    let poll = Poll {
        question: draft.question().to_owned(),
        disclosure: draft.disclosure(),
        choice: draft.choice(),
        answers: draft
            .answers()
            .iter()
            .enumerate()
            .map(|(index, text)| PollAnswer {
                id: format!("demo-sent-{sequence}-answer-{index}"),
                text: text.clone(),
                voters: Vec::new(),
            })
            .collect(),
        status: PollStatus::Open,
        editable: true,
    };
    TimelineMessage {
        body: MessageBody::Poll(poll),
        mentions_room: false,
        ..own_message(sequence, draft.question(), None, send_state)
    }
}

pub fn own_sticker(
    sequence: u64,
    image: &StickerImage,
    reply: Option<ReplyInfo>,
) -> TimelineMessage {
    let id = format!(
        "demo-sent-{sequence}{STICKER_ASSET_MARKER}{}",
        media::mxc_asset(&image.mxc)
    );
    let content = media::content_of(&id);
    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        local_id: None,
        sender_pronouns: Vec::new(),
        sender: own_user().to_owned(),
        sender_display_name: Some("You".to_owned()),
        sender_avatar_url: Some(own_user().to_owned()),
        body: MessageBody::Sticker {
            alt: image.body.clone(),
            meta: ImageMeta {
                width: Some(SENT_STICKER_EXTENT),
                height: Some(SENT_STICKER_EXTENT),
                mimetype: None,
                filename: None,
                thumbnail: Some(content),
            },
        },
        mentions_room: false,
        timestamp: now_ms(),
        is_own: true,
        reply,
        edited: false,
        editable: false,
        is_first_unread: false,
        send_state: SendState::default(),
        reactions: Vec::new(),
        read_by: ReadBy::default(),
    }
}

pub fn own_attachment(
    sequence: u64,
    attachment: &OutgoingAttachment,
    reply: Option<ReplyInfo>,
) -> TimelineMessage {
    let id = format!("demo-sent-{sequence}");
    let picked = &attachment.picked;
    let sent = attachment.caption.as_deref().map(sent_text);
    let mentions_room = sent.as_ref().is_some_and(|sent| sent.mentions_room);
    let caption = || sent.as_ref().map(|sent| sent.text.clone());
    let (width, height) = picked.dimensions.unzip();
    let image_meta = || ImageMeta {
        width,
        height,
        mimetype: Some(picked.mimetype.clone()),
        filename: Some(picked.filename.clone()),
        thumbnail: Some(media::content_of(&id)),
    };
    let body = if attachment.as_document {
        MessageBody::File {
            meta: FileMeta {
                filename: picked.filename.clone(),
                mimetype: Some(picked.mimetype.clone()),
                size: Some(picked.size),
            },
        }
    } else if picked.is_video() {
        MessageBody::Video {
            caption: caption(),
            meta: VideoMeta {
                image: image_meta(),
                duration: picked.duration,
                size: Some(picked.size),
            },
        }
    } else if picked.is_audio() {
        MessageBody::Audio {
            caption: caption(),
            meta: AudioMeta {
                kind: AudioKind::Track,
                file: media::content_of(&id),
                filename: picked.filename.clone(),
                mimetype: Some(picked.mimetype.clone()),
                duration: picked.duration,
                size: Some(picked.size),
                waveform: picked.waveform.clone(),
            },
        }
    } else if picked.is_image() {
        MessageBody::Image {
            caption: caption(),
            meta: image_meta(),
        }
    } else {
        MessageBody::File {
            meta: FileMeta {
                filename: picked.filename.clone(),
                mimetype: Some(picked.mimetype.clone()),
                size: Some(picked.size),
            },
        }
    };
    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        local_id: None,
        sender_pronouns: Vec::new(),
        sender: own_user().to_owned(),
        sender_display_name: Some("You".to_owned()),
        sender_avatar_url: Some(own_user().to_owned()),
        body,
        mentions_room,
        timestamp: now_ms(),
        is_own: true,
        reply,
        edited: false,
        editable: true,
        is_first_unread: false,
        send_state: SendState::default(),
        reactions: Vec::new(),
        read_by: ReadBy::default(),
    }
}

pub fn sticker_asset_in(event_id: &str) -> Option<&str> {
    event_id
        .split_once(STICKER_ASSET_MARKER)
        .map(|(_, asset)| asset)
}

pub fn body_preview(body: &MessageBody) -> RichText {
    match body {
        MessageBody::Text(text) | MessageBody::Notice(text) | MessageBody::Emote(text) => {
            text.clone()
        }
        MessageBody::Image { caption, .. }
        | MessageBody::Video { caption, .. }
        | MessageBody::Audio { caption, .. } => caption.clone().unwrap_or_default(),
        MessageBody::Sticker { alt, .. } => RichText::plain(alt.clone()),
        MessageBody::File { meta } => RichText::plain(meta.filename.clone()),
        MessageBody::Poll(poll) => RichText::plain(poll.question.clone()),
        MessageBody::Service(_) | MessageBody::UnableToDecrypt => RichText::default(),
        MessageBody::Unsupported { fallback, .. } => RichText::plain(fallback.clone()),
    }
}

pub fn sender_label(message: &TimelineMessage) -> String {
    message
        .sender_display_name
        .clone()
        .unwrap_or_else(|| message.sender.clone())
}

fn last_message_only(room_id: &RoomId) -> Vec<TimelineMessage> {
    let Some(dto) = data().rooms.iter().find(|room| room.id == room_id.as_ref()) else {
        return Vec::new();
    };
    if dto.last_message.body.is_empty() {
        return Vec::new();
    }

    vec![synthesized_message(dto, &dto.to_room(now_ms(), None))]
}

fn synthesized_message(dto: &RoomDto, room: &Room) -> TimelineMessage {
    let (sender, display_name) = if dto.last_message.own {
        (own_user().to_owned(), "You".to_owned())
    } else {
        (
            dto.last_message
                .sender_id
                .clone()
                .unwrap_or_else(|| UNKNOWN_SENDER.to_owned()),
            dto.last_message.sender.clone().unwrap_or_default(),
        )
    };
    let id = format!("demo-{}-last", dto.id.trim_start_matches('!'));

    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        local_id: None,
        sender_pronouns: pronouns(&sender),
        sender_avatar_url: Some(sender.clone()),
        sender,
        sender_display_name: Some(display_name),
        body: MessageBody::Text(room.last_message_body.clone()),
        mentions_room: false,
        timestamp: room.last_activity_ts,
        is_own: dto.last_message.own,
        reply: None,
        edited: room.last_message_edited,
        editable: dto.last_message.own,
        is_first_unread: false,
        send_state: SendState::default(),
        reactions: Vec::new(),
        read_by: ReadBy::default(),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default()
}
