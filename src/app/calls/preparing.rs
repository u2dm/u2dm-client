use std::sync::Arc;

use super::device_test::{TestOutcome, TestState, TestTask, Testing};
use super::devices::Devices;
use super::offer::{Media, OfferFacts, offer};
use super::preview::{Preview, PreviewOutcome, PreviewState, PreviewTask};
use super::sheet::{Sheet, SheetFacts};
use super::{Calls, send};
use crate::app::event::CallEvent;
use crate::app::room_directory::RoomDirectory;
use crate::commands::view::{
    CallSheetView, DeviceTestView, JoinOffer, LeavingCall, OfferView, SheetPreview, SheetTests,
    TestPhase,
};
use crate::domain::call::device_test::{DeviceTest, level_percent};
use crate::domain::call::devices::{DeviceChoice, DeviceKey, DeviceKind, DeviceLists};
use crate::domain::room::{Room, RoomId};

impl Calls {
    pub(super) fn follow_sheet(&mut self, directory: &RoomDirectory) {
        let Some(sheet) = &self.sheet else {
            return;
        };
        if directory.room(&sheet.room_id).is_none() {
            tracing::debug!(room_id = %sheet.room_id, "the room the call sheet asks about left the list");
            self.sheet = None;
        }
    }

    pub(super) fn open_sheet(&mut self, room_id: &RoomId, video: bool, directory: &RoomDirectory) {
        let Some(room) = directory.room(room_id) else {
            tracing::debug!(%room_id, "no listed room to offer a call in");
            return;
        };
        if self.offer_for(room) == JoinOffer::InCall {
            tracing::debug!(%room_id, "you are already in this call");
            return;
        }
        tracing::debug!(%room_id, video, "asking how to join the call");
        self.stop_previous_sheets_test();
        self.sheet_serial = self.sheet_serial.wrapping_add(1);
        let draft = self.joined.is_some().then(|| self.devices.clone());
        self.sheet = Some(Sheet::open(room, video, self.sheet_serial, draft));
        if self.joined.is_none() {
            self.list_devices();
        }
        self.publish(directory);
    }

    fn stop_previous_sheets_test(&mut self) {
        if let Some(testing) = self.testing.take() {
            tracing::debug!("a new call sheet stops the device test");
            testing.stop();
        }
    }

    pub(super) fn sheet_camera(&mut self, on: bool, directory: &RoomDirectory) {
        let Some(sheet) = self.sheet.as_mut().filter(|sheet| sheet.camera != on) else {
            return;
        };
        tracing::debug!(on, "the call sheet's camera is switched");
        sheet.camera = on;
        self.publish(directory);
    }

    pub(in crate::app) fn preview_shown(
        &mut self,
        preview: u64,
        outcome: PreviewOutcome,
        directory: &RoomDirectory,
    ) {
        let Some(running) = self
            .preview
            .as_mut()
            .filter(|running| running.serial == preview)
        else {
            tracing::debug!(
                preview,
                "news of a camera preview that was stopped, dropping"
            );
            return;
        };
        running.state = match outcome {
            PreviewOutcome::Shown(feed) => PreviewState::Showing(feed),
            PreviewOutcome::Failed(trouble) => PreviewState::Failed(trouble),
        };
        self.publish(directory);
    }

    pub(super) fn follow_preview(&mut self, directory: &RoomDirectory) {
        let wanted = self.wanted_preview(directory);
        if self.preview.as_ref().map(|preview| &preview.camera) == wanted.as_ref() {
            return;
        }
        if let Some(preview) = self.preview.take() {
            preview.stop();
        }
        let (Some(camera), Some(port)) = (wanted, self.media.clone()) else {
            return;
        };
        self.previews = self.previews.wrapping_add(1);
        let task = PreviewTask {
            port,
            camera,
            turn: Arc::clone(&self.media_turn),
            events: self.events.clone(),
            serial: self.previews,
        };
        let (preview, run) = Preview::start(task, self.preview_tasks.token());
        self.preview_tasks.spawn(run);
        self.preview = Some(preview);
    }

    fn wanted_preview(&self, directory: &RoomDirectory) -> Option<DeviceChoice> {
        if self.joined.is_some() {
            return None;
        }
        let sheet = self.sheet.as_ref().filter(|sheet| sheet.camera)?;
        let room = directory.room(&sheet.room_id)?;
        let refused = matches!(self.offer_for(room), JoinOffer::Refused(_));
        (!refused).then(|| self.devices.camera.clone())
    }

