use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::connections::Meter;
use super::{Joined, LinkState, OwnCamera, OwnMicrophone, OwnScreen, SHARES_MISSED_BEFORE_HINT};
use crate::app::calls::people;
use crate::commands::view::{
    ActiveCall, CallNotice, CallPhase, CallTile, CameraView, CantDecrypt, MicrophoneView,
    MissingMedia, ScreenView, TileKind, TileState,
};
use crate::domain::call::identity::{MediaIdentity, member_spellings};
use crate::domain::call::media::{ConnectionQuality, Decryption, MediaSource};
use crate::domain::call::sfu::SfuId;
use crate::domain::call::video::VideoKind;
use crate::domain::call::{
    CallIntent, CallMember, DeviceStanding, FocusSelection, MemberKey, bridges_in_order,
};
use crate::domain::member_name::NamedUser;
use crate::domain::room::UnreadFlags;

impl Joined {
    fn phase(&self) -> CallPhase {
        match self.publish_link().map(|link| &link.state) {
            Some(LinkState::Reconnecting) => CallPhase::Reconnecting,
            Some(LinkState::Connected) if self.is_ringing_out() => CallPhase::Calling,
            Some(LinkState::Connected) if !self.has_company() => CallPhase::Waiting,
            Some(LinkState::Connected) => CallPhase::Live,
            _ if self.membership.is_some() => CallPhase::Connecting,
            _ => CallPhase::Joining,
        }
    }

    fn notice(&self, tiles: &[CallTile]) -> Option<CallNotice> {
        if let Some(kind) = self.unannounced() {
            return Some(CallNotice::NotAnnounced(kind));
        }
        if self.microphone == OwnMicrophone::Failed {
            return Some(CallNotice::MicrophoneFailed);
        }
        if self.camera == OwnCamera::Failed {
            return Some(CallNotice::CameraFailed);
        }
        self.screen_notice()
            .or_else(|| self.connection_notice())
            .or_else(|| undecryptable(tiles))
            .or_else(|| not_received(tiles))
    }

    fn connection_notice(&self) -> Option<CallNotice> {
        let connected = self
            .publish_link()
            .is_some_and(|link| link.state == LinkState::Connected);
        (connected && self.own_quality != ConnectionQuality::Good)
            .then_some(CallNotice::ConnectionPoor)
    }

    fn screen_notice(&self) -> Option<CallNotice> {
        let missed_often = self.shares_missed >= SHARES_MISSED_BEFORE_HINT;
        match self.screen {
            OwnScreen::Choosing(_) => Some(CallNotice::ChoosingScreen),
            OwnScreen::Off | OwnScreen::Failed if missed_often => {
                Some(CallNotice::SharingNeverStarted)
            }
            OwnScreen::Failed => Some(CallNotice::ScreenFailed),
            OwnScreen::Off | OwnScreen::On(_) | OwnScreen::Unavailable(_) => None,
        }
    }

