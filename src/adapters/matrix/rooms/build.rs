use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures_util::{StreamExt, stream};
use matrix_sdk::deserialized_responses::SyncOrStrippedState;
use matrix_sdk::latest_events::LatestEventValue;
use matrix_sdk::notification_settings::{
    IsEncrypted, IsOneToOne, NotificationSettings, RoomNotificationMode,
};
use matrix_sdk::ruma::events::room::member::MembershipState;
use matrix_sdk::ruma::events::room::message::{Relation, RoomMessageEventContent};
use matrix_sdk::ruma::events::space::child::SpaceChildEventContent;
use matrix_sdk::ruma::events::space_order::SpaceOrderEventContent;
use matrix_sdk::ruma::events::{
    AnyMessageLikeEventContent, AnySyncMessageLikeEvent, AnySyncStateEvent, AnySyncTimelineEvent,
    SyncMessageLikeEvent, SyncStateEvent,
};
use matrix_sdk::ruma::{OwnedUserId, UserId};
use matrix_sdk::{Client, Room};

use crate::adapters::matrix::preview::{self, MessagePreview};
use crate::domain::message::{MessagePreviewKind, ServiceEvent};
use crate::domain::room::{NotifyMode, Room as DomainRoom, RoomId, Space as DomainSpace};

const SEED_INFLIGHT: usize = 16;

fn room_avatar_mxc(room: &Room, is_direct: bool) -> Option<String> {
    if let Some(mxc) = room.avatar_url() {
        return Some(mxc.to_string());
    }
    if !is_direct {
        return None;
    }
    room.heroes()
        .first()
        .and_then(|hero| hero.avatar_url.as_ref())
        .map(ToString::to_string)
}

pub(super) struct UnreadFlags {
    pub(super) has_unread: bool,
    pub(super) has_mentions: bool,
    pub(super) has_activity: bool,
    pub(super) notify: NotifyMode,
}

async fn notify_mode(room: &Room, settings: &NotificationSettings) -> NotifyMode {
    let mode = match settings
        .get_user_defined_room_notification_mode(room.room_id())
        .await
    {
        Some(mode) => mode,
        None => {
            settings
                .get_default_room_notification_mode(
                    IsEncrypted::from(room.encryption_state().is_encrypted()),
                    IsOneToOne::from(room.active_members_count() == 2),
                )
                .await
        }
    };
    match mode {
        RoomNotificationMode::AllMessages => NotifyMode::AllMessages,
        RoomNotificationMode::MentionsAndKeywordsOnly => NotifyMode::MentionsOnly,
        RoomNotificationMode::Mute => NotifyMode::Muted,
    }
}

pub(super) async fn unread_flags(room: &Room, settings: &NotificationSettings) -> UnreadFlags {
    let messages = room.num_unread_messages();
    let notifications = room.num_unread_notifications();
    let mentions = room.num_unread_mentions();
    let notify = notify_mode(room, settings).await;

    tracing::debug!(
        room = %room.room_id(),
        messages,
        notifications,
        mentions,
        ?notify,
        "unread flags"
    );
    UnreadFlags {
        has_unread: notifications > 0,
        has_mentions: mentions > 0,
        has_activity: messages > 0,
        notify,
    }
}

pub(super) async fn build_single_room(room: &Room, settings: &NotificationSettings) -> DomainRoom {
    let display_name = room
        .cached_display_name()
        .map(|dn| dn.to_string())
        .unwrap_or_default();
    let flags = unread_flags(room, settings).await;
    let is_direct = room.is_direct().await.unwrap_or_default();
    let member_count = room.joined_members_count();
    let last_activity_ts: u64 = room.latest_event_timestamp().map_or(0, |ts| ts.0.into());
    let last_message = build_last_message(room, is_direct).await;
    DomainRoom {
        id: RoomId::new(room.room_id().to_string()),
        display_name,
        avatar_mxc: room_avatar_mxc(room, is_direct),
        is_direct,
        member_count,
        has_unread: flags.has_unread,
        has_mentions: flags.has_mentions,
        has_activity: flags.has_activity,
        notify: flags.notify,
        last_activity_ts,
        last_message_sender: last_message.sender,
        last_message_kind: last_message.kind,
        last_message_body: last_message.body,
        last_message_service: last_message.service,
        last_message_is_own: last_message.is_own,
        last_message_edited: last_message.edited,
    }
}

#[derive(Default)]
struct LastMessage {
    sender: Option<String>,
    kind: MessagePreviewKind,
    body: String,
    service: Option<ServiceEvent>,
    is_own: bool,
    edited: bool,
}

async fn build_last_message(room: &Room, is_direct: bool) -> LastMessage {
    let Some((preview, sender_id)) = latest_message_preview(&room.latest_event()) else {
        return LastMessage::default();
    };

    let is_own = sender_id
        .as_ref()
        .is_none_or(|sender| sender == room.own_user_id());

    let is_service = preview.service.is_some();
    let sender = if is_own || (is_direct && !is_service) {
        None
    } else {
        match &sender_id {
            Some(sender) => Some(resolve_sender_name(room, sender).await),
            None => None,
        }
    };

    LastMessage {
        sender,
        kind: preview.kind,
        body: preview.body,
        service: preview.service,
        is_own,
        edited: preview.edited,
    }
}

