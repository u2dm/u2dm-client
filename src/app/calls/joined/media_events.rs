use super::connections::Meter;
use super::{Effect, Joined, LinkState, OwnCamera, OwnScreen, Pick};
use crate::app::calls::media::MediaCommand;
use crate::commands::view::CallFailure;
use crate::domain::call::device_test::{PERCENT, level_percent};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::ReceivedKey;
use crate::domain::call::media::{
    ConnectionQuality, MediaEvent, MediaSource, Microphone, SfuState,
};
use crate::domain::call::sfu::{SfuId, SfuRole};
use crate::domain::call::video::{Camera, ScreenShare};
use crate::util::unix_now_ms;

impl Joined {
    pub(in crate::app::calls) fn media_event(
        &mut self,
        event: MediaEvent,
    ) -> Result<(), CallFailure> {
        match event {
            MediaEvent::Sfu { sfu, state } => return self.sfu_state(&sfu, state),
            MediaEvent::Present {
                sfu,
                identity,
                present,
            } => {
                let identities = self.heard.present.entry(sfu).or_default();
                if present {
                    identities.insert(identity);
                } else {
                    identities.remove(&identity);
                }
            }
            MediaEvent::Muted {
                sfu,
                identity,
                muted,
            } => {
                let identities = self.heard.muted.entry(sfu).or_default();
                if muted {
                    identities.insert(identity);
                } else {
                    identities.remove(&identity);
                }
            }
            MediaEvent::Speaking { sfu, speakers } => {
                self.heard.speaking.insert(sfu, speakers);
            }
            MediaEvent::Missing {
                sfu,
                identity,
                source,
                missing,
            } => self.missing_heard(sfu, identity, source, missing),
            MediaEvent::Quality {
                sfu,
                identity,
                quality,
            } => self.quality_heard(sfu, identity, quality),
            MediaEvent::OwnQuality { sfu, quality } => self.own_quality_heard(&sfu, quality),
            MediaEvent::Stalled {
                sfu,
                identity,
                stalled,
            } => {
                if self.links.contains_key(&sfu) {
                    self.heard.stalled_heard(sfu, identity, stalled);
                }
            }
            MediaEvent::Decryption { identity, state } => {
                self.heard.decryption.insert(identity, state);
            }
            MediaEvent::NotEncrypted { sfu, trouble } => {
                tracing::warn!(call = self.call, %sfu, ?trouble, "your call media could not be encrypted, so the call stops");
                return Err(CallFailure::NotEncrypted);
            }
            MediaEvent::Microphone(microphone) => self.microphone_reported(&microphone),
            MediaEvent::Connections(readings) => self.connections_read(readings),
            MediaEvent::MicrophoneLevel(percent) => {
                if self.microphone_meter != Meter::Off {
                    self.microphone_meter =
                        Meter::Level(level_percent(f32::from(percent) / PERCENT));
                }
            }
            MediaEvent::Camera(camera) => self.camera_reported(camera),
            MediaEvent::Screen(screen) => self.screen_reported(screen),
            MediaEvent::Video {
                sfu,
                identity,
                kind,
                feed,
            } => self.heard.video_heard(sfu, identity, kind, feed),
            MediaEvent::Devices(lists) => self.devices.listed(lists),
            MediaEvent::DeviceFailed { kind, device, why } => {
                tracing::warn!(call = self.call, %kind, %device, why, "a call device could not be used");
                self.devices.failed(kind, device);
            }
        }
        Ok(())
    }

    fn missing_heard(
        &mut self,
        sfu: SfuId,
        identity: MediaIdentity,
        source: MediaSource,
        missing: bool,
    ) {
        if self.links.contains_key(&sfu) {
            self.heard.source_heard(sfu, identity, source, missing);
        }
    }

    fn quality_heard(&mut self, sfu: SfuId, identity: MediaIdentity, quality: ConnectionQuality) {
        if self.links.contains_key(&sfu) {
            self.heard.rated(sfu, identity, quality);
        }
    }

    fn own_quality_heard(&mut self, sfu: &SfuId, quality: ConnectionQuality) {
        if self
            .links
            .get(sfu)
            .is_some_and(|link| link.role == SfuRole::Publish)
        {
            self.own_quality = quality;
        }
    }