    pub(in crate::app::calls) fn view(&self) -> ActiveCall {
        let tiles = self.tiles();
        let notice = self.notice(&tiles);
        let sharing = matches!(self.screen, OwnScreen::On(_));
        let people = people::of_tiles(
            &tiles,
            &self.members,
            sharing,
            self.stage.self_focusable(&tiles),
        );
        let arranged = self.stage.arrange(tiles);
        ActiveCall {
            call: self.call,
            room_id: self.room_id.clone(),
            room_name: self.room_name.clone(),
            room_unread: UnreadFlags::default(),
            phase: self.phase(),
            live_since_ms: self.live_since_ms,
            encrypted: self.encrypted,
            video: self.intent == CallIntent::Video,
            microphone: match self.microphone {
                OwnMicrophone::Off => MicrophoneView::Off,
                OwnMicrophone::Starting => MicrophoneView::Starting,
                OwnMicrophone::On => MicrophoneView::On,
                OwnMicrophone::Failed => MicrophoneView::Failed,
            },
            microphone_level: match self.microphone_meter {
                Meter::Level(level) if self.microphone == OwnMicrophone::On => Some(level),
                Meter::Off | Meter::Waiting | Meter::Level(_) => None,
            },
            camera: match self.camera {
                OwnCamera::Off => CameraView::Off,
                OwnCamera::Starting => CameraView::Starting,
                OwnCamera::On(_) => CameraView::On,
                OwnCamera::Failed => CameraView::Failed,
            },
            screen: match self.screen {
                OwnScreen::Off => ScreenView::Off,
                OwnScreen::Choosing(_) => ScreenView::Choosing,
                OwnScreen::On(_) => ScreenView::On,
                OwnScreen::Failed => ScreenView::Failed,
                OwnScreen::Unavailable(refusal) => ScreenView::Unavailable(refusal),
            },
            hand: self.hand_view(),
            hands_raised: self.hands_raised(),
            hand_news: self.hand_news(),
            reaction: self.reaction_view(),
            reaction_news: self.reaction_news(),
            share_preview: self.stage.share_preview(),
            share_change_cancelled: self.changes_cancelled,
            notice,
            stage: arranged.stage,
            stage_chosen: arranged.chosen,
            view_chosen: arranged.view_chosen,
            tiles: Arc::from(arranged.tiles),
            compact_self: arranged.compact_self,
            self_view: self.stage.self_view(),
            follows_speaker: self.stage.follows_speaker(),
            faces_ready: 0,
            shares: Arc::from(arranged.shares),
            offered_share: arranged.offered_share,
            started_share: self.stage.started_share().cloned(),
            ended_share: self.stage.ended_share().cloned(),
            bridges: Arc::from(if self.encrypted {
                bridges_in_order(&self.members)
            } else {
                Vec::new()
            }),
            people: Arc::from(people),
            devices: self.devices.view(),
            connection: self
                .readings
                .as_ref()
                .map(|readings| Arc::from(self.connection_details(readings))),
        }
    }

    pub(super) fn tiles(&self) -> Vec<CallTile> {
        let mut tiles: Vec<CallTile> = self
            .members
            .iter()
            .filter(|member| MemberKey::of(member) != self.me)
            .filter_map(|member| self.screen_tile(member))
            .collect();
        if !self
            .members
            .iter()
            .any(|member| MemberKey::of(member) == self.me)
        {
            let me = NamedUser::unnamed(self.me.user.clone());
            tiles.extend(self.own_screen_tile(&me));
            tiles.push(self.own_tile(me, None));
        }
        for member in &self.members {
            if MemberKey::of(member) == self.me {
                tiles.extend(self.own_screen_tile(&member.user));
                tiles.push(self.own_tile(member.user.clone(), member.avatar_mxc.clone()));
            } else {
                tiles.push(self.remote_tile(member));
            }
        }
        number_devices(&mut tiles);
        for tile in tiles.iter_mut().filter(|tile| !tile.own) {
            tile.muted_for_you = self.unheard.contains(&tile.member.user);
        }
        let order = self.hand_order();
        for tile in tiles
            .iter_mut()
            .filter(|tile| tile.kind == TileKind::Person)
        {
            tile.hand = order
                .iter()
                .position(|member| *member == tile.member)
                .map(|index| index + 1);
            tile.reaction = self.reaction_on(&tile.member);
        }
        tiles
    }

    fn own_tile(&self, name: NamedUser, avatar_mxc: Option<String>) -> CallTile {
        let speaking = self
            .own_spellings()
            .iter()
            .any(|identity| self.heard.is_speaking(identity));
        let state = match self.publish_link().map(|link| &link.state) {
            Some(LinkState::Connected) if self.own_quality != ConnectionQuality::Good => {
                TileState::PoorConnection
            }
            Some(LinkState::Connected | LinkState::Reconnecting) => TileState::Live,
            Some(LinkState::Failed(_)) => TileState::Unreachable,
            _ => TileState::Joining,
        };
        let video = match &self.camera {
            OwnCamera::On(feed) => Some(feed.clone()),
            OwnCamera::Off | OwnCamera::Starting | OwnCamera::Failed => None,
        };
        CallTile {
            member: self.me.clone(),
            name,
            device: 0,
            avatar_mxc,
            own: true,
            kind: TileKind::Person,
            state,
            muted: self.microphone != OwnMicrophone::On,
            muted_for_you: false,
            hand: None,
            reaction: None,
            speaking,
            video,
        }
    }

