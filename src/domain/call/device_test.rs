use super::devices::{DeviceChoice, DeviceKind};

#[cfg_attr(not(feature = "calls"), allow(dead_code))]
const LOUDNESS_FLOOR_DB: f64 = -60.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceTest {
    Microphone(DeviceChoice),
    Speaker(DeviceChoice),
}

impl DeviceTest {
    pub const fn kind(&self) -> DeviceKind {
        match self {
            Self::Microphone(_) => DeviceKind::Microphone,
            Self::Speaker(_) => DeviceKind::Speaker,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum TestNews {
    Level(f32),
    Played,
    Failed(String),
}

#[cfg_attr(not(feature = "calls"), allow(dead_code))]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub fn loudness(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let power = samples
        .iter()
        .map(|sample| {
            let value = f64::from(*sample) / f64::from(i16::MAX);
            value * value
        })
        .sum::<f64>()
        / samples.len() as f64;
    if power <= 0.0 {
        return 0.0;
    }
    let decibels = 10.0 * power.log10();
    ((decibels - LOUDNESS_FLOOR_DB) / -LOUDNESS_FLOOR_DB).clamp(0.0, 1.0) as f32
}
