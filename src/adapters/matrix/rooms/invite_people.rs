use std::sync::Arc;

use async_trait::async_trait;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::ruma::api::client::membership::invite_user::v3::{
    InvitationRecipient, InviteUserId, Request as InviteRequest,
};
use matrix_sdk::ruma::api::client::user_directory::search_users;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::profile::{AvatarUrl, DisplayName};
use matrix_sdk::ruma::{OwnedMxcUri, OwnedUserId, UInt};
use matrix_sdk::{Client, HttpError, Room};

use crate::adapters::matrix::media::fetch_avatar_thumbnails;
use crate::adapters::matrix::permissions::may_invite;
use crate::adapters::matrix::session::ClientHandle;
use crate::adapters::matrix::user_info::{membership, own_user, parse_user};
use crate::domain::invite_people::{
    InviteFailure, InviteResult, PEOPLE_SEARCH_LIMIT, PeopleFound, PeopleSearchFailure, Person,
    SearchResult,
};
use crate::domain::member_name::MemberName;
use crate::domain::room::RoomId;
use crate::domain::user_info::{RoomMembership, UserId};
use crate::ports::matrix::InvitePeoplePort;

pub(in crate::adapters::matrix) struct MatrixInvitePeople {
    matrix: Arc<ClientHandle>,
}

impl MatrixInvitePeople {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

struct Listing {
    user_id: OwnedUserId,
    display_name: Option<String>,
    avatar_url: Option<OwnedMxcUri>,
}

#[async_trait]
impl InvitePeoplePort for MatrixInvitePeople {
    async fn search_people(&self, room_id: &RoomId, term: &str) -> SearchResult {
        let client = self
            .matrix
            .client()
            .await
            .map_err(|_| PeopleSearchFailure::Failed)?;
        let own = own_user(&client).map_err(|_| PeopleSearchFailure::Failed)?;
        let term = term.trim();
        let typed = OwnedUserId::try_from(term)
            .ok()
            .filter(|typed| *typed != own);
        let (listings, limited) = match search_directory(&client, term).await {
            Ok(response) => (
                response
                    .results
                    .into_iter()
                    .map(|user| Listing {
                        user_id: user.user_id,
                        display_name: user.display_name,
                        avatar_url: user.avatar_url,
                    })
                    .collect::<Vec<_>>(),
                response.limited,
            ),
            Err(e) if typed.is_some() => {
                tracing::warn!("the user directory failed, offering the typed user id: {e}");
                (Vec::new(), false)
            }
            Err(e) => {
                tracing::warn!("the user directory failed: {e}");
                return Err(search_failure(&e));
            }
        };
        let room = self.matrix.room(room_id).await.ok();
        let mut people = Vec::with_capacity(listings.len() + 1);
        if let Some(typed) =
            typed.filter(|typed| !listings.iter().any(|listing| listing.user_id == *typed))
        {
            let listing = typed_listing(&client, typed).await;
            people.push(person(room.as_ref(), listing).await);
        }
        for listing in listings
            .into_iter()
            .filter(|listing| listing.user_id != own)
        {
            people.push(person(room.as_ref(), listing).await);
        }
        Ok(PeopleFound { people, limited })
    }

    async fn invite(
        &self,
        room_id: &RoomId,
        user_id: &UserId,
        reason: Option<&str>,
    ) -> InviteResult {
        let room = self
            .matrix
            .room(room_id)
            .await
            .map_err(|_| InviteFailure::Failed)?;
        let user = parse_user(user_id).map_err(|_| InviteFailure::Failed)?;
        if !may_invite(&room).await {
            tracing::warn!(%room_id, "the power levels do not let you invite");
            return Err(InviteFailure::Refused);
        }
        let mut recipient = InviteUserId::new(user);
        recipient.reason = reason.map(str::to_owned);
        let request = InviteRequest::new(
            room.room_id().to_owned(),
            InvitationRecipient::UserId(recipient),
        );
        room.client()
            .send(request)
            .with_request_config(RequestConfig::short_retry())
            .await
            .map_err(|e| {
                tracing::warn!(%room_id, %user_id, "the invite failed: {e}");
                invite_failure(&e)
            })?;
        room.mark_members_missing();
        tracing::info!(%room_id, %user_id, "invited someone");
        Ok(())
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        let Ok(client) = self.matrix.client().await else {
            return 0;
        };
        fetch_avatar_thumbnails(&client, self.matrix.media(), mxcs).await
    }
}

async fn search_directory(
    client: &Client,
    term: &str,
) -> Result<search_users::v3::Response, HttpError> {
    let mut request = search_users::v3::Request::new(term.to_owned());
    request.limit = UInt::from(PEOPLE_SEARCH_LIMIT);
    client
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await
}

async fn typed_listing(client: &Client, user_id: OwnedUserId) -> Listing {
    match client.account().fetch_user_profile_of(&user_id).await {
        Ok(profile) => Listing {
            display_name: profile.get_static::<DisplayName>().ok().flatten(),
            avatar_url: profile.get_static::<AvatarUrl>().ok().flatten(),
            user_id,
        },
        Err(e) => {
            tracing::debug!(%user_id, "the typed user id has no profile to show: {e}");
            Listing {
                user_id,
                display_name: None,
                avatar_url: None,
            }
        }
    }
}

async fn person(room: Option<&Room>, listing: Listing) -> Person {
    let membership = match room {
        Some(room) => {
            let member = room
                .get_member_no_sync(&listing.user_id)
                .await
                .ok()
                .flatten();
            membership(room, member.as_ref())
        }
        None => RoomMembership::Unknown,
    };
    Person {
        user_id: UserId::new(listing.user_id.as_str()),
        name: MemberName::new(listing.display_name.as_deref(), false),
        avatar_mxc: listing.avatar_url.map(|mxc| mxc.to_string()),
        membership,
    }
}

fn search_failure(error: &HttpError) -> PeopleSearchFailure {
    match error.client_api_error_kind() {
        Some(ErrorKind::LimitExceeded(_)) => PeopleSearchFailure::RateLimited,
        _ => PeopleSearchFailure::Failed,
    }
}

fn invite_failure(error: &HttpError) -> InviteFailure {
    match error.client_api_error_kind() {
        Some(ErrorKind::InviteBlocked) => InviteFailure::Blocked,
        Some(ErrorKind::Forbidden) => InviteFailure::Refused,
        Some(ErrorKind::LimitExceeded(_)) => InviteFailure::RateLimited,
        _ => InviteFailure::Failed,
    }
}
