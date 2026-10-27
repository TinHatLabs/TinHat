//! viewer.rs — which attachments this app shows or opens, and as what (security review 2026-10-26: the desktop side
//! of the web's H1).
//!
//! An attachment's `info.mimetype` is the sender's word. Before this, "Open" saved every non-image as `<event>.img`
//! and handed it to the OS opener (Windows runs the `.img` verb — mount a disk image; Linux sniffs the content and
//! picks a handler), and an `m.image` claiming `image/svg+xml` (or any bytes Qt's image plugins recognise) was decoded
//! inline on every reader's screen. Now:
//! - **Open** works only for the viewer types below; the cached file's extension comes from THIS list (never from the
//!   event), and the bytes must be that type ([`sniff`]) — everything else is Save as only;
//! - **inline** images are decoded only when their BYTES are png / jpeg / gif / webp ([`sniff_image`]; never SVG), at
//!   most [`INLINE_IMAGE_MAX`] bytes; the file gets the sniffed format's extension.
//!
//! Review follow-up (Astra, 2026-10-26): AVIF is no longer an inline format on the desktop. No shipped Qt here has an
//! AVIF reader (Linux: qt6-imageformats has none, kimageformats isn't bundled; Windows: the bundle's imageformats are
//! gif/icns/ico/jpeg/svg/tga/tiff/wbmp/webp), so Qt fell back to guessing the format from the content — and an SVG
//! behind a forged `ftyp…avif` header was decoded (its `xlink:href="//host/share/x.png"` is an SMB fetch on Windows).
//! The four formats left are each recognised by their own decoder from the magic [`sniff`] already checked, so Qt never
//! has to guess. ISO-BMFF headers (avif, mp4, m4a) must be a whole, well-formed `ftyp` box. Text attachments are never
//! handed to an outside opener on the desktop ([`open_ext`]): what an outside handler makes of a `.txt` is beyond any
//! prefix check. Text-like ones ([`text_like`]: plain text, JSON, CSV, Markdown, logs, source) are shown IN THE APP
//! instead, as plain text, when their bytes pass the `txt` sniff (launch-gate's web review #8; the web opens the same
//! files as `text/plain` in a tab).
//!
//! The list is Tin Hat Web's (core/mediatype.ts, the same MIME types); both apps replay the shared vectors
//! `testvectors/member-content.json` (sections `viewer` and `sniff`).

/// An inline image is decoded in the bubble only up to this many bytes (larger: a placeholder; Open / Save as).
pub const INLINE_IMAGE_MAX: u64 = 32 * 1024 * 1024;

/// The viewer types: (mime, extension, inline image).
const VIEWER: &[(&str, &str, bool)] = &[
    ("image/png", "png", true),
    ("image/jpeg", "jpg", true),
    ("image/jpg", "jpg", true),
    ("image/pjpeg", "jpg", true),
    ("image/gif", "gif", true),
    ("image/webp", "webp", true),
    ("image/avif", "avif", false),
    ("video/mp4", "mp4", false),
    ("video/webm", "webm", false),
    ("audio/mpeg", "mp3", false),
    ("audio/mp3", "mp3", false),
    ("audio/ogg", "ogg", false),
    ("audio/opus", "opus", false),
    ("audio/wav", "wav", false),
    ("audio/x-wav", "wav", false),
    ("audio/wave", "wav", false),
    ("audio/vnd.wave", "wav", false),
    ("audio/flac", "flac", false),
    ("audio/x-flac", "flac", false),
    ("audio/mp4", "m4a", false),
    ("audio/x-m4a", "m4a", false),
    ("audio/m4a", "m4a", false),
    ("audio/aac", "aac", false),
    ("audio/webm", "weba", false),
    ("application/pdf", "pdf", false),
    ("text/plain", "txt", false),
];

/// Every extension a cached attachment may carry when it is handed to the OS opener (LinkGate.qml's openMedia checks
/// the same list): the viewer list's minus [`SAVE_ONLY_EXTS`].
pub const OPEN_EXTS: &[&str] = &["png", "jpg", "gif", "webp", "avif", "mp4", "webm", "mp3", "ogg", "opus", "wav", "flac", "m4a", "aac", "weba", "pdf"];

/// The inline image formats, by extension (no AVIF: see the module doc).
pub const INLINE_EXTS: &[&str] = &["png", "jpg", "gif", "webp"];

/// Viewer types the desktop keeps to "Save as" anyway (the module doc): never handed to the OS opener.
pub const SAVE_ONLY_EXTS: &[&str] = &["txt"];

/// The most of a text-like attachment the in-app text view downloads and shows (bigger: Save as).
pub const TEXT_VIEW_MAX: u64 = 1024 * 1024;

