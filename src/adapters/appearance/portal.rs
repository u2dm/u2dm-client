use futures_util::StreamExt;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, Proxy};

use crate::ports::appearance::{AppearancePort, MotionPreference, MotionSink};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const SETTING_CHANGED: &str = "SettingChanged";
const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
const APPEARANCE: &str = "org.freedesktop.appearance";
const REDUCED_MOTION: &str = "reduced-motion";
const REDUCED_MOTION_ASKED: u32 = 1;
const GNOME_INTERFACE: &str = "org.gnome.desktop.interface";
const ENABLE_ANIMATIONS: &str = "enable-animations";

pub(super) struct PortalAppearance;

impl AppearancePort for PortalAppearance {
    fn follow_motion(&self, sink: MotionSink) {
        tokio::spawn(async move {
            if let Err(e) = follow(&sink).await {
                tracing::debug!("not following the desktop's motion preference: {e}");
            }
        });
    }
}

async fn follow(sink: &MotionSink) -> zbus::Result<()> {
    let connection = Connection::session().await?;
    let settings = Proxy::new(&connection, PORTAL, PORTAL_PATH, SETTINGS).await?;
    let mut changes = settings.receive_signal(SETTING_CHANGED).await?;
    let mut reported = motion(&settings).await;
    if let Some(preference) = reported {
        tracing::debug!(?preference, "read the desktop's motion preference");
        sink(preference);
    }
    while let Some(change) = changes.next().await {
        let Ok((namespace, key, _)) = change.body().deserialize::<(String, String, OwnedValue)>()
        else {
            continue;
        };
        if !names_motion(&namespace, &key) {
            continue;
        }
        let now = motion(&settings).await;
        if now != reported
            && let Some(preference) = now
        {
            tracing::debug!(?preference, "the desktop's motion preference changed");
            sink(preference);
        }
        reported = now;
    }
    Ok(())
}

fn names_motion(namespace: &str, key: &str) -> bool {
    (namespace, key) == (APPEARANCE, REDUCED_MOTION)
        || (namespace, key) == (GNOME_INTERFACE, ENABLE_ANIMATIONS)
}

async fn motion(settings: &Proxy<'_>) -> Option<MotionPreference> {
    if let Ok(value) = read(settings, APPEARANCE, REDUCED_MOTION).await {
        let reduced = matches!(value.downcast_ref::<u32>(), Ok(REDUCED_MOTION_ASKED));
        return Some(preference(reduced));
    }
    let value = read(settings, GNOME_INTERFACE, ENABLE_ANIMATIONS)
        .await
        .ok()?;
    let animated = value.downcast_ref::<bool>().ok()?;
    Some(preference(!animated))
}

fn preference(reduced: bool) -> MotionPreference {
    if reduced {
        MotionPreference::Reduced
    } else {
        MotionPreference::Full
    }
}

async fn read(settings: &Proxy<'_>, namespace: &str, key: &str) -> zbus::Result<OwnedValue> {
    match settings.call("ReadOne", &(namespace, key)).await {
        Err(zbus::Error::MethodError(name, ..)) if name.as_str() == UNKNOWN_METHOD => {
            settings.call("Read", &(namespace, key)).await
        }
        answer => answer,
    }
}