    fn sheet_preview(&self, sheet: &Sheet, offer: JoinOffer) -> Option<SheetPreview> {
        if !sheet.camera || matches!(offer, JoinOffer::Refused(_)) {
            return None;
        }
        if let Some(joined) = &self.joined {
            let camera = &sheet.draft.as_ref()?.camera;
            let room_name = joined.room_name().to_owned();
            return Some(match joined.camera_showing(camera) {
                Some(feed) => SheetPreview::InCall { feed, room_name },
                None => SheetPreview::Elsewhere { room_name },
            });
        }
        Some(match &self.preview.as_ref()?.state {
            PreviewState::Starting => SheetPreview::Starting,
            PreviewState::Showing(feed) => SheetPreview::Local(feed.clone()),
            PreviewState::Failed(trouble) => SheetPreview::Failed(*trouble),
        })
    }

    pub(super) fn test_device(&mut self, kind: DeviceKind, on: bool, directory: &RoomDirectory) {
        if let Some(testing) = self.testing.take() {
            testing.stop();
        }
        let test = match kind {
            DeviceKind::Microphone => DeviceTest::Microphone(self.devices.microphone.clone()),
            DeviceKind::Speaker => DeviceTest::Speaker(self.devices.speaker.clone()),
            DeviceKind::Camera => {
                tracing::debug!("a camera is previewed, not tested");
                return;
            }
        };
        if on && self.offers_tests(directory) {
            self.start_test(test);
        } else if on {
            tracing::debug!(%kind, "no device test is offered here");
        }
        self.publish(directory);
    }

    fn start_test(&mut self, test: DeviceTest) {
        let Some(port) = self.media.clone() else {
            return;
        };
        self.tests = self.tests.wrapping_add(1);
        let task = TestTask {
            port,
            test,
            turn: Arc::clone(&self.media_turn),
            events: self.events.clone(),
            serial: self.tests,
        };
        let (testing, run) = Testing::start(task, self.test_tasks.token());
        self.test_tasks.spawn(run);
        self.testing = Some(testing);
    }

    pub(in crate::app) fn test_heard(
        &mut self,
        test: u64,
        outcome: TestOutcome,
        directory: &RoomDirectory,
    ) {
        let Some(running) = self
            .testing
            .as_mut()
            .filter(|running| running.serial == test)
        else {
            tracing::debug!(test, "news of a device test that was stopped, dropping");
            return;
        };
        match outcome {
            TestOutcome::Started => running.state = TestState::Running(0),
            TestOutcome::Level(loudness) => {
                running.state = TestState::Running(level_percent(loudness));
            }
            TestOutcome::Failed => running.state = TestState::Failed,
            TestOutcome::Lost => running.state = TestState::Lost,
            TestOutcome::Ended => self.testing = None,
        }
        self.publish(directory);
    }

    fn offers_tests(&self, directory: &RoomDirectory) -> bool {
        if self.joined.is_some() || self.media.is_none() {
            return false;
        }
        self.sheet
            .as_ref()
            .and_then(|sheet| directory.room(&sheet.room_id))
            .is_some_and(|room| !matches!(self.offer_for(room), JoinOffer::Refused(_)))
    }

    pub(super) fn follow_test(&mut self, directory: &RoomDirectory) {
        let Some(running) = &self.testing else {
            return;
        };
        let still_chosen = match &running.test {
            DeviceTest::Microphone(choice) => *choice == self.devices.microphone,
            DeviceTest::Speaker(choice) => *choice == self.devices.speaker,
        };
        let offered = self.offers_tests(directory);
        if offered && still_chosen {
            return;
        }
        let restart = offered && matches!(running.test, DeviceTest::Microphone(_));
        if let Some(testing) = self.testing.take() {
            testing.stop();
        }
        if restart {
            self.start_test(DeviceTest::Microphone(self.devices.microphone.clone()));
        }
    }

    fn sheet_tests(&self, directory: &RoomDirectory) -> SheetTests {
        if !self.offers_tests(directory) {
            return SheetTests::NotOffered;
        }
        self.test_view()
            .map_or(SheetTests::Idle, SheetTests::Testing)
    }

    fn test_view(&self) -> Option<DeviceTestView> {
        let running = self.testing.as_ref()?;
        let (phase, level) = match running.state {
            TestState::Starting => (TestPhase::Starting, 0),
            TestState::Running(level) => (TestPhase::Running, level),
            TestState::Failed => (TestPhase::Failed, 0),
            TestState::Lost => (TestPhase::Lost, 0),
        };
        Some(DeviceTestView {
            kind: running.test.kind(),
            phase,
            level,
        })
    }

    fn sheet_devices(&self) -> Option<Devices> {
        let Some(joined) = &self.joined else {
            return self.listed_devices();
        };
        let mut devices = Devices::new(self.sheet.as_ref()?.draft.clone()?);
        devices.listed(joined.device_lists());
        Some(devices)
    }

    pub(super) fn list_devices(&mut self) {
        let Some(media) = self.media.clone() else {
            return;
        };
        self.listings = self.listings.wrapping_add(1);
        let listing = self.listings;
        let events = self.events.clone();
        self.actions.spawn(async move {
            match media.list_devices().await {
                Ok(lists) => send(&events, CallEvent::DevicesListed { listing, lists }),
                Err(e) => tracing::warn!("the call devices could not be listed: {e}"),
            }
        });
    }

