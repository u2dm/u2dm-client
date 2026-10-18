use matrix_sdk::ruma::RoomVersionId;
use matrix_sdk::ruma::events::room::guest_access::GuestAccess as SdkGuestAccess;
use matrix_sdk::ruma::events::room::history_visibility::HistoryVisibility as SdkHistoryVisibility;
use matrix_sdk::ruma::room::{JoinRuleSummary, RestrictedSummary};
use matrix_sdk::ruma::room_version_rules::EventIdFormatVersion;

use crate::domain::event_size::EventIds;
use crate::domain::room::RoomId;
use crate::domain::room_state::{GuestAccess, HistoryVisibility};
use crate::domain::space_index::JoinRule;

pub(super) fn join_rule(summary: &JoinRuleSummary) -> JoinRule {
    match summary {
        JoinRuleSummary::Public => JoinRule::Public,
        JoinRuleSummary::Restricted(rule) => JoinRule::Restricted {
            allowed: allowed_rooms(rule),
        },
        JoinRuleSummary::KnockRestricted(rule) => JoinRule::KnockRestricted {
            allowed: allowed_rooms(rule),
        },
        JoinRuleSummary::Knock => JoinRule::Knock,
        JoinRuleSummary::Invite => JoinRule::Invite,
        JoinRuleSummary::Private => JoinRule::Private,
        _ => JoinRule::Unsupported,
    }
}

fn allowed_rooms(rule: &RestrictedSummary) -> Vec<RoomId> {
    rule.allowed_room_ids
        .iter()
        .map(|id| RoomId::new(id.to_string()))
        .collect()
}

pub(super) fn event_ids(version: Option<&RoomVersionId>) -> EventIds {
    match version
        .and_then(RoomVersionId::rules)
        .map(|rules| rules.event_id_format)
    {
        Some(EventIdFormatVersion::V2 | EventIdFormatVersion::V3) => EventIds::Hashed,
        _ => EventIds::WithDomain,
    }
}

pub(super) fn history_visibility(visibility: &SdkHistoryVisibility) -> HistoryVisibility {
    match visibility {
        SdkHistoryVisibility::WorldReadable => HistoryVisibility::WorldReadable,
        SdkHistoryVisibility::Shared => HistoryVisibility::Shared,
        SdkHistoryVisibility::Invited => HistoryVisibility::Invited,
        SdkHistoryVisibility::Joined => HistoryVisibility::Joined,
        _ => HistoryVisibility::Unknown,
    }
}

pub(super) fn guest_access(access: &SdkGuestAccess) -> GuestAccess {
    match access {
        SdkGuestAccess::CanJoin => GuestAccess::CanJoin,
        SdkGuestAccess::Forbidden => GuestAccess::Forbidden,
        _ => GuestAccess::Unknown,
    }
}