    fn microphone_reported(&mut self, report: &Microphone) {
        if *report == Microphone::On {
            self.microphone_published = true;
        }
        if let Microphone::Failed(why) = report {
            if self.microphone.sends() {
                tracing::warn!(call = self.call, why, "the microphone failed");
            } else {
                tracing::debug!(
                    call = self.call,
                    why,
                    "the microphone you switched off failed"
                );
            }
        }
        self.microphone = self.microphone.reported(report);
    }

    fn camera_reported(&mut self, camera: Camera) {
        match camera {
            Camera::On(feed) if self.camera == OwnCamera::Starting => {
                self.camera = OwnCamera::On(feed);
            }
            Camera::On(_) => {}
            Camera::Failed(why) if self.camera == OwnCamera::Off => {
                tracing::debug!(call = self.call, why, "the camera you switched off failed");
            }
            Camera::Failed(why) => {
                tracing::warn!(call = self.call, why, "the camera did not start");
                self.camera = OwnCamera::Failed;
                self.command(MediaCommand::Camera(false));
                self.tell_intent();
            }
        }
    }

    fn screen_reported(&mut self, screen: ScreenShare) {
        self.screen = match screen {
            ScreenShare::On(_) if !matches!(self.screen, OwnScreen::Choosing(_)) => return,
            ScreenShare::On(feed) => {
                self.shares_missed = 0;
                OwnScreen::On(feed)
            }
            ScreenShare::Off if matches!(self.screen, OwnScreen::Unavailable(_)) => return,
            ScreenShare::Off => self.screen_ended(),
            ScreenShare::Failed(why) => {
                tracing::warn!(call = self.call, why, "the screen share did not start");
                self.shares_missed = self.shares_missed.saturating_add(1);
                OwnScreen::Failed
            }
            ScreenShare::Unavailable(refusal) => {
                tracing::info!(
                    call = self.call,
                    ?refusal,
                    "this desktop cannot share its screen"
                );
                OwnScreen::Unavailable(refusal)
            }
        };
    }

    fn screen_ended(&mut self) -> OwnScreen {
        match self.screen {
            OwnScreen::Choosing(Pick::Replacement) => {
                tracing::info!(
                    call = self.call,
                    "no other screen was chosen, so your screen share stopped"
                );
                self.changes_cancelled = self.changes_cancelled.saturating_add(1);
            }
            OwnScreen::Choosing(Pick::First) => {
                self.shares_missed = self.shares_missed.saturating_add(1);
            }
            OwnScreen::Off | OwnScreen::On(_) | OwnScreen::Failed | OwnScreen::Unavailable(_) => {}
        }
        OwnScreen::Off
    }

    fn sfu_state(&mut self, sfu: &SfuId, state: SfuState) -> Result<(), CallFailure> {
        let Some(link) = self.links.get_mut(sfu) else {
            return Ok(());
        };
        let publish = link.role == SfuRole::Publish;
        let was_up = matches!(link.state, LinkState::Connected | LinkState::Reconnecting);
        link.state = match state {
            SfuState::Connecting => LinkState::Connecting,
            SfuState::Connected => LinkState::Connected,
            SfuState::Reconnecting => LinkState::Reconnecting,
            SfuState::Disconnected(why) => {
                tracing::info!(call = self.call, %sfu, why, was_up, "a call media server connection ended");
                match (publish, was_up) {
                    (true, true) => return Err(CallFailure::CutOff),
                    (true, false) => return Err(CallFailure::ServerUnreachable),
                    (false, _) => {
                        self.heard.forget(sfu);
                        LinkState::Failed(why)
                    }
                }
            }
        };
        if publish && link.state == LinkState::Connected && self.live_since_ms.is_none() {
            self.live_since_ms = Some(unix_now_ms());
        }
        Ok(())
    }

    pub(in crate::app::calls) fn received_key(&mut self, key: ReceivedKey) -> Vec<Effect> {
        if !self.encrypted {
            return Vec::new();
        }
        let actions = self.keys.received(key, &self.holders());
        self.apply_keys(actions)
    }

    pub(in crate::app::calls) fn key_wake(&mut self) -> Vec<Effect> {
        let actions = self.keys.tick(self.now());
        self.apply_keys(actions)
    }
}
