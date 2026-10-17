use std::collections::HashSet;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use super::calls::{self, RECONNECT_AFTER, RECONNECT_FOR, SPEAKING_TURN, UNTRUSTED};
use super::data;
use crate::domain::call::identity::{MediaIdentity, spellings};
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{Decryption, MediaEvent, MediaSetup, Microphone, SfuState};
use crate::domain::call::sfu::{SfuGrant, SfuId, SfuRole};
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::call::{CallMediaPort, MediaSession};

pub struct DemoCallMedia;

#[async_trait]
impl CallMediaPort for DemoCallMedia {
    fn broken(&self) -> Option<String> {
        None
    }

    async fn open(
        &self,
        setup: MediaSetup,
        events_tx: mpsc::UnboundedSender<MediaEvent>,
    ) -> Result<Box<dyn MediaSession>> {
        tracing::info!(room_id = %setup.room_id, encrypted = setup.encrypted, "demo: the call media opens");
        Ok(Box::new(DemoMediaSession {
            room_id: setup.room_id,
            encrypted: setup.encrypted,
            events_tx,
            connected: HashSet::new(),
            keyed: HashSet::new(),
            tasks: Vec::new(),
        }))
    }
}

struct DemoMediaSession {
    room_id: RoomId,
    encrypted: bool,
    events_tx: mpsc::UnboundedSender<MediaEvent>,
    connected: HashSet<SfuId>,
    keyed: HashSet<MediaIdentity>,
    tasks: Vec<JoinHandle<()>>,
}

impl DemoMediaSession {
    fn emit(&self, event: MediaEvent) {
        if self.events_tx.send(event).is_err() {
            tracing::debug!("demo: the app stopped listening to the call media");
        }
    }

    fn own_identity() -> MediaIdentity {
        let user = data::own_user();
        let device = calls::own_device();
        let [legacy, _, _] = spellings(user, device, &format!("{user}:{device}"));
        legacy
    }

    fn spawn_reconnect(&mut self, sfu: SfuId) {
        let events_tx = self.events_tx.clone();
        self.tasks.push(tokio::spawn(async move {
            sleep(RECONNECT_AFTER).await;
            tracing::info!(%sfu, "demo: the media server reconnects");
            drop(events_tx.send(MediaEvent::Sfu {
                sfu: sfu.clone(),
                state: SfuState::Reconnecting,
            }));
            sleep(RECONNECT_FOR).await;
            drop(events_tx.send(MediaEvent::Sfu {
                sfu,
                state: SfuState::Connected,
            }));
        }));
    }

    fn spawn_speaking(&mut self, sfu: SfuId) {
        let events_tx = self.events_tx.clone();
        let room_id = self.room_id.clone();
        self.tasks.push(tokio::spawn(async move {
            let mut turn = 0_usize;
            loop {
                sleep(SPEAKING_TURN).await;
                let mut voices: Vec<MediaIdentity> = calls::book()
                    .others_in(room_id.as_ref())
                    .into_iter()
                    .map(|(_, identity)| identity)
                    .collect();
                voices.push(Self::own_identity());
                let speaker = voices.get(turn % voices.len().max(1)).cloned();
                turn = turn.wrapping_add(1);
                let speaking = MediaEvent::Speaking {
                    sfu: sfu.clone(),
                    speakers: speaker.into_iter().collect(),
                };
                if events_tx.send(speaking).is_err() {
                    return;
                }
            }
        }));
    }

    fn report_waiting(&self) {
        if !self.encrypted {
            return;
        }
        let untrusted = calls::scenario().undecryptable;
        for (member, identity) in calls::book().others_in(self.room_id.as_ref()) {
            if self.keyed.contains(&identity) {
                continue;
            }
            let state = if untrusted && member.user == UNTRUSTED {
                Decryption::Failed
            } else {
                Decryption::MissingKey
            };
            self.emit(MediaEvent::Decryption { identity, state });
        }
    }
}

#[async_trait]
impl MediaSession for DemoMediaSession {
    async fn connect(&mut self, grant: SfuGrant) -> Result<()> {
        calls::pause().await;
        let sfu = grant.sfu.clone();
        if calls::scenario().connect_fails {
            tracing::info!(%sfu, "demo: the media server refuses the connection");
            self.emit(MediaEvent::Sfu {
                sfu,
                state: SfuState::Disconnected("demo: the media server refused".into()),
            });
            return Err(AppError::Other("demo: the media server refused".into()));
        }
        tracing::info!(%sfu, role = ?grant.role, identity = %grant.identity, "demo: connected to a media server");
        self.emit(MediaEvent::Sfu {
            sfu: sfu.clone(),
            state: SfuState::Connected,
        });
        calls::book().watch(self.room_id.as_ref(), sfu.as_ref(), self.events_tx.clone());
        self.connected.insert(sfu.clone());
        self.report_waiting();
        if grant.role == SfuRole::Publish {
            if calls::scenario().reconnecting {
                self.spawn_reconnect(sfu.clone());
            }
            if calls::scenario().speaking {
                self.spawn_speaking(sfu);
            }
        }
        Ok(())
    }

    async fn disconnect(&mut self, sfu: &SfuId) {
        tracing::info!(%sfu, "demo: left a media server");
        self.connected.remove(sfu);
    }

    async fn set_microphone(&mut self, on: bool) {
        tracing::info!(on, "demo: the microphone switches");
        self.emit(MediaEvent::Microphone(if on {
            Microphone::On
        } else {
            Microphone::Off
        }));
    }

    async fn set_own_key(&mut self, index: KeyIndex, _key: &MediaKey) {
        tracing::info!(%index, "demo: your call key is installed");
    }

    async fn use_own_key(&mut self, index: KeyIndex) {
        tracing::info!(%index, "demo: your call media uses a new key");
    }

    async fn set_key(&mut self, identities: &[MediaIdentity], index: KeyIndex, _key: &MediaKey) {
        let others = calls::book().others_in(self.room_id.as_ref());
        let Some((member, identity)) = others
            .into_iter()
            .find(|(_, identity)| identities.contains(identity))
        else {
            tracing::info!(
                identities = identities.len(),
                "demo: a call key for nobody in the call"
            );
            return;
        };
        tracing::info!(%member, %index, "demo: a call key is installed");
        let state = if calls::scenario().undecryptable && member.user == UNTRUSTED {
            Decryption::Failed
        } else {
            Decryption::Ok
        };
        self.keyed.insert(identity.clone());
        self.emit(MediaEvent::Decryption { identity, state });
    }

    async fn close(self: Box<Self>) {
        tracing::info!(room_id = %self.room_id, "demo: the call media closes");
        for task in &self.tasks {
            task.abort();
        }
    }
}
