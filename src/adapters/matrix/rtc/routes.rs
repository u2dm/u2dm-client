use matrix_sdk::Client;
use matrix_sdk::ruma::api::FeatureFlag;

pub(super) const TRANSPORTS_STABLE: &str = "org.matrix.msc4143.stable";
pub(super) const DELAYED_EVENTS_STABLE: &str = "org.matrix.msc4140.stable";

pub(super) async fn advertises(client: &Client, flag: &str) -> bool {
    match client.unstable_features().await {
        Ok(features) => features.contains(&FeatureFlag::from(flag)),
        Err(e) => {
            tracing::debug!(flag, "could not read the homeserver's features: {e}");
            false
        }
    }
}
