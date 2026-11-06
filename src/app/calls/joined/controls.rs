use std::time::Duration;

use super::{Joined, OwnCamera, OwnScreen, Pick};
use crate::app::calls::media::MediaCommand;
use crate::commands::view::{TileKey, TileKind};
use crate::domain::call::devices::{DeviceChoice, DeviceKey, DeviceKind, DeviceLists};
use crate::domain::call::identity::{MediaIdentity, member_spellings};
use crate::domain::call::sfu::SfuId;
use crate::domain::call::video::FeedId;
use crate::domain::call::{CallIntent, MemberKey};

impl Joined {
    pub(in crate::app::calls) fn set_microphone(&mut self, on: bool) {
        let Some(switched) = self.microphone.switched(on, self.microphone_published) else {
            return;
        };
        tracing::info!(call = self.call, on, "switching the microphone");
        self.microphone = switched;
        self.command(MediaCommand::Microphone(on));
    }

    pub(in crate::app::calls) fn set_camera(&mut self, on: bool) {
        let wanted = if on {
            OwnCamera::Starting
        } else {
            OwnCamera::Off
        };
        if matches!(
            (&self.camera, &wanted),
            (OwnCamera::Off, OwnCamera::Off)
                | (OwnCamera::Starting | OwnCamera::On(_), OwnCamera::Starting)
        ) {
            return;
        }
        tracing::info!(call = self.call, on, "switching the camera");
        if on {
            self.devices.retry(DeviceKind::Camera);
        }
        self.camera = wanted;
        self.command(MediaCommand::Camera(on));
        self.tell_intent();
    }

    pub(super) fn tell_intent(&self) {
        let intent = CallIntent::of_camera(self.media_sent().camera);
        self.intent_tx.send_if_modified(|told| {
            if *told == intent {
                return false;
            }
            tracing::info!(
                call = self.call,
                ?intent,
                "the call membership's intent follows the camera"
            );
            *told = intent;
            true
        });
    }

    pub(in crate::app::calls) fn use_device(
        &mut self,
        kind: DeviceKind,
        key: Option<&DeviceKey>,
    ) -> Option<DeviceChoice> {
        let Some(choice) = self.devices.choice_for(kind, key) else {
            tracing::debug!(call = self.call, %kind, "no such device to use in the call");
            return None;
        };
        tracing::info!(call = self.call, %kind, device = ?choice.key(), "choosing a call device");
        self.devices.choose(kind, choice.clone());
        self.command(MediaCommand::Device(kind, choice.clone()));
        Some(choice)
    }

    pub(in crate::app::calls) fn refresh_devices(&self) {
        self.command(MediaCommand::RefreshDevices);
    }

    pub(in crate::app::calls) fn device_lists(&self) -> DeviceLists {
        self.devices.lists().clone()
    }

    pub(in crate::app::calls) fn camera_showing(&self, camera: &DeviceChoice) -> Option<FeedId> {
        let OwnCamera::On(feed) = &self.camera else {
            return None;
        };
        self.devices
            .uses_alike(DeviceKind::Camera, camera)
            .then(|| feed.clone())
    }

    pub(in crate::app::calls) fn set_screen_share(&mut self, on: bool) {
        let unchanged = match self.screen {
            OwnScreen::Unavailable(_) => true,
            OwnScreen::Off | OwnScreen::Failed => !on,
            OwnScreen::Choosing(_) | OwnScreen::On(_) => on,
        };
        if unchanged {
            return;
        }
        tracing::info!(call = self.call, on, "switching the screen share");
        self.screen = if on {
            OwnScreen::Choosing(Pick::First)
        } else {
            OwnScreen::Off
        };
        self.command(MediaCommand::ScreenShare(on));
    }

