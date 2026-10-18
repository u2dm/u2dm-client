use std::fmt;

#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
const SMALLEST_SIDE: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeedId(String);

impl FeedId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl AsRef<str> for FeedId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FeedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

impl FrameSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    #[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
    pub const fn pixels(self) -> usize {
        self.width as usize * self.height as usize
    }

    #[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
    pub fn framed(self, wanted: Self, framing: Framing) -> Self {
        if self.width == 0 || self.height == 0 {
            return self;
        }
        let across = f64::from(wanted.width.max(1)) / f64::from(self.width);
        let down = f64::from(wanted.height.max(1)) / f64::from(self.height);
        let filling = match framing {
            Framing::Cover => across.max(down),
            Framing::Contain => across.min(down),
        };
        let scale = filling.min(1.0);
        Self {
            width: scaled(self.width, scale),
            height: scaled(self.height, scale),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    Cover,
    Contain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum VideoKind {
    Camera,
    Screen,
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
fn scaled(side: u32, scale: f64) -> u32 {
    let even = ((f64::from(side) * scale / 2.0).round() * 2.0) as u32;
    even.clamp(SMALLEST_SIDE.min(side), side)
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum Camera {
    On(FeedId),
    Failed(String),
}

impl fmt::Display for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::On(feed) => write!(f, "on ({feed})"),
            Self::Failed(why) => write!(f, "failed: {why}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum ScreenRefusal {
    NotWayland,
    NoPortal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum ScreenShare {
    Off,
    On,
    Failed(String),
    Unavailable(ScreenRefusal),
}
