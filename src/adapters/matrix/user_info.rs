use std::sync::Arc;

use async_trait::async_trait;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::room::RoomMember;
use matrix_sdk::ruma::api::client::config::{get_global_account_data, set_global_account_data};
use matrix_sdk::ruma::api::client::membership::{ban_user, kick_user, unban_user};
use matrix_sdk::ruma::api::client::room::create_room;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::direct::DirectEventContent;
use matrix_sdk::ruma::events::ignored_user_list::{IgnoredUser, IgnoredUserListEventContent};
use matrix_sdk::ruma::events::room::encryption::RoomEncryptionEventContent;
use matrix_sdk::ruma::events::room::member::MembershipState;
use matrix_sdk::ruma::events::{GlobalAccountDataEventType, InitialStateEvent};
use matrix_sdk::ruma::profile::{AvatarUrl, DisplayName};
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId};
use matrix_sdk::{Client, Room};
use tokio::sync::Semaphore;

use super::media::fetch_avatar_thumbnails;
use super::permissions::{member_role, moderation_powers};
use super::profile::{PronounCache, pronouns_of};
use super::session::ClientHandle;
use crate::domain::room::RoomId;
use crate::domain::user_info::{
    GlobalProfile, IdentityTrust, IgnoreChange, Moderation, Pronouns, RoomMembership, UserId,
    UserProfile,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::UserInfoPort;

pub(super) struct MatrixUserInfo {
    matrix: Arc<ClientHandle>,
    pronouns: Arc<PronounCache>,
    direct_writes: Semaphore,
}

impl MatrixUserInfo {
    pub(super) fn new(matrix: Arc<ClientHandle>, pronouns: Arc<PronounCache>) -> Self {
        Self {
            matrix,
            pronouns,
            direct_writes: Semaphore::new(1),
        }
    }

    async fn mark_direct(
        &self,
        client: &Client,
        room_id: &OwnedRoomId,
        user: &OwnedUserId,
    ) -> Result<()> {
        let _write = self
            .direct_writes
            .acquire()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        let own = own_user(client)?;
        let mut direct = current_direct(client, &own).await?;
        direct.entry(user.into()).or_default().push(room_id.clone());
        let request = set_global_account_data::v3::Request::new(own, &direct)
            .map_err(|e| AppError::Other(e.to_string()))?;
        client
            .send(request)
            .with_request_config(RequestConfig::short_retry())
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }

    async fn trust(&self, user: &OwnedUserId) -> IdentityTrust {
        let Ok(client) = self.matrix.client().await else {
            return IdentityTrust::Unverified;
        };
        match client.encryption().get_user_identity(user).await {
            Ok(Some(identity)) if identity.is_verified() => IdentityTrust::Verified,
            Ok(_) => IdentityTrust::Unverified,
            Err(e) => {
                tracing::debug!(%user, "could not read the user's identity: {e}");
                IdentityTrust::Unverified
            }
        }
    }

    fn cached_pronouns(&self, user_id: &UserId) -> Pronouns {
        if self.pronouns.needs_fetch(user_id) {
            Pronouns::Unknown
        } else {
            Pronouns::Known(self.pronouns.resolved(user_id))
        }
    }
}

#[async_trait]
impl UserInfoPort for MatrixUserInfo {
    async fn profile(&self, room_id: &RoomId, user_id: &UserId) -> Result<UserProfile> {
        let room = self.matrix.room(room_id).await?;
        let user = parse_user(user_id)?;
        let member = room
            .get_member_no_sync(&user)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        let levels = room.power_levels_or_default().await;
        let is_self = *user == *room.own_user_id();
        let client = room.client();
        let direct_room = client
            .get_dm_room(&user)
            .map(|direct| RoomId::new(direct.room_id().to_string()));
        let ignored = client.is_user_ignored(&user).await;
        let trust = if is_self {
            IdentityTrust::Unverified
        } else {
            self.trust(&user).await
        };
        Ok(UserProfile {
            user_id: user_id.clone(),
            display_name: member
                .as_ref()
                .and_then(|member| member.display_name())
                .map(ToOwned::to_owned),
            avatar_mxc: member
                .as_ref()
                .and_then(|member| member.avatar_url())
                .map(ToString::to_string),
            role: member_role(levels.for_user(&user)),
            membership: membership(&room, member.as_ref()),
            trust,
            is_self,
            pronouns: self.cached_pronouns(user_id),
            link: user.matrix_to_uri().to_string(),
            direct_room,
            ignored,
            powers: moderation_powers(&levels, room.own_user_id(), &user),
        })
    }

    async fn pronouns(&self, user_id: &UserId) -> Vec<String> {
        let Ok(client) = self.matrix.client().await else {
            return Vec::new();
        };
        self.pronouns.resolve(&client, user_id).await
    }

    async fn global_profile(&self, user_id: &UserId) -> Result<GlobalProfile> {
        let client = self.matrix.client().await?;
        let user = parse_user(user_id)?;
        let profile = client
            .account()
            .fetch_user_profile_of(&user)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(GlobalProfile {
            display_name: profile.get_static::<DisplayName>().ok().flatten(),
            avatar_mxc: profile
                .get_static::<AvatarUrl>()
                .ok()
                .flatten()
                .map(|mxc| mxc.to_string()),
            pronouns: pronouns_of(&profile),
        })
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        let Ok(client) = self.matrix.client().await else {
            return 0;
        };
        fetch_avatar_thumbnails(&client, self.matrix.media(), mxcs).await
    }

    async fn start_dm(&self, user_id: &UserId) -> Result<RoomId> {
        let client = self.matrix.client().await?;
        let user = parse_user(user_id)?;
        if let Some(existing) = client.get_dm_room(&user) {
            return Ok(RoomId::new(existing.room_id().to_string()));
        }
        let response = client
            .send(direct_room_request(&user))
            .with_request_config(RequestConfig::short_retry().disable_retry())
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        let room_id = response.room_id;
        if let Err(e) = self.mark_direct(&client, &room_id, &user).await {
            tracing::warn!(%room_id, %user, "started a chat but could not mark it direct: {e}");
        }
        Ok(RoomId::new(room_id.to_string()))
    }

    async fn set_ignored(&self, user_id: &UserId, change: IgnoreChange) -> Result<()> {
        let client = self.matrix.client().await?;
        let user = parse_user(user_id)?;
        let own = own_user(&client)?;
        if user == own {
            return Err(AppError::Other("you cannot ignore yourself".to_owned()));
        }
        let mut list = client
            .account()
            .account_data::<IgnoredUserListEventContent>()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?
            .map(|raw| raw.deserialize())
            .transpose()
            .map_err(|e| AppError::Other(e.to_string()))?
            .unwrap_or_default();
        let changed = match change {
            IgnoreChange::Ignore => list
                .ignored_users
                .insert(user, IgnoredUser::new())
                .is_none(),
            IgnoreChange::Unignore => list.ignored_users.remove(&user).is_some(),
        };
        if !changed {
            return Ok(());
        }
        let request = set_global_account_data::v3::Request::new(own, &list)
            .map_err(|e| AppError::Other(e.to_string()))?;
        client
            .send(request)
            .with_request_config(RequestConfig::short_retry())
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }

    async fn moderate(&self, room_id: &RoomId, user_id: &UserId, action: Moderation) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        let user = parse_user(user_id)?;
        let levels = room.power_levels_or_default().await;
        let powers = moderation_powers(&levels, room.own_user_id(), &user);
        let allowed = match action {
            Moderation::Kick => powers.kick,
            Moderation::Ban => powers.ban,
            Moderation::Unban => powers.unban,
        };
        if !allowed {
            return Err(AppError::Other(
                "the room's power levels do not allow that".to_owned(),
            ));
        }
        send_moderation(&room, user, action).await
    }
}

