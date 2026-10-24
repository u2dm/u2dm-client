use matrix_sdk::Room;
use matrix_sdk::deserialized_responses::DisplayName;
use matrix_sdk::room::RoomMember;
use matrix_sdk::ruma::UserId;
use matrix_sdk::ruma::events::room::member::MembershipState;

use crate::domain::member_name::{MemberName, NamedUser};

pub(super) async fn named_member(room: &Room, user_id: &UserId) -> NamedUser {
    named_member_with_avatar(room, user_id).await.0
}

pub(super) async fn named_member_with_avatar(
    room: &Room,
    user_id: &UserId,
) -> (NamedUser, Option<String>) {
    let (name, avatar_mxc) = match room.get_member_no_sync(user_id).await {
        Ok(Some(member)) => (
            member_name(room, &member).await,
            member.avatar_url().map(ToString::to_string),
        ),
        Ok(None) => (MemberName::Unset, None),
        Err(e) => {
            tracing::debug!(%user_id, "could not read the member's name: {e}");
            (MemberName::Unset, None)
        }
    };
    (NamedUser::new(user_id.as_str(), name), avatar_mxc)
}

pub(super) async fn member_name(room: &Room, member: &RoomMember) -> MemberName {
    let Some(name) = member.display_name() else {
        return MemberName::Unset;
    };
    let ambiguous = if counts_for_ambiguity(member.membership()) {
        member.name_ambiguous()
    } else {
        used_by_another_member(room, member.user_id(), name).await
    };
    MemberName::new(Some(name), ambiguous)
}

fn counts_for_ambiguity(membership: &MembershipState) -> bool {
    matches!(
        membership,
        MembershipState::Join | MembershipState::Invite | MembershipState::Knock
    )
}

async fn used_by_another_member(room: &Room, user_id: &UserId, name: &str) -> bool {
    let name = DisplayName::new(name);
    if name.is_inherently_ambiguous() {
        return true;
    }
    let client = room.client();
    match client
        .state_store()
        .get_users_with_display_name(room.room_id(), &name)
        .await
    {
        Ok(users) => users.iter().any(|user| user != user_id),
        Err(error) => {
            tracing::debug!(%error, %user_id, "could not read who else uses a display name");
            true
        }
    }
}