/// Types of text-like attachments beyond `text/*` (Tin Hat Web core/mediatype.ts TEXT_LIKE, the same list).
const TEXT_LIKE: &[&str] = &[
    "application/json",
    "application/ld+json",
    "application/x-ndjson",
    "application/javascript",
    "application/x-javascript",
    "application/ecmascript",
    "application/x-sh",
    "application/x-shellscript",
    "application/yaml",
    "application/x-yaml",
    "application/toml",
    "application/sql",
    "application/x-python",
    "application/x-log",
];
/// File-name extensions that make an UNTYPED attachment (no type, or application/octet-stream) text-like (Tin Hat Web
/// core/mediatype.ts TEXT_EXT, the same list).
const TEXT_NAME_EXTS: &[&str] = &[
    "txt", "text", "md", "markdown", "log", "csv", "tsv", "json", "ndjson", "yaml", "yml", "toml", "ini", "cfg", "conf",
    "env", "sh", "bash", "zsh", "ps1", "bat", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "rs", "go", "c", "h", "cc",
    "cpp", "hpp", "cs", "java", "kt", "swift", "rb", "php", "pl", "lua", "sql", "css", "scss", "diff", "patch", "srt",
    "vtt", "nfo",
];

/// Is an attachment of this (sender-given) type and file name text-like — shown in the app as plain text when its bytes
/// are (Tin Hat Web's isTextLike, word for word): `text/*` except HTML / XML / XSL, the [`TEXT_LIKE`] types, or an
/// untyped file whose name ends in one of [`TEXT_NAME_EXTS`]. HTML, XML and SVG never are.
pub fn text_like(mime: &str, name: &str) -> bool {
    let e = essence(mime);
    if e.starts_with("text/") {
        return !(e.contains("html") || e.contains("xml") || e.contains("xsl"));
    }
    if TEXT_LIKE.contains(&e.as_str()) {
        return true;
    }
    let ext = name.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).unwrap_or_default();
    (e.is_empty() || e == "application/octet-stream") && TEXT_NAME_EXTS.contains(&ext.as_str())
}

fn essence(mime: &str) -> String {
    mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// The extension an attachment of this (sender-given) type is opened as; `None` = not a viewer type (Save as only).
pub fn viewer_ext(mime: &str) -> Option<&'static str> {
    let m = essence(mime);
    VIEWER.iter().find(|(t, _, _)| *t == m).map(|(_, e, _)| *e)
}

/// The extension an attachment of this type is OPENED as on the desktop (handed to the OS opener): a viewer type that
/// isn't Save as only. `None`: Save as only.
pub fn open_ext(mime: &str) -> Option<&'static str> {
    viewer_ext(mime).filter(|e| !SAVE_ONLY_EXTS.contains(e))
}

/// The sender says it is an image this app decodes inline.
pub fn inline_image(mime: &str) -> bool {
    let m = essence(mime);
    VIEWER.iter().any(|(t, _, i)| *i && *t == m)
}

/// The inline image format the bytes ARE (by their magic), whatever the event said.
pub fn sniff_image(head: &[u8]) -> Option<&'static str> {
    INLINE_EXTS.iter().copied().find(|e| sniff(e, head))
}

fn at(head: &[u8], off: usize, magic: &[u8]) -> bool {
    head.len() >= off + magic.len() && &head[off..off + magic.len()] == magic
}

/// The size of a well-formed ISO-BMFF `ftyp` box at the start of `head`: a 32-bit size of at least 16 (header, major
/// brand, minor version), a whole number of 4-byte brands, at most 4 KiB, and all of it inside `head`. A size that
/// doesn't fit is a forgery, never something to clamp (Astra review: `<!--ftypavif` read as a box of 1 GB).
fn ftyp_box(head: &[u8]) -> Option<usize> {
    if !at(head, 4, b"ftyp") {
        return None;
    }
    let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
    (size >= 16 && size % 4 == 0 && size <= 4096 && size <= head.len()).then_some(size)
}

/// An ISO-BMFF file (a well-formed `ftyp` box) whose major or compatible brands include one of `brands`.
fn ftyp_has(head: &[u8], brands: &[&[u8]]) -> bool {
    let Some(end) = ftyp_box(head) else { return false };
    let has = |b: &[u8]| brands.iter().any(|x| *x == b);
    if has(&head[8..12]) {
        return true;
    }
    // compatible brands start at 16 (after the minor version)
    let mut i = 16;
    while i + 4 <= end {
        if has(&head[i..i + 4]) {
            return true;
        }
        i += 4;
    }
    false
}

