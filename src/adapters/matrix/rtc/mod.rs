mod delayed;
mod grants;
mod http;
mod join;
mod keys;
mod members;
mod routes;
mod signals;
mod transports;

pub(super) use self::join::MatrixRtcMembership;
pub(super) use self::members::room_call;
pub(super) use self::signals::MatrixCallSignals;
use crate::domain::call::Capability;

pub(in crate::adapters::matrix) const CALL_MEMBER_TYPE: &str = "org.matrix.msc3401.call.member";
const NOTIFICATION_TYPE: &str = "org.matrix.msc4075.rtc.notification";
const NOTIFICATION_TYPE_STABLE: &str = "m.rtc.notification";
const DECLINE_TYPE: &str = "org.matrix.msc4310.rtc.decline";
const DECLINE_TYPE_STABLE: &str = "m.rtc.decline";
const ENCRYPTED_TYPE: &str = "m.room.encrypted";
const KEYS_TYPE: &str = "io.element.call.encryption_keys";

const fn capability_word(capability: Capability) -> &'static str {
    match capability {
        Capability::RenderAudio => "m.render_audio",
        Capability::RenderVideo => "m.render_video",
        Capability::ExposesE2ee => "m.exposes_e2ee",
    }
}

fn capability_named(word: &str) -> Option<Capability> {
    [
        Capability::RenderAudio,
        Capability::RenderVideo,
        Capability::ExposesE2ee,
    ]
    .into_iter()
    .find(|capability| capability_word(*capability) == word)
}
