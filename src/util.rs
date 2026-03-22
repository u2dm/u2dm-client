use std::fmt::Write;

pub fn hex_encode_id(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        write!(out, "{b:02x}").ok();
    }
    out
}
