use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use super::DEMO_FPS;
use crate::domain::call::video::{FeedId, FrameSize, Framing};
use crate::ports::call::{FrameSink, VideoFrame};

pub(super) const SOURCE: FrameSize = FrameSize::new(1280, 720);

pub(super) const PORTRAIT_SOURCE: FrameSize = FrameSize::new(720, 1280);

pub(super) const SCREEN_SOURCE: FrameSize = FrameSize::new(2560, 1440);

pub(super) const SENT_SCREEN_SOURCE: FrameSize = FrameSize::new(1920, 1080);

const BAR_STEP: u64 = 6;

const BAR_SHARE: u32 = 12;

const WINDOW_INSET: u32 = 8;

const TITLE_SHARE: u32 = 16;

const TEXT_MARGIN: u32 = 24;

const LINE_PITCH: u32 = 22;

const TEXT_HEIGHT: u32 = 9;

const WORD_SLOT: u32 = 56;

const WORD_SHORTEST: u32 = 14;

const WORD_SPREAD: u32 = 34;

const GLYPH_PITCH: u32 = 4;

const LINE_WORDS_LEAST: u32 = 4;

const LINE_WORDS_SPREAD: u32 = 18;

const DESKTOP: [u8; 3] = [0x2f, 0x3b, 0x52];

const WINDOW: [u8; 3] = [0xf2, 0xf2, 0xee];

const TITLE: [u8; 3] = [0x4c, 0x6e, 0xa8];

const INK: [u8; 3] = [0x30, 0x34, 0x3a];

pub(super) fn paint(
    running: &AtomicBool,
    size: &Mutex<FrameSize>,
    source: FrameSize,
    framing: Framing,
    hue: [u8; 3],
    sink: &FrameSink,
) {
    let mut frame_number = 0_u64;
    while running.load(Ordering::Acquire) {
        let wanted = size.lock().map_or(source, |size| *size);
        let painted = Painted {
            size: source.framed(wanted, framing),
            source,
            look: match framing {
                Framing::Cover => Look::Camera(hue),
                Framing::Contain => Look::Screen,
            },
            frame_number,
        };
        sink(&painted);
        frame_number = frame_number.wrapping_add(1);
        thread::sleep(Duration::from_millis(1000 / DEMO_FPS));
    }
}

pub(super) fn hue_of(feed: &FeedId) -> [u8; 3] {
    let sum = feed.as_ref().bytes().fold(0_u32, |sum, byte| {
        sum.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    let [a, b, c, _] = sum.to_le_bytes();
    [a | 0x40, b | 0x40, c | 0x40]
}

#[derive(Clone, Copy)]
enum Look {
    Camera([u8; 3]),
    Screen,
}

struct Painted {
    size: FrameSize,
    source: FrameSize,
    look: Look,
    frame_number: u64,
}

impl Painted {
    fn colour_at(&self, x: u32, y: u32, bar_x: u32, bar_width: u32) -> [u8; 3] {
        let lit = x >= bar_x && x < bar_x.saturating_add(bar_width);
        match self.look {
            Look::Camera(_) if lit => [u8::MAX; 3],
            Look::Camera(hue) => hue,
            Look::Screen => self.screen_at(
                in_source(x, self.size.width, self.source.width),
                in_source(y, self.size.height, self.source.height),
                lit,
            ),
        }
    }

    fn screen_at(&self, x: u32, y: u32, lit: bool) -> [u8; 3] {
        let width = self.source.width.max(1);
        let height = self.source.height.max(1);
        let inset_x = width / WINDOW_INSET;
        let inset_y = height / WINDOW_INSET;
        let inside = x >= inset_x && x < width - inset_x && y >= inset_y && y < height - inset_y;
        let text_top = inset_y + height / TITLE_SHARE;
        if !inside {
            DESKTOP
        } else if y < text_top || lit {
            TITLE
        } else if inked(x - inset_x, y - text_top) {
            INK
        } else {
            WINDOW
        }
    }
}

fn in_source(at: u32, shown: u32, source: u32) -> u32 {
    let scaled = u64::from(at) * u64::from(source) / u64::from(shown.max(1));
    u32::try_from(scaled).unwrap_or(u32::MAX)
}

fn inked(across: u32, down: u32) -> bool {
    if down % LINE_PITCH >= TEXT_HEIGHT || across < TEXT_MARGIN {
        return false;
    }
    let line = down / LINE_PITCH;
    let along = across - TEXT_MARGIN;
    let slot = along / WORD_SLOT;
    let words = LINE_WORDS_LEAST + mixed(line, u32::MAX) % LINE_WORDS_SPREAD;
    let word = WORD_SHORTEST + mixed(line, slot) % WORD_SPREAD;
    slot < words && along % WORD_SLOT < word && along % GLYPH_PITCH != GLYPH_PITCH - 1
}

const fn mixed(line: u32, slot: u32) -> u32 {
    let spread = line.wrapping_mul(0x9e37_79b9) ^ slot.wrapping_mul(0x85eb_ca6b);
    spread.rotate_left(13).wrapping_mul(0xc2b2_ae35) >> 16
}

impl VideoFrame for Painted {
    fn size(&self) -> FrameSize {
        self.size
    }

    fn source(&self) -> FrameSize {
        self.source
    }

    fn write_rgba(&self, rgba: &mut [u8]) -> bool {
        let width = self.size.width.max(1);
        let bar_width = (width / BAR_SHARE).max(1);
        let bar_x =
            u32::try_from(self.frame_number.wrapping_mul(BAR_STEP) % u64::from(width)).unwrap_or(0);
        for (index, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let index = u32::try_from(index).unwrap_or(0);
            let [red, green, blue] = self.colour_at(index % width, index / width, bar_x, bar_width);
            *pixel = [red, green, blue, u8::MAX];
        }
        true
    }
}
