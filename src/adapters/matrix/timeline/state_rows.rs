use std::collections::HashSet;

use matrix_sdk::room::power_levels::power_level_user_changes;
use matrix_sdk::ruma::UserId;
use matrix_sdk::ruma::events::room::canonical_alias::RoomCanonicalAliasEventContent;
use matrix_sdk::ruma::events::room::create::RoomCreateEventContent;
use matrix_sdk::ruma::events::room::guest_access::RoomGuestAccessEventContent;
use matrix_sdk::ruma::events::room::history_visibility::RoomHistoryVisibilityEventContent;
use matrix_sdk::ruma::events::room::join_rules::RoomJoinRulesEventContent;
use matrix_sdk::ruma::events::room::name::RoomNameEventContent;
use matrix_sdk::ruma::events::room::pinned_events::RoomPinnedEventsEventContent;
use matrix_sdk::ruma::events::room::power_levels::RoomPowerLevelsEventContent;
use matrix_sdk::ruma::events::room::server_acl::RoomServerAclEventContent;
use matrix_sdk::ruma::events::room::third_party_invite::RoomThirdPartyInviteEventContent;
use matrix_sdk::ruma::events::{StateEventContentChange, StateEventType};
use matrix_sdk::ruma::room::JoinRuleSummary;
use matrix_sdk_ui::timeline::{
    AnyOtherStateEventContentChange, EventTimelineItem, RoomPinnedEventsChange, TimelineItemContent,
};
use serde::Deserialize;
use serde_json::to_value;

use crate::adapters::matrix::room_state::{guest_access, history_visibility, join_rule};
use crate::domain::message::ServiceEvent;
use crate::domain::room_state::PowerChange;

pub(super) fn other_state_to_service(
    state: &AnyOtherStateEventContentChange,
) -> Option<ServiceEvent> {
    match state {
        AnyOtherStateEventContentChange::RoomName(change) => room_name(change),
        AnyOtherStateEventContentChange::RoomTopic(_) => Some(ServiceEvent::RoomTopicChanged),
        AnyOtherStateEventContentChange::RoomAvatar(_) => Some(ServiceEvent::RoomAvatarChanged),
        AnyOtherStateEventContentChange::RoomCreate(change) => Some(room_creation(change)),
        AnyOtherStateEventContentChange::RoomTombstone(_) => Some(ServiceEvent::RoomUpgraded),
        AnyOtherStateEventContentChange::RoomEncryption(_) => Some(ServiceEvent::EncryptionEnabled),
        AnyOtherStateEventContentChange::RoomJoinRules(change) => join_rules(change),
        AnyOtherStateEventContentChange::RoomHistoryVisibility(change) => history(change),
        AnyOtherStateEventContentChange::RoomGuestAccess(change) => guests(change),
        AnyOtherStateEventContentChange::RoomServerAcl(change) => server_acl(change),
        AnyOtherStateEventContentChange::RoomCanonicalAlias(change) => addresses(change),
        AnyOtherStateEventContentChange::RoomPinnedEvents(change) => pins(change),
        AnyOtherStateEventContentChange::RoomPowerLevels(change) => power_levels(change),
        AnyOtherStateEventContentChange::RoomThirdPartyInvite(change) => third_party_invite(change),
        _ => None,
    }
}

pub(super) fn unparsed_state(event: &EventTimelineItem) -> Option<ServiceEvent> {
    let TimelineItemContent::FailedToParseState {
        event_type,
        state_key,
        ..
    } = event.content()
    else {
        return None;
    };
    match event_type {
        StateEventType::RoomName if state_key.is_empty() => Some(ServiceEvent::RoomNameRemoved),
        StateEventType::RoomThirdPartyInvite => revoked_invite(event),
        _ => None,
    }
}

fn room_name(change: &StateEventContentChange<RoomNameEventContent>) -> Option<ServiceEvent> {
    let StateEventContentChange::Original { content, .. } = change else {
        return None;
    };
    Some(if content.name.trim().is_empty() {
        ServiceEvent::RoomNameRemoved
    } else {
        ServiceEvent::RoomNameChanged {
            name: content.name.clone(),
        }
    })
}

fn room_creation(change: &StateEventContentChange<RoomCreateEventContent>) -> ServiceEvent {
    match change {
        StateEventContentChange::Original { content, .. } if content.predecessor.is_some() => {
            ServiceEvent::UpgradedRoomCreated
        }
        StateEventContentChange::Original { .. } | StateEventContentChange::Redacted(_) => {
            ServiceEvent::RoomCreated
        }
    }
}

fn join_rules(change: &StateEventContentChange<RoomJoinRulesEventContent>) -> Option<ServiceEvent> {
    let rule = match change {
        StateEventContentChange::Original {
            content,
            prev_content,
        } => {
            if prev_content
                .as_ref()
                .is_some_and(|prev| prev.join_rule == content.join_rule)
            {
                return None;
            }
            &content.join_rule
        }
        StateEventContentChange::Redacted(redacted) => &redacted.join_rule,
    };
    Some(ServiceEvent::JoinRuleChanged(join_rule(
        &JoinRuleSummary::from(rule.clone()),
    )))
}

