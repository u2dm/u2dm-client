use ffmpeg_next::codec::packet::side_data::Type as SideDataType;
use ffmpeg_next::format::stream::Stream;
use image::metadata::Orientation;
use image::{DynamicImage, RgbImage};

const FIXED_POINT_ONE: f64 = 65_536.0;
const RIGHT_ANGLE_TOLERANCE_DEGREES: f64 = 1.0;
const FULL_TURN_DEGREES: f64 = 360.0;

struct LinearPart {
    a: i32,
    b: i32,
    c: i32,
    d: i32,
}

impl LinearPart {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let words: Vec<i32> = bytes
            .chunks_exact(size_of::<i32>())
            .filter_map(|chunk| <[u8; 4]>::try_from(chunk).ok())
            .map(i32::from_ne_bytes)
            .collect();
        let [a, b, _, c, d, _, _, _, _] = words.as_slice() else {
            return None;
        };
        Some(Self {
            a: *a,
            b: *b,
            c: *c,
            d: *d,
        })
    }

    fn clockwise_degrees(&self) -> Option<f64> {
        if (self.a == 0 && self.c == 0) || (self.b == 0 && self.d == 0) {
            return None;
        }
        let [a, b, c, d] =
            [self.a, self.b, self.c, self.d].map(|word| f64::from(word) / FIXED_POINT_ONE);
        let angle = (b / b.hypot(d)).atan2(a / a.hypot(c)).to_degrees();
        Some(angle.round().rem_euclid(FULL_TURN_DEGREES))
    }

    fn orientation(&self) -> Orientation {
        let Some(clockwise) = self.clockwise_degrees() else {
            return Orientation::NoTransforms;
        };
        let near = |target: f64| (clockwise - target).abs() < RIGHT_ANGLE_TOLERANCE_DEGREES;
        let mirrored_vertically = self.d < 0;
        if near(90.0) {
            if self.c > 0 {
                Orientation::Rotate90FlipH
            } else {
                Orientation::Rotate90
            }
        } else if near(180.0) {
            if mirrored_vertically {
                Orientation::Rotate180
            } else {
                Orientation::FlipHorizontal
            }
        } else if near(270.0) {
            if self.c < 0 {
                Orientation::Rotate270FlipH
            } else {
                Orientation::Rotate270
            }
        } else if near(0.0) && mirrored_vertically {
            Orientation::FlipVertical
        } else {
            Orientation::NoTransforms
        }
    }
}

pub fn of_stream(stream: &Stream<'_>) -> Orientation {
    stream
        .side_data()
        .find(|side_data| side_data.kind() == SideDataType::DisplayMatrix)
        .and_then(|side_data| LinearPart::parse(side_data.data()))
        .map_or(Orientation::NoTransforms, |matrix| matrix.orientation())
}

pub fn displayed_extent(width: u32, height: u32, orientation: Orientation) -> (u32, u32) {
    match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (height, width),
        Orientation::NoTransforms
        | Orientation::Rotate180
        | Orientation::FlipHorizontal
        | Orientation::FlipVertical => (width, height),
    }
}

pub fn upright(picture: RgbImage, orientation: Orientation) -> RgbImage {
    let mut picture = DynamicImage::ImageRgb8(picture);
    picture.apply_orientation(orientation);
    picture.into_rgb8()
}