async fn send_moderation(room: &Room, user: OwnedUserId, action: Moderation) -> Result<()> {
    let client = room.client();
    let room_id = room.room_id().to_owned();
    let config = RequestConfig::short_retry();
    let sent = match action {
        Moderation::Kick => client
            .send(kick_user::v3::Request::new(room_id, user))
            .with_request_config(config)
            .await
            .map(drop),
        Moderation::Ban => client
            .send(ban_user::v3::Request::new(room_id, user))
            .with_request_config(config)
            .await
            .map(drop),
        Moderation::Unban => client
            .send(unban_user::v3::Request::new(room_id, user))
            .with_request_config(config)
            .await
            .map(drop),
    };
    sent.map_err(|e| AppError::Other(e.to_string()))
}

fn own_user(client: &Client) -> Result<OwnedUserId> {
    client
        .user_id()
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Other("the session has no user id".to_owned()))
}

fn direct_room_request(user: &OwnedUserId) -> create_room::v3::Request {
    let mut request = create_room::v3::Request::new();
    request.invite = vec![user.clone()];
    request.is_direct = true;
    request.preset = Some(create_room::v3::RoomPreset::TrustedPrivateChat);
    request.initial_state = vec![
        InitialStateEvent::with_empty_state_key(
            RoomEncryptionEventContent::with_recommended_defaults(),
        )
        .to_raw_any(),
    ];
    request
}

async fn current_direct(client: &Client, own: &OwnedUserId) -> Result<DirectEventContent> {
    let request =
        get_global_account_data::v3::Request::new(own.clone(), GlobalAccountDataEventType::Direct);
    match client
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await
    {
        Ok(response) => response
            .account_data
            .deserialize_as_unchecked::<DirectEventContent>()
            .map_err(|e| AppError::Other(e.to_string())),
        Err(e) if e.client_api_error_kind() == Some(&ErrorKind::NotFound) => {
            Ok(DirectEventContent::default())
        }
        Err(e) => Err(AppError::Other(e.to_string())),
    }
}

fn parse_user(user_id: &UserId) -> Result<OwnedUserId> {
    OwnedUserId::try_from(user_id.as_ref())
        .map_err(|e| AppError::Other(format!("not a user id: {e}")))
}

fn membership(room: &Room, member: Option<&RoomMember>) -> RoomMembership {
    let Some(member) = member else {
        return if room.are_members_synced() {
            RoomMembership::Outside
        } else {
            RoomMembership::Unknown
        };
    };
    match member.membership() {
        MembershipState::Join => RoomMembership::Joined,
        MembershipState::Invite => RoomMembership::Invited,
        MembershipState::Knock => RoomMembership::Knocking,
        MembershipState::Leave => RoomMembership::Left,
        MembershipState::Ban => RoomMembership::Banned,
        _ => RoomMembership::Unknown,
    }
}