    pub(in crate::app) fn devices_listed(
        &mut self,
        listing: u64,
        lists: DeviceLists,
        directory: &RoomDirectory,
    ) {
        if listing != self.listings {
            tracing::debug!("an older listing of the call devices, dropping");
            return;
        }
        self.listed = Some(lists);
        self.publish(directory);
    }

    fn listed_devices(&self) -> Option<Devices> {
        let mut devices = Devices::new(self.devices.clone());
        devices.listed(self.listed.clone()?);
        Some(devices)
    }

    pub(super) fn close_sheet(&mut self, directory: &RoomDirectory) {
        if self.sheet.take().is_some() {
            self.publish(directory);
        }
    }

    pub(super) fn use_device(
        &mut self,
        kind: DeviceKind,
        key: Option<&DeviceKey>,
        directory: &RoomDirectory,
    ) {
        if self.ring_choosing() {
            self.choose_for_ring(kind, key, directory);
            return;
        }
        if self
            .sheet
            .as_ref()
            .is_some_and(|sheet| sheet.draft.is_some())
        {
            self.choose_for_next_call(kind, key, directory);
            return;
        }
        let Some(joined) = self.joined.as_mut() else {
            let choice = match key {
                None => Some(DeviceChoice::Default),
                Some(_) => self
                    .listed_devices()
                    .and_then(|devices| devices.choice_for(kind, key)),
            };
            let Some(choice) = choice else {
                tracing::debug!(%kind, "no listing names the device to choose");
                return;
            };
            tracing::info!(%kind, "choosing a call device for the next call");
            *self.devices.of_mut(kind) = choice;
            self.publish(directory);
            return;
        };
        let span = joined.span.clone();
        let _in_call = span.enter();
        if let Some(choice) = joined.use_device(kind, key) {
            *self.devices.of_mut(kind) = choice;
        }
        self.publish(directory);
    }

    fn choose_for_next_call(
        &mut self,
        kind: DeviceKind,
        key: Option<&DeviceKey>,
        directory: &RoomDirectory,
    ) {
        let Some(choice) = self
            .sheet_devices()
            .and_then(|devices| devices.choice_for(kind, key))
        else {
            tracing::debug!(%kind, "no device of that key to choose for the next call");
            return;
        };
        tracing::info!(%kind, device = ?choice.key(), "choosing a device for the call the sheet would join");
        if let Some(draft) = self.sheet.as_mut().and_then(|sheet| sheet.draft.as_mut()) {
            *draft.of_mut(kind) = choice;
        }
        self.publish(directory);
    }

    pub(super) fn refresh_devices(&mut self) {
        if let Some(joined) = &self.joined {
            joined.refresh_devices();
        } else if self.sheet.is_some() || self.ring_choosing() {
            self.list_devices();
        }
    }

    pub(super) fn close_sheet_elsewhere(&mut self, selected: Option<&RoomId>) {
        if let Some(sheet) = self.sheet.take_if(|sheet| Some(&sheet.room_id) != selected) {
            tracing::debug!(room_id = %sheet.room_id, "another room was opened, so the call sheet closes");
        }
    }

    pub(super) fn offer_for(&self, room: &Room) -> JoinOffer {
        offer(&OfferFacts {
            media: match self.media.as_ref().map(|media| media.broken()) {
                None => Media::NotBuilt,
                Some(Some(_)) => Media::Broken,
                Some(None) => Media::Ready,
            },
            transports: &self.transports,
            room,
            verified: self.verified,
            in_this_call: self
                .joined
                .as_ref()
                .is_some_and(|joined| joined.room_id == room.id),
        })
    }

    pub(super) fn offer_view(&self, directory: &RoomDirectory) -> Option<OfferView> {
        self.own.as_ref()?;
        let room = directory.room(self.selected.as_ref()?)?;
        Some(OfferView {
            room_id: room.id.clone(),
            offer: self.offer_for(room),
        })
    }

    pub(super) fn sheet_view(&self, directory: &RoomDirectory) -> Option<CallSheetView> {
        let sheet = self.sheet.as_ref()?;
        let room = directory.room(&sheet.room_id)?;
        let leaving = self
            .joined
            .as_ref()
            .filter(|joined| joined.room_id != room.id)
            .map(|joined| LeavingCall {
                room_id: joined.room_id.clone(),
                room_name: joined.room_name().to_owned(),
            });
        let offer = self.offer_for(room);
        let facts = SheetFacts {
            offer,
            leaving,
            me: self.own.as_ref().map(|own| &*own.user),
            devices: self.sheet_devices().map(|devices| devices.view()),
            preview: self.sheet_preview(sheet, offer),
            tests: self.sheet_tests(directory),
        };
        Some(sheet.view(room, facts))
    }
}
