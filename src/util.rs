use std::fmt::Write;

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(out, "{b:02x}").ok();
    }
    out
}

pub fn hex_encode_id(s: &str) -> String {
    hex_encode(s.as_bytes())
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::fill(buf.as_mut_slice());
    hex_encode(&buf)
}

const BYTE_UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
const BYTE_STEP: f64 = 1024.0;

pub fn format_bytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= BYTE_STEP && unit + 1 < BYTE_UNITS.len() {
        size /= BYTE_STEP;
        unit += 1;
    }
    let label = BYTE_UNITS.get(unit).copied().unwrap_or("B");
    if unit == 0 {
        format!("{bytes} {label}")
    } else {
        format!("{size:.1} {label}")
    }
}