    pub(in crate::app::calls) fn change_screen_share(&mut self) {
        if !matches!(self.screen, OwnScreen::On(_)) {
            tracing::debug!(call = self.call, "no screen share to change");
            return;
        }
        tracing::info!(
            call = self.call,
            "changing the shared screen: stopping the share, then asking again"
        );
        self.screen = OwnScreen::Choosing(Pick::Replacement);
        self.command(MediaCommand::ScreenShare(false));
        self.command(MediaCommand::ScreenShare(true));
    }

    pub(in crate::app::calls) fn settle_stage(&mut self) -> Option<Duration> {
        let tiles = self.tiles();
        let loudest = self.loudest_speaker();
        let now = self.now();
        self.stage.settle(&tiles, loudest.as_ref(), now)
    }

    fn loudest_speaker(&self) -> Option<TileKey> {
        let mut servers: Vec<&SfuId> = self.heard.speaking.keys().collect();
        servers.sort_unstable();
        servers
            .into_iter()
            .filter_map(|sfu| self.heard.speaking.get(sfu))
            .flatten()
            .find_map(|identity| self.person_speaking(identity))
    }

    fn person_speaking(&self, identity: &MediaIdentity) -> Option<TileKey> {
        self.members
            .iter()
            .filter(|member| MemberKey::of(member) != self.me)
            .find(|member| member_spellings(member).contains(identity))
            .map(|member| TileKey {
                member: MemberKey::of(member),
                kind: TileKind::Person,
            })
    }

    pub(in crate::app::calls) fn focus_tile(&mut self, key: &str) {
        let tiles = self.tiles();
        if !self.stage.focus(key, &tiles) {
            tracing::debug!(tile = key, "no call tile to focus");
        }
    }

    pub(in crate::app::calls) fn unpin_tile(&mut self) {
        tracing::info!(
            call = self.call,
            "the call's stage goes back to its own rule"
        );
        self.stage.unpin();
    }

    pub(in crate::app::calls) fn show_gallery(&mut self) {
        tracing::info!(call = self.call, "showing the call as a gallery");
        self.stage.gallery();
    }

    pub(in crate::app::calls) fn view_share(&mut self) {
        let tiles = self.tiles();
        self.stage.view_share(&tiles);
    }

    pub(in crate::app::calls) fn show_self_view(&mut self, on: bool) {
        tracing::info!(call = self.call, on, "switching the self-view");
        self.stage.show_self(on);
    }

    pub(in crate::app::calls) fn mute_for_me(&mut self, user_id: &str, on: bool) {
        if user_id == self.me.user {
            tracing::debug!(call = self.call, "you cannot mute yourself for you");
            return;
        }
        let changed = if on {
            self.unheard.insert(user_id.to_owned())
        } else {
            self.unheard.remove(user_id)
        };
        if changed {
            tracing::info!(
                call = self.call,
                user_id,
                on,
                "muting a call participant for you"
            );
            self.tell_unheard();
        }
    }

    pub(super) fn tell_unheard(&mut self) {
        let mut identities: Vec<MediaIdentity> = self
            .members
            .iter()
            .filter(|member| self.unheard.contains(&member.user.user_id))
            .flat_map(|member| self.identities.identities_of(&MemberKey::of(member)))
            .collect();
        identities.sort();
        identities.dedup();
        if identities == self.unheard_sent {
            return;
        }
        tracing::debug!(
            call = self.call,
            people = self.unheard.len(),
            identities = identities.len(),
            "telling the media whom you do not hear"
        );
        self.unheard_sent.clone_from(&identities);
        self.command(MediaCommand::Unheard(identities));
    }

    pub(in crate::app::calls) fn follow_speaker(&mut self, on: bool) {
        tracing::info!(call = self.call, on, "switching speaker following");
        self.stage.follow_speaker(on);
    }

    pub(in crate::app::calls) fn show_share_preview(&mut self, on: bool) {
        tracing::info!(
            call = self.call,
            on,
            "switching the preview of your shared screen"
        );
        self.stage.show_share_preview(on);
    }
}
