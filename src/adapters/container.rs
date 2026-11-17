use crate::util::lookup;

const LAUNCHER_SAFE_VIDEO_FORMATS: &[(&str, &str)] = &[
    ("video/mp4", "mp4"),
    ("video/x-m4v", "m4v"),
    ("video/quicktime", "mov"),
    ("video/3gpp", "3gp"),
    ("video/3gpp2", "3g2"),
    ("video/webm", "webm"),
    ("video/x-matroska", "mkv"),
    ("video/x-msvideo", "avi"),
];

const LAUNCHER_SAFE_AUDIO_FORMATS: &[(&str, &str)] = &[
    ("audio/mpeg", "mp3"),
    ("audio/m4a", "m4a"),
    ("audio/m4b", "m4b"),
    ("audio/opus", "opus"),
    ("audio/ogg", "ogg"),
    ("audio/x-flac", "flac"),
    ("audio/x-wav", "wav"),
    ("audio/aac", "aac"),
    ("audio/amr", "amr"),
    ("video/mp4", "m4a"),
    ("video/3gpp", "3gp"),
    ("video/3gpp2", "3g2"),
    ("video/webm", "webm"),
    ("video/x-matroska", "mka"),
];

const CANONICAL_AUDIO_TYPES: &[(&str, &str)] = &[
    ("audio/m4a", "audio/mp4"),
    ("audio/x-m4a", "audio/mp4"),
    ("audio/m4b", "audio/mp4"),
    ("audio/x-flac", "audio/flac"),
    ("audio/x-wav", "audio/wav"),
];

const AUDIO_ONLY_CONTAINERS: &[(&str, &str)] = &[
    ("video/mp4", "audio/mp4"),
    ("video/x-m4v", "audio/mp4"),
    ("video/quicktime", "audio/mp4"),
    ("video/3gpp", "audio/3gpp"),
    ("video/3gpp2", "audio/3gpp2"),
    ("video/webm", "audio/webm"),
    ("video/x-matroska", "audio/x-matroska"),
];

const FILE_TYPE_BOX: &[u8] = b"ftyp";

pub fn canonical_audio_mime(mime: &str) -> &str {
    lookup(CANONICAL_AUDIO_TYPES, &mime).unwrap_or(mime)
}

pub fn audio_only_mime(container: &str) -> Option<&'static str> {
    lookup(AUDIO_ONLY_CONTAINERS, &container)
}

pub fn extension_of(mime: &str) -> Option<&'static str> {
    lookup(LAUNCHER_SAFE_VIDEO_FORMATS, &mime)
        .or_else(|| lookup(LAUNCHER_SAFE_AUDIO_FORMATS, &mime))
}

pub fn sniffed_mime(data: &[u8]) -> Option<&'static str> {
    branded_mime(data).or_else(|| infer::get(data).map(|kind| kind.mime_type()))
}

pub fn branded_extension(data: &[u8]) -> Option<&'static str> {
    extension_of(branded_mime(data)?)
}

pub fn launcher_safe_audio_extension(data: &[u8]) -> Option<&'static str> {
    lookup(LAUNCHER_SAFE_AUDIO_FORMATS, &sniffed_mime(data)?)
}

pub fn launcher_safe_video_extension(data: &[u8]) -> Option<&'static str> {
    lookup(LAUNCHER_SAFE_VIDEO_FORMATS, &sniffed_mime(data)?)
}

fn branded_mime(data: &[u8]) -> Option<&'static str> {
    if data.get(4..8)? != FILE_TYPE_BOX {
        return None;
    }
    match data.get(8..12)? {
        [b'3', b'g', b'2', _] => Some("video/3gpp2"),
        [b'3', b'g', _, _] => Some("video/3gpp"),
        b"M4B " => Some("audio/m4b"),
        _ => None,
    }
}
