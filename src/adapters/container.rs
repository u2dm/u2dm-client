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
    CANONICAL_AUDIO_TYPES
        .iter()
        .find(|(sniffed, _)| *sniffed == mime)
        .map_or(mime, |(_, canonical)| *canonical)
}

pub fn audio_only_mime(container: &str) -> Option<&'static str> {
    AUDIO_ONLY_CONTAINERS
        .iter()
        .find(|(video, _)| *video == container)
        .map(|(_, audio)| *audio)
}

pub fn launcher_safe_audio_extension(data: &[u8]) -> Option<&'static str> {
    let mime = infer::get(data)?.mime_type();
    LAUNCHER_SAFE_AUDIO_FORMATS
        .iter()
        .find(|(container, _)| *container == mime)
        .map(|(_, extension)| *extension)
}

pub fn launcher_safe_video_extension(data: &[u8]) -> Option<&'static str> {
    let mime = infer::get(data)?.mime_type();
    LAUNCHER_SAFE_VIDEO_FORMATS
        .iter()
        .find(|(container, _)| *container == mime)
        .map(|(_, extension)| *extension)
}
