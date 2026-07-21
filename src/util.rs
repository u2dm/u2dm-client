use std::ffi::OsString;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

pub fn hex_encode_id(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        write!(out, "{b:02x}").ok();
    }
    out
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::fill(buf.as_mut_slice());
    let mut out = String::with_capacity(bytes * 2);
    for b in &buf {
        write!(out, "{b:02x}").ok();
    }
    out
}

pub fn unique_tmp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = process::id();
    let mut name = match path.file_name() {
        Some(file_name) => file_name.to_os_string(),
        None => OsString::from("tmp"),
    };
    name.push(format!(".{pid}.{seq}.tmp"));
    path.with_file_name(name)
}
