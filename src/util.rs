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
