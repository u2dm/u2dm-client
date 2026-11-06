use super::faces::apply_names;
use crate::adapters::ui::dto::{call_offer_mark, sheet_preview_mark, test_phase_mark};
use crate::adapters::ui::props::{BoolProp, IntProp, StringProp, UiProps};
use crate::commands::view::{CallSheetView, CallView, JoinOffer, OfferView, SheetPreview};
use crate::domain::call::video::FeedId;
use crate::util::saturating_i32;

pub(super) fn apply_offer(w: &impl UiProps, offer: Option<&OfferView>) {
    w.set_text(
        StringProp::OfferRoomId,
        offer.map_or("", |offer| offer.room_id.as_ref()),
    );
    let join = offer.map(|offer| offer.offer);
    if let Some(JoinOffer::Refused(refusal)) = join {
        w.set_call_refusal(refusal);
    }
    w.set_call_offer(call_offer_mark(join));
}

pub(super) fn apply_sheet(w: &impl UiProps, sheet: Option<&CallSheetView>) {
    let Some(sheet) = sheet else {
        w.set_bool(BoolProp::SheetVisible, false);
        apply_sheet_preview(w, None);
        w.set_call_sheet_test_phase(test_phase_mark(None));
        return;
    };
    w.set_text(StringProp::SheetRoomId, sheet.room_id.as_ref());
    w.set_text(StringProp::SheetRoomName, &sheet.room_name);
    if let JoinOffer::Refused(refusal) = sheet.offer {
        w.set_call_sheet_refusal(refusal);
    }
    w.set_call_sheet_offer(call_offer_mark(Some(sheet.offer)));
    w.set_bool(BoolProp::SheetDirect, sheet.direct);
    w.set_bool(BoolProp::SheetVideo, sheet.video);
    w.set_int(IntProp::SheetPeople, saturating_i32(sheet.people));
    apply_names(
        w,
        &sheet.others,
        [StringProp::SheetPeopleFirst, StringProp::SheetPeopleSecond],
        IntProp::SheetPeopleMore,
    );
    apply_names(
        w,
        &sheet.bridges,
        [
            StringProp::SheetBridgesFirst,
            StringProp::SheetBridgesSecond,
        ],
        IntProp::SheetBridgesMore,
    );
    w.set_bool(BoolProp::SheetMicrophone, sheet.proposed.microphone);
    w.set_bool(BoolProp::SheetCamera, sheet.proposed.camera);
    apply_sheet_preview(w, sheet.preview.as_ref());
    w.set_bool(BoolProp::SheetTests, sheet.tests.offered());
    let running = sheet.tests.running();
    if let Some(test) = running {
        w.set_call_sheet_test_kind(test.kind);
        w.set_int(IntProp::SheetTestLevel, i32::from(test.level));
    }
    w.set_call_sheet_test_phase(test_phase_mark(running.map(|test| test.phase)));
    let leaving = sheet.leaving.as_ref();
    w.set_text(
        StringProp::SheetLeavingId,
        leaving.map_or("", |leaving| leaving.room_id.as_ref()),
    );
    w.set_text(
        StringProp::SheetLeavingName,
        leaving.map_or("", |leaving| leaving.room_name.as_str()),
    );
    w.set_bool(BoolProp::SheetEnded, sheet.ended);
    w.set_int(IntProp::SheetSerial, sheet.serial);
    w.set_bool(BoolProp::SheetVisible, true);
}

fn apply_sheet_preview(w: &impl UiProps, preview: Option<&SheetPreview>) {
    let call = match preview {
        Some(SheetPreview::InCall { room_name, .. } | SheetPreview::Elsewhere { room_name }) => {
            room_name.as_str()
        }
        _ => "",
    };
    w.set_text(StringProp::SheetPreviewCall, call);
    w.set_text(
        StringProp::SheetPreviewFeed,
        preview
            .and_then(SheetPreview::feed)
            .map_or("", |feed| feed.as_ref()),
    );
    if let Some(SheetPreview::Failed(trouble)) = preview {
        w.set_call_preview_trouble(*trouble);
    }
    w.set_call_sheet_preview(sheet_preview_mark(preview));
}

pub(super) fn previewed_feed(call: &CallView) -> Option<&FeedId> {
    call.sheet
        .as_ref()
        .and_then(|sheet| sheet.preview.as_ref())
        .and_then(SheetPreview::feed)
}
