const LAUNCHER_SAFE_VIDEO_FORMATS: &[(&str, &str)] = &[
    ("video/mp4", "mp4"),
    ("video/quicktime", "mov"),
    ("video/webm", "webm"),
    ("video/x-matroska", "mkv"),
    ("video/x-msvideo", "avi"),
];

pub fn launcher_safe_video_extension(data: &[u8]) -> Option<&'static str> {
    let mime = infer::get(data)?.mime_type();
    LAUNCHER_SAFE_VIDEO_FORMATS
        .iter()
        .find(|(container, _)| *container == mime)
        .map(|(_, extension)| *extension)
}
