use std::fmt;

pub const EVENT_MAX_BYTES: usize = 65_536;
pub const ID_MAX_BYTES: usize = 255;
pub const AUTH_EVENTS_MAX: usize = 10;
pub const PREV_EVENTS_MAX: usize = 20;
pub const JSON_INTEGER_MAX: u64 = 9_007_199_254_740_991;
pub const SHA256_BYTES: usize = 32;
pub const ED25519_SIGNATURE_BYTES: usize = 64;
pub const PUBLIC_KEY_BYTES: usize = 32;
pub const AES_KEY_BYTES: usize = 32;
pub const AES_BLOCK_BYTES: usize = 16;

const MEGOLM_VERSION_BYTES: usize = 1;
const MEGOLM_TAG_BYTES: usize = 1;
const MEGOLM_MAC_BYTES: usize = 8;
const MEGOLM_INDEX_MAX: u64 = u32::MAX as u64;
const VARINT_PAYLOAD_BITS: u32 = 7;
const BASE64_GROUP_BYTES: usize = 3;
const BASE64_GROUP_CHARS: usize = 4;
const PERCENT: usize = 100;
const LEAST_SHORTENING: u8 = 1;
const MOST_SHORTENING: u8 = 99;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encryption {
    Plain,
    Megolm,
}

impl Encryption {
    pub fn when(encrypted: bool) -> Self {
        if encrypted { Self::Megolm } else { Self::Plain }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EventIds {
    Hashed,
    #[default]
    WithDomain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrapping {
    Plain,
    Megolm { payload: usize, fields: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventShape {
    pub envelope: usize,
    pub wrapping: Wrapping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSize {
    shape: EventShape,
    content: usize,
}

impl EventSize {
    pub fn of(shape: EventShape, content: usize) -> Self {
        Self { shape, content }
    }

    pub fn bytes(self) -> usize {
        let carried = match self.shape.wrapping {
            Wrapping::Plain => self.content,
            Wrapping::Megolm { payload, fields } => {
                fields.saturating_add(megolm_ciphertext_len(payload.saturating_add(self.content)))
            }
        };
        self.shape.envelope.saturating_add(carried)
    }

    pub fn checked(self) -> Result<Self, Oversize> {
        if self.fits() {
            return Ok(self);
        }
        Err(Oversize {
            bytes: self.bytes(),
            shorten_by: self.shortening(),
        })
    }

    fn fits(self) -> bool {
        self.bytes() <= EVENT_MAX_BYTES
    }

    fn with_content(self, content: usize) -> Self {
        Self { content, ..self }
    }

    fn largest_fitting_content(self) -> usize {
        let mut fitting = 0;
        let mut overflowing = self.content;
        while overflowing.saturating_sub(fitting) > 1 {
            let middle = fitting + (overflowing - fitting) / 2;
            if self.with_content(middle).fits() {
                fitting = middle;
            } else {
                overflowing = middle;
            }
        }
        fitting
    }

    fn shortening(self) -> u8 {
        let content = self.content.max(1);
        let cut = content.saturating_sub(self.largest_fitting_content());
        let percent = cut.saturating_mul(PERCENT).div_ceil(content);
        u8::try_from(percent)
            .unwrap_or(MOST_SHORTENING)
            .clamp(LEAST_SHORTENING, MOST_SHORTENING)
    }
}

pub fn unpadded_base64_len(bytes: usize) -> usize {
    bytes
        .saturating_mul(BASE64_GROUP_CHARS)
        .div_ceil(BASE64_GROUP_BYTES)
}

pub fn megolm_ciphertext_len(plaintext: usize) -> usize {
    let encrypted = (plaintext / AES_BLOCK_BYTES)
        .saturating_add(1)
        .saturating_mul(AES_BLOCK_BYTES);
    let message = MEGOLM_VERSION_BYTES
        + MEGOLM_TAG_BYTES
        + varint_len(MEGOLM_INDEX_MAX)
        + MEGOLM_TAG_BYTES
        + varint_len(u64::try_from(encrypted).unwrap_or(u64::MAX))
        + encrypted
        + MEGOLM_MAC_BYTES
        + ED25519_SIGNATURE_BYTES;
    unpadded_base64_len(message)
}

fn varint_len(value: u64) -> usize {
    let bits = u64::BITS - value.leading_zeros();
    usize::try_from(bits.div_ceil(VARINT_PAYLOAD_BITS).max(1)).unwrap_or(usize::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Oversize {
    bytes: usize,
    shorten_by: u8,
}

impl Oversize {
    pub fn shorten_by(self) -> u8 {
        self.shorten_by
    }
}

impl fmt::Display for Oversize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} bytes against the {EVENT_MAX_BYTES} byte limit, {}% too long",
            self.bytes, self.shorten_by
        )
    }
}