/// The bytes (the first few KiB are enough) are what the extension says. `txt`: UTF-8 text without NULs that does not
/// start like a web page or an SVG.
pub fn sniff(ext: &str, head: &[u8]) -> bool {
    match ext {
        "png" => at(head, 0, b"\x89PNG\r\n\x1a\n"),
        "jpg" => at(head, 0, b"\xff\xd8\xff"),
        "gif" => at(head, 0, b"GIF87a") || at(head, 0, b"GIF89a"),
        "webp" => at(head, 0, b"RIFF") && at(head, 8, b"WEBP"),
        "avif" => ftyp_has(head, &[b"avif", b"avis"]),
        "mp4" => ftyp_box(head).is_some() && !ftyp_has(head, &[b"avif", b"avis", b"heic", b"heix", b"mif1"]),
        "m4a" => ftyp_has(head, &[b"M4A ", b"M4B ", b"mp42", b"isom", b"mp41", b"dash"]),
        "webm" | "weba" => at(head, 0, b"\x1a\x45\xdf\xa3"),
        "ogg" | "opus" => at(head, 0, b"OggS"),
        "wav" => at(head, 0, b"RIFF") && at(head, 8, b"WAVE"),
        "flac" => at(head, 0, b"fLaC"),
        "mp3" => at(head, 0, b"ID3") || (head.len() >= 2 && head[0] == 0xff && head[1] & 0xe0 == 0xe0),
        "aac" => at(head, 0, b"ID3") || (head.len() >= 2 && head[0] == 0xff && head[1] & 0xf6 == 0xf0),
        "pdf" => at(head, 0, b"%PDF-"),
        "txt" => plain_text(head),
        _ => false,
    }
}

fn plain_text(head: &[u8]) -> bool {
    let head = &head[..head.len().min(8192)];
    if head.contains(&0) {
        return false;
    }
    let text = match std::str::from_utf8(head) {
        Ok(t) => t,
        // a multi-byte character cut by the 8 KiB window is fine; anything else is not text
        Err(e) if e.error_len().is_none() => std::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or(""),
        Err(_) => return false,
    };
    let t = text.trim_start_matches('\u{feff}').trim_start().to_ascii_lowercase();
    !["<!doctype", "<html", "<?xml", "<svg", "<script", "<head", "<body", "<iframe"].iter().any(|p| t.starts_with(p))
}

/// Someone's picture shown without a click (an avatar, a link preview's image) already in `dir` as `<id>.<ext>`.
pub fn cached_image(dir: &std::path::Path, id: &str) -> Option<std::path::PathBuf> {
    INLINE_EXTS.iter().map(|e| dir.join(format!("{id}.{e}"))).find(|p| p.exists())
}

/// Where someone's picture shown without a click goes: `<dir>/<id>.<ext>` when its BYTES are png / jpeg / gif / webp
/// and at most [`INLINE_IMAGE_MAX`]; `None` (show no picture) for SVG or anything else Qt's image plugins would
/// otherwise sniff and decode.
pub fn image_path_for(dir: &std::path::Path, id: &str, bytes: &[u8]) -> Option<std::path::PathBuf> {
    if bytes.len() as u64 > INLINE_IMAGE_MAX {
        return None;
    }
    let ext = sniff_image(bytes)?;
    Some(dir.join(format!("{id}.{ext}")))
}