async fn resolve_sender_name(room: &Room, user_id: &UserId) -> String {
    if let Ok(Some(member)) = room.get_member_no_sync(user_id).await
        && let Some(name) = member.display_name()
    {
        return name.to_owned();
    }
    user_id.localpart().to_owned()
}

fn latest_message_preview(
    value: &LatestEventValue,
) -> Option<(MessagePreview, Option<OwnedUserId>)> {
    match value {
        LatestEventValue::Remote(event) => {
            let preview = preview_from_event(&event.raw().deserialize().ok()?)?;
            Some((preview, event.sender()))
        }
        LatestEventValue::LocalIsSending(local)
        | LatestEventValue::LocalHasBeenSent { value: local, .. }
        | LatestEventValue::LocalCannotBeSent(local) => match local.content.deserialize().ok()? {
            AnyMessageLikeEventContent::RoomMessage(message) => {
                Some((preview_from_message_content(&message), None))
            }
            _ => None,
        },
        LatestEventValue::None | LatestEventValue::RemoteInvite { .. } => None,
    }
}

fn preview_from_event(event: &AnySyncTimelineEvent) -> Option<MessagePreview> {
    match event {
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
            SyncMessageLikeEvent::Original(message),
        )) => Some(preview_from_message_content(&message.content)),
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomEncrypted(_)) => {
            Some(MessagePreview::labelled(MessagePreviewKind::Encrypted))
        }
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::Sticker(_)) => {
            Some(MessagePreview::labelled(MessagePreviewKind::Sticker))
        }
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::CallInvite(_)) => {
            Some(MessagePreview::service(ServiceEvent::CallStarted))
        }
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RtcNotification(_)) => {
            Some(MessagePreview::service(ServiceEvent::CallNotification))
        }
        AnySyncTimelineEvent::State(AnySyncStateEvent::RoomMember(member))
            if matches!(member.membership(), MembershipState::Knock) =>
        {
            Some(MessagePreview::service(ServiceEvent::Knocked))
        }
        _ => None,
    }
}

fn preview_from_message_content(content: &RoomMessageEventContent) -> MessagePreview {
    if let Some(Relation::Replacement(replacement)) = &content.relates_to {
        let mut preview = preview::from_msgtype(&replacement.new_content.msgtype);
        preview.edited = true;
        preview
    } else {
        preview::from_msgtype(&content.msgtype)
    }
}

pub(super) async fn build_rooms(
    client: &Client,
    settings: &NotificationSettings,
) -> HashMap<String, Arc<DomainRoom>> {
    let joined = client
        .joined_rooms()
        .into_iter()
        .filter(|room| !room.is_space());
    stream::iter(joined)
        .map(|room| async move { build_single_room(&room, settings).await })
        .buffer_unordered(SEED_INFLIGHT)
        .map(|room| (room.id.to_string(), Arc::new(room)))
        .collect()
        .await
}

async fn space_child_ids(space: &Room) -> Vec<String> {
    let events = match space
        .get_state_events_static::<SpaceChildEventContent>()
        .await
    {
        Ok(events) => events,
        Err(e) => {
            tracing::debug!(space = %space.room_id(), "failed to read space children: {e}");
            return Vec::new();
        }
    };
    events
        .into_iter()
        .filter_map(|raw| match raw.deserialize() {
            Ok(SyncOrStrippedState::Sync(SyncStateEvent::Original(event))) => {
                (!event.content.via.is_empty()).then(|| event.state_key.to_string())
            }
            _ => None,
        })
        .collect()
}

async fn space_order(space: &Room) -> Option<String> {
    let raw = space
        .account_data_static::<SpaceOrderEventContent>()
        .await
        .ok()??;
    let event = raw.deserialize().ok()?;
    Some(event.content.order.to_string())
}

pub(super) async fn build_spaces_meta(client: &Client) -> Vec<DomainSpace> {
    let joined_spaces = client.joined_space_rooms();
    let space_ids: HashSet<String> = joined_spaces
        .iter()
        .map(|space| space.room_id().to_string())
        .collect();

    let space_ids = &space_ids;
    stream::iter(joined_spaces)
        .map(|space| async move {
            let name = space
                .cached_display_name()
                .map(|dn| dn.to_string())
                .unwrap_or_default();
            let (child_space_ids, child_room_ids) = space_child_ids(&space)
                .await
                .into_iter()
                .partition(|child| space_ids.contains(child));
            let avatar_mxc = space.avatar_url().map(|mxc| mxc.to_string());
            let order = space_order(&space).await;
            DomainSpace {
                id: space.room_id().to_string(),
                name,
                avatar_mxc,
                child_room_ids,
                child_space_ids,
                order,
                alert: false,
                mention: false,
                hint: false,
            }
        })
        .buffered(SEED_INFLIGHT)
        .collect()
        .await
}