    fn own_screen_tile(&self, name: &NamedUser) -> Option<CallTile> {
        let OwnScreen::On(feed) = &self.screen else {
            return None;
        };
        Some(CallTile {
            member: self.me.clone(),
            name: name.clone(),
            device: 0,
            avatar_mxc: None,
            own: true,
            kind: TileKind::Screen,
            state: TileState::Live,
            muted: false,
            muted_for_you: false,
            hand: None,
            reaction: None,
            speaking: false,
            video: Some(feed.clone()),
        })
    }

    fn remote_tile(&self, member: &CallMember) -> CallTile {
        let identities = member_spellings(member);
        let key = MemberKey::of(member);
        let publishes_to = self.publishes_to(member);
        let muted = publishes_to.as_ref().is_some_and(|sfu| {
            identities
                .iter()
                .any(|identity| self.heard.is_muted_on(sfu, identity))
        });
        let video =
            publishes_to.and_then(|sfu| self.heard.video_on(&sfu, &identities, VideoKind::Camera));
        CallTile {
            state: self.remote_state(member, &key, &identities),
            muted,
            muted_for_you: false,
            hand: None,
            reaction: None,
            speaking: identities
                .iter()
                .any(|identity| self.heard.is_speaking(identity)),
            member: key,
            name: member.user.clone(),
            device: 0,
            avatar_mxc: member.avatar_mxc.clone(),
            own: false,
            kind: TileKind::Person,
            video,
        }
    }

    fn screen_tile(&self, member: &CallMember) -> Option<CallTile> {
        let identities = member_spellings(member);
        let sfu = self.publishes_to(member)?;
        let feed = self.heard.video_on(&sfu, &identities, VideoKind::Screen)?;
        let key = MemberKey::of(member);
        let state = match self.remote_state(member, &key, &identities) {
            TileState::NotReceived(_) | TileState::PoorConnection | TileState::VideoStalled => {
                TileState::Live
            }
            state => state,
        };
        Some(CallTile {
            state,
            muted: false,
            muted_for_you: false,
            hand: None,
            reaction: None,
            speaking: false,
            member: key,
            name: member.user.clone(),
            device: 0,
            avatar_mxc: None,
            own: false,
            kind: TileKind::Screen,
            video: Some(feed),
        })
    }

    fn remote_state(
        &self,
        member: &CallMember,
        key: &MemberKey,
        identities: &[MediaIdentity; 3],
    ) -> TileState {
        let publishes_to = self.publishes_to(member);
        if publishes_to.as_ref().is_some_and(|sfu| {
            self.links
                .get(sfu)
                .is_some_and(|link| matches!(link.state, LinkState::Failed(_)))
        }) {
            return TileState::Unreachable;
        }
        if !identities
            .iter()
            .any(|identity| self.heard.is_present(identity))
        {
            return TileState::Joining;
        }
        let keys = self.key_state(key, identities);
        if matches!(keys, TileState::CantDecrypt(_)) {
            return keys;
        }
        publishes_to
            .and_then(|sfu| self.trouble_on(&sfu, identities))
            .unwrap_or(keys)
    }

    fn key_state(&self, key: &MemberKey, identities: &[MediaIdentity; 3]) -> TileState {
        if !self.encrypted {
            return TileState::Live;
        }
        let decryption = identities
            .iter()
            .find_map(|identity| self.heard.decryption.get(identity));
        match decryption {
            Some(Decryption::Ok) => TileState::Live,
            Some(Decryption::Failed) => TileState::CantDecrypt(self.why_undecryptable(key)),
            None if self.keys.has_key_from(key) => TileState::Live,
            Some(Decryption::MissingKey) | None => TileState::WaitingForKeys,
        }
    }