/// The first `n` bytes of a file (for [`sniff`] on a cached copy).
pub fn file_head(path: &std::path::Path, n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    let mut got = 0;
    while got < n {
        match f.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => got += k,
        }
    }
    buf.truncate(got);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("testvectors/member-content.json")).unwrap()
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Launch-gate's web review #8 — Tin Hat Web's mediatype.test.ts cases, the same verdicts.
    #[test]
    fn text_like_files_may_be_shown_as_plain_text_pages_never() {
        for (m, n) in [("application/json", "a.json"), ("text/csv", "a.csv"), ("text/markdown", "README.md"), ("text/x-log", "x.log"),
                       ("application/x-sh", "run.sh"), ("", "notes.md"), ("application/octet-stream", "main.rs"), ("text/x-python", "a.py"),
                       ("application/javascript", "a.js"), ("text/plain", "a.txt"), ("Text/Plain; charset=utf-8", "x"), ("", ".env")] {
            assert!(text_like(m, n), "{m} {n}");
        }
        for (m, n) in [("text/html", "a.html"), ("application/xhtml+xml", "a.xhtml"), ("image/svg+xml", "a.svg"), ("text/xml", "a.xml"),
                       ("application/zip", "a.zip"), ("", "a.exe"), ("application/octet-stream", "a.html"), ("image/png", "a.txt"),
                       ("text/xsl", "a.txt"), ("", "md"), ("application/pdf", "a.txt")] {
            assert!(!text_like(m, n), "{m} {n}");
        }
    }

    #[test]
    fn viewer_vectors() {
        let v = vectors();
        let cases = v["viewer"].as_array().unwrap();
        assert!(cases.len() >= 30);
        for c in cases {
            let mime = c["mime"].as_str().unwrap();
            let ext = c["ext"].as_str();
            assert_eq!(viewer_ext(mime), ext, "{mime}");
            assert_eq!(inline_image(mime), c["inline"].as_bool().unwrap(), "{mime} inline");
            if let Some(e) = ext {
                assert!(OPEN_EXTS.contains(&e) || SAVE_ONLY_EXTS.contains(&e), "{e} missing from OPEN_EXTS");
            }
        }
        // every extension the list can produce is in OPEN_EXTS or Save as only (the desktop's text rule)
        for (_, e, _) in VIEWER {
            assert!(OPEN_EXTS.contains(e) != SAVE_ONLY_EXTS.contains(e), "{e}");
        }
        assert_eq!(open_ext("text/plain"), None, "text is Save as only on the desktop");
        assert_eq!(open_ext("application/pdf"), Some("pdf"));
    }

    #[test]
    fn sniff_vectors() {
        let v = vectors();
        let cases = v["sniff"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for c in cases {
            let ext = c["ext"].as_str().unwrap();
            let head = unhex(c["hex"].as_str().unwrap());
            assert_eq!(sniff(ext, &head), c["ok"].as_bool().unwrap(), "{ext} {}", c["hex"]);
        }
    }

    #[test]
    fn inline_images_are_what_their_bytes_are() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some("png"));
        assert_eq!(sniff_image(b"\xff\xd8\xff\xe0\0\x10JFIF"), Some("jpg"));
        assert_eq!(sniff_image(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"), None, "SVG is never inline");
        assert_eq!(sniff_image(b"<?xml version=\"1.0\"?><svg/>"), None);
        assert_eq!(sniff_image(b"BM\x36\0\0\0"), None, "BMP is not on the list");
        assert_eq!(sniff_image(b"II*\0"), None, "TIFF is not on the list");
        assert_eq!(sniff_image(b""), None);
        // an AVIF is not taken for an MP4 to open, and vice versa; AVIF is no inline format on the desktop
        let avif = b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf";
        assert!(sniff("avif", avif));
        assert_eq!(sniff_image(avif), None);
        assert!(!sniff("mp4", avif));
        assert!(sniff("mp4", b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom"));
        // Astra review: a forged box size is no AVIF / MP4 (an SVG behind `<!--ftypavif-->`), nor a truncated box
        let svg = b"<!--ftypavif--><svg xmlns=\"http://www.w3.org/2000/svg\"><image href=\"//host/share/x.png\"/></svg>";
        assert!(!sniff("avif", svg) && !sniff("mp4", svg) && !sniff("m4a", svg));
        assert_eq!(sniff_image(svg), None);
        assert!(!sniff("avif", b"\0\0\0\x1cftypavif\0\0\0\0avif"), "a box longer than the bytes");
        assert!(!sniff("avif", b"\0\0\0\x11ftypavif\0\0\0\0avif\0"), "not whole brands");
        assert!(!sniff("mp4", b"\0\0\0\x08ftypmp42"), "shorter than a box can be");
    }

    #[test]
    fn pictures_shown_without_a_click_are_raster_images() {
        let dir = std::path::Path::new("/nonexistent-tinhat-test");
        assert_eq!(image_path_for(dir, "a", b"\x89PNG\r\n\x1a\nrest"), Some(dir.join("a.png")));
        assert_eq!(image_path_for(dir, "a", b"<svg xmlns=\"http://www.w3.org/2000/svg\"><image href=\"https://evil.example/\"/></svg>"), None);
        assert_eq!(image_path_for(dir, "a", b"II*\0tiff"), None);
        assert_eq!(cached_image(dir, "a"), None);
    }

    #[test]
    fn text_is_text() {
        assert!(sniff("txt", b"hello\nworld"));
        assert!(sniff("txt", "\u{feff}héllo".as_bytes()));
        assert!(sniff("txt", b""));
        assert!(!sniff("txt", b"MZ\x90\0\x03\0"));
        assert!(!sniff("txt", b"  <!DOCTYPE html><html>"));
        assert!(!sniff("txt", b"<svg onload=x>"));
        assert!(!sniff("txt", b"\xff\xfe\x00h"));
        // a character cut by the 8 KiB window
        let mut long = "a".repeat(8191).into_bytes();
        long.extend_from_slice("é".as_bytes());
        assert!(sniff("txt", &long));
    }
}
