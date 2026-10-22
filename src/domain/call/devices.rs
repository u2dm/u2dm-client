use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    Microphone,
    Speaker,
    Camera,
}

impl DeviceKind {
    pub const ALL: [Self; 3] = [Self::Microphone, Self::Speaker, Self::Camera];

    pub const fn has_system_default(self) -> bool {
        !matches!(self, Self::Camera)
    }
}

impl fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Microphone => "microphone",
            Self::Speaker => "speaker",
            Self::Camera => "camera",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceKey(String);

impl DeviceKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

impl AsRef<str> for DeviceKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaDevice {
    pub key: DeviceKey,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ByKind<T> {
    pub microphone: T,
    pub speaker: T,
    pub camera: T,
}

impl<T> ByKind<T> {
    pub const fn of(&self, kind: DeviceKind) -> &T {
        match kind {
            DeviceKind::Microphone => &self.microphone,
            DeviceKind::Speaker => &self.speaker,
            DeviceKind::Camera => &self.camera,
        }
    }

    pub const fn of_mut(&mut self, kind: DeviceKind) -> &mut T {
        match kind {
            DeviceKind::Microphone => &mut self.microphone,
            DeviceKind::Speaker => &mut self.speaker,
            DeviceKind::Camera => &mut self.camera,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DeviceChoice {
    #[default]
    Default,
    Device(MediaDevice),
}

impl DeviceChoice {
    pub const fn key(&self) -> Option<&DeviceKey> {
        match self {
            Self::Default => None,
            Self::Device(device) => Some(&device.key),
        }
    }

    pub fn resolve<'a>(
        &self,
        kind: DeviceKind,
        connected: &'a [MediaDevice],
        failed: Option<&DeviceKey>,
    ) -> Option<&'a MediaDevice> {
        let usable = |device: &&'a MediaDevice| failed != Some(&device.key);
        let chosen = self.key().and_then(|key| {
            connected
                .iter()
                .filter(usable)
                .find(|device| device.key == *key)
        });
        if kind.has_system_default() {
            chosen
        } else {
            chosen.or_else(|| connected.iter().find(usable))
        }
    }
}

pub type DeviceChoices = ByKind<DeviceChoice>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceLists {
    pub connected: ByKind<Vec<MediaDevice>>,
    pub defaults: ByKind<Option<String>>,
}

impl DeviceLists {
    pub fn is_connected(&self, kind: DeviceKind, key: &DeviceKey) -> bool {
        self.connected
            .of(kind)
            .iter()
            .any(|device| device.key == *key)
    }
}

#[cfg_attr(not(feature = "calls"), allow(dead_code))]
pub fn numbered(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    names
        .into_iter()
        .map(|name| {
            let count = seen.entry(name.clone()).or_insert(0);
            *count += 1;
            if *count == 1 {
                name
            } else {
                format!("{name} ({count})")
            }
        })
        .collect()
}
