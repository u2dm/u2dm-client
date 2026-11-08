use crate::util::lookup;

const LAUNCHER_SAFE_VIDEO_FORMATS: &[(&str, &str)] = &[
    ("video/mp4", "mp4"),
    ("video/quicktime", "mov"),
    ("video/webm", "webm"),
    ("video/x-matroska", "mkv"),
    ("video/x-msvideo", "avi"),
];

const LAUNCHER_SAFE_AUDIO_FORMATS: &[(&str, &str)] = &[
    ("audio/mpeg", "mp3"),
    ("audio/m4a", "m4a"),
    ("audio/opus", "opus"),
    ("audio/ogg", "ogg"),
    ("audio/x-flac", "flac"),
    ("audio/x-wav", "wav"),
    ("audio/aac", "aac"),
    ("audio/amr", "amr"),
    ("video/mp4", "m4a"),
    ("video/webm", "webm"),
    ("video/x-matroska", "mka"),
];

const CANONICAL_AUDIO_TYPES: &[(&str, &str)] = &[
    ("audio/m4a", "audio/mp4"),
    ("audio/x-m4a", "audio/mp4"),
    ("audio/x-flac", "audio/flac"),
    ("audio/x-wav", "audio/wav"),
];

const AUDIO_ONLY_CONTAINERS: &[(&str, &str)] = &[
    ("video/mp4", "audio/mp4"),
    ("video/quicktime", "audio/mp4"),
    ("video/webm", "audio/webm"),
    ("video/x-matroska", "audio/x-matroska"),
];

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

pub fn launcher_safe_audio_extension(data: &[u8]) -> Option<&'static str> {
    lookup(LAUNCHER_SAFE_AUDIO_FORMATS, &infer::get(data)?.mime_type())
}

pub fn launcher_safe_video_extension(data: &[u8]) -> Option<&'static str> {
    lookup(LAUNCHER_SAFE_VIDEO_FORMATS, &infer::get(data)?.mime_type())
}