fn history(
    change: &StateEventContentChange<RoomHistoryVisibilityEventContent>,
) -> Option<ServiceEvent> {
    let visibility = match change {
        StateEventContentChange::Original {
            content,
            prev_content,
        } => {
            if prev_content
                .as_ref()
                .is_some_and(|prev| prev.history_visibility == content.history_visibility)
            {
                return None;
            }
            &content.history_visibility
        }
        StateEventContentChange::Redacted(redacted) => &redacted.history_visibility,
    };
    Some(ServiceEvent::HistoryVisibilityChanged(history_visibility(
        visibility,
    )))
}

fn guests(change: &StateEventContentChange<RoomGuestAccessEventContent>) -> Option<ServiceEvent> {
    let StateEventContentChange::Original {
        content,
        prev_content,
    } = change
    else {
        return None;
    };
    let before = prev_content
        .as_ref()
        .and_then(|prev| prev.guest_access.as_ref());
    (before != Some(&content.guest_access))
        .then(|| ServiceEvent::GuestAccessChanged(guest_access(&content.guest_access)))
}

fn server_acl(change: &StateEventContentChange<RoomServerAclEventContent>) -> Option<ServiceEvent> {
    if let StateEventContentChange::Original {
        content,
        prev_content: Some(prev),
    } = change
        && prev.allow == content.allow
        && prev.deny == content.deny
        && prev.allow_ip_literals == content.allow_ip_literals
    {
        return None;
    }
    Some(ServiceEvent::ServerAclChanged)
}

fn addresses(
    change: &StateEventContentChange<RoomCanonicalAliasEventContent>,
) -> Option<ServiceEvent> {
    let StateEventContentChange::Original {
        content,
        prev_content,
    } = change
    else {
        return None;
    };
    let before = prev_content.as_ref();
    if content.alias.as_ref() != before.and_then(|prev| prev.alias.as_ref()) {
        return Some(match &content.alias {
            Some(alias) => ServiceEvent::MainAddressSet {
                alias: alias.to_string(),
            },
            None => ServiceEvent::MainAddressRemoved,
        });
    }
    let alternatives: HashSet<_> = content.alt_aliases.iter().collect();
    let alternatives_before: HashSet<_> = before
        .map(|prev| prev.alt_aliases.iter().collect())
        .unwrap_or_default();
    (alternatives != alternatives_before).then_some(ServiceEvent::AlternativeAddressesChanged)
}

fn pins(change: &StateEventContentChange<RoomPinnedEventsEventContent>) -> Option<ServiceEvent> {
    let StateEventContentChange::Original {
        content,
        prev_content,
    } = change
    else {
        return None;
    };
    let unchanged = match prev_content {
        Some(prev) => prev.pinned.as_ref() == Some(&content.pinned),
        None => content.pinned.is_empty(),
    };
    if unchanged {
        return None;
    }
    Some(match RoomPinnedEventsChange::from(change) {
        RoomPinnedEventsChange::Added => ServiceEvent::MessagePinned,
        RoomPinnedEventsChange::Removed => ServiceEvent::MessageUnpinned,
        RoomPinnedEventsChange::Changed => ServiceEvent::PinnedMessagesChanged,
    })
}

fn power_levels(
    change: &StateEventContentChange<RoomPowerLevelsEventContent>,
) -> Option<ServiceEvent> {
    let StateEventContentChange::Original {
        content,
        prev_content,
    } = change
    else {
        return None;
    };
    let before = prev_content.as_ref()?;
    if same_levels(content, before) {
        return None;
    }
    let mut changed = power_level_user_changes(content, prev_content).into_iter();
    let power = match (changed.next(), changed.next()) {
        (Some((user, level)), None) => {
            PowerChange::of_user(user.as_str(), level_of(before, &user), level)
        }
        _ => PowerChange::Levels,
    };
    Some(ServiceEvent::PowerLevelsChanged(power))
}

fn same_levels(now: &RoomPowerLevelsEventContent, before: &RoomPowerLevelsEventContent) -> bool {
    match (to_value(now), to_value(before)) {
        (Ok(now), Ok(before)) => now == before,
        _ => false,
    }
}

fn level_of(levels: &RoomPowerLevelsEventContent, user: &UserId) -> i64 {
    i64::from(
        levels
            .users
            .get(user)
            .copied()
            .unwrap_or(levels.users_default),
    )
}

fn third_party_invite(
    change: &StateEventContentChange<RoomThirdPartyInviteEventContent>,
) -> Option<ServiceEvent> {
    let StateEventContentChange::Original { content, .. } = change else {
        return None;
    };
    (!content.display_name.trim().is_empty()).then(|| ServiceEvent::ThirdPartyInvited {
        name: content.display_name.clone(),
    })
}

#[derive(Deserialize)]
struct StateJson<C> {
    content: C,
    unsigned: Option<UnsignedJson<C>>,
}

#[derive(Deserialize)]
struct UnsignedJson<C> {
    prev_content: Option<C>,
}

#[derive(Deserialize)]
struct InviteJson {
    #[serde(default)]
    display_name: Option<String>,
}

fn revoked_invite(event: &EventTimelineItem) -> Option<ServiceEvent> {
    let state = event
        .original_json()?
        .deserialize_as_unchecked::<StateJson<InviteJson>>()
        .ok()?;
    if named(state.content.display_name.as_deref()) {
        return None;
    }
    let name = state.unsigned?.prev_content?.display_name?;
    named(Some(name.as_str())).then_some(ServiceEvent::ThirdPartyInviteRevoked { name })
}

fn named(display_name: Option<&str>) -> bool {
    display_name.is_some_and(|name| !name.trim().is_empty())
}