    fn trouble_on(&self, sfu: &SfuId, identities: &[MediaIdentity]) -> Option<TileState> {
        let quality = self.heard.quality_on(sfu, identities);
        if quality == ConnectionQuality::Lost {
            return Some(TileState::ConnectionLost);
        }
        if let Some(what) = missing(&self.heard.missing_on(sfu, identities)) {
            return Some(TileState::NotReceived(what));
        }
        if self.heard.is_stalled_on(sfu, identities) {
            return Some(TileState::VideoStalled);
        }
        (quality == ConnectionQuality::Poor).then_some(TileState::PoorConnection)
    }

    fn why_undecryptable(&self, key: &MemberKey) -> CantDecrypt {
        if self.standings.get(key) == Some(&DeviceStanding::NotCrossSigned) {
            CantDecrypt::NotCrossSigned
        } else {
            CantDecrypt::NoKey
        }
    }

    fn publishes_to(&self, member: &CallMember) -> Option<SfuId> {
        let transport = match member.selection {
            FocusSelection::MultiSfu => member.transport.as_ref(),
            FocusSelection::OldestMembership => self
                .members
                .first()
                .and_then(|oldest| oldest.transport.as_ref()),
        }?;
        SfuId::of(transport)
    }
}

pub(super) fn missing(sources: &HashSet<MediaSource>) -> Option<MissingMedia> {
    if sources.is_empty() {
        return None;
    }
    let only = |allowed: &[MediaSource]| sources.iter().all(|source| allowed.contains(source));
    let media = if only(&[MediaSource::Microphone]) {
        MissingMedia::Audio
    } else if only(&[MediaSource::Camera]) {
        MissingMedia::Video
    } else if only(&[MediaSource::Microphone, MediaSource::Camera]) {
        MissingMedia::AudioAndVideo
    } else if sources.contains(&MediaSource::Screen)
        && only(&[MediaSource::Screen, MediaSource::ScreenAudio])
    {
        MissingMedia::Screen
    } else {
        MissingMedia::Other
    };
    Some(media)
}

fn not_received(tiles: &[CallTile]) -> Option<CallNotice> {
    let mut troubled = tiles.iter().filter_map(|tile| match tile.state {
        TileState::NotReceived(what) if tile.kind == TileKind::Person => Some((tile, what)),
        _ => None,
    });
    let (first, what) = troubled.next()?;
    let others: HashSet<&str> = troubled
        .map(|(tile, _)| tile.member.user.as_str())
        .filter(|user| *user != first.member.user)
        .collect();
    Some(CallNotice::NotReceived {
        who: first.name.clone(),
        what,
        others: others.len(),
    })
}

fn undecryptable(tiles: &[CallTile]) -> Option<CallNotice> {
    let mut troubled = tiles.iter().filter_map(|tile| match tile.state {
        TileState::CantDecrypt(why) if tile.kind == TileKind::Person => Some((tile, why)),
        _ => None,
    });
    let (first, why) = troubled.next()?;
    Some(CallNotice::CantDecrypt {
        who: first.name.clone(),
        why,
        others: troubled.count(),
    })
}

fn remote_person(tile: &CallTile) -> bool {
    !tile.own && tile.kind == TileKind::Person
}

fn number_devices(tiles: &mut [CallTile]) {
    let mut devices: HashMap<String, u8> = HashMap::new();
    for tile in tiles.iter().filter(|tile| remote_person(tile)) {
        let count = devices.entry(tile.member.user.clone()).or_default();
        *count = count.saturating_add(1);
    }
    let mut numbered: HashMap<String, u8> = HashMap::new();
    for tile in tiles.iter_mut().filter(|tile| remote_person(tile)) {
        if devices
            .get(&tile.member.user)
            .is_some_and(|count| *count > 1)
        {
            let number = numbered.entry(tile.member.user.clone()).or_default();
            *number = number.saturating_add(1);
            tile.device = *number;
        }
    }
}
