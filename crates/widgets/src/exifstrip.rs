//! "Location is removed from photos you send" (docs/PHOTO-METADATA.md).
//!
//! Every image the app uploads (chat attach / paste / drag-drop, Memories "Add photos", the profile and room photos)
//! goes through [`strip_image_metadata`] first: GPS and every other identifying tag (camera serial, owner name, maker
//! notes, XMP, IPTC, comments, secondary pictures) is removed WITHOUT re-encoding the picture; only the EXIF Orientation
//! survives, so a phone photo doesn't turn sideways. A line-for-line port of Tin Hat Web's `src/core/exifstrip.ts`; both
//! replay `testvectors/exif-strip/` (a byte-identical copy in both repos, made by the independent reference
//! `scripts/exif-strip-oracle.py`). Pure: no Qt, no I/O.
//!
//! - JPEG: keep SOI, the JFIF APP0 (header only, no thumbnail), APP2 "ICC_PROFILE", APP14 "Adobe" (its fields only),
//!   every non-APP marker and the scans; drop the other APPn and COM; stop at the first EOI. A truncated last scan is
//!   kept as it is.
//! - PNG: keep the chunks that draw the picture ([`PNG_KEEP`]; iCCP's name replaced, IEND empty), drop the rest; stop
//!   after IEND.
//! - WebP: drop EXIF, "XMP " and unknown chunks (also inside animation frames); fix the VP8X flags and the RIFF size.
//! - HEIF (HEIC/HEIF/AVIF): overwrite every metadata item (Exif, mime, uri) in place (same length, same offsets) and
//!   clear every top-level box but ftyp/the first meta/mdat/moov (Samsung sefd, uuid, a second meta, …), the main
//!   meta's xml boxes and moov's user data; refused when such an item has no location, lies outside the media data or
//!   overlaps the picture, or when any item's extent runs past its end.
//! - other: unchanged (GIF, BMP, SVG, anything else).
//!
//! Orientation (1..8) is written back as the only tag of a minimal big-endian TIFF. A structure the rule can't walk is
//! an [`StripError`]: the caller does NOT upload the file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripError(pub String);

impl fmt::Display for StripError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StripError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripFormat {
    Jpeg,
    Png,
    Webp,
    Heif,
    Other,
}

impl StripFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            StripFormat::Jpeg => "jpeg",
            StripFormat::Png => "png",
            StripFormat::Webp => "webp",
            StripFormat::Heif => "heif",
            StripFormat::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stripped {
    pub format: StripFormat,
    pub bytes: Vec<u8>,
    /// The source's EXIF orientation, 1..8 (1 = none).
    pub orientation: u16,
}

type R<T> = Result<T, StripError>;

fn fail<T>(m: impl Into<String>) -> R<T> {
    Err(StripError(m.into()))
}

fn u16be(d: &[u8], p: usize) -> u32 {
    (u32::from(d[p]) << 8) | u32::from(d[p + 1])
}
fn u32be(d: &[u8], p: usize) -> u64 {
    u64::from(u32::from_be_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]))
}
fn u32le(d: &[u8], p: usize) -> u64 {
    u64::from(u32::from_le_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]))
}

fn starts_with(d: &[u8], p: usize, s: &[u8]) -> bool {
    p.checked_add(s.len()).is_some_and(|e| e <= d.len()) && &d[p..p + s.len()] == s
}

// ---- TIFF / EXIF ------------------------------------------------------------------------------------------------

/// IFD0 Orientation of a TIFF block (from its byte-order mark); 1 when missing or unreadable.
pub fn orientation_from_tiff(t: &[u8]) -> u16 {
    if t.len() < 8 {
        return 1;
    }
    let le = if t[0..4] == *b"II*\0" {
        true
    } else if t[0..4] == *b"MM\0*" {
        false
    } else {
        return 1;
    };
    let r16 = |p: usize| -> u32 { if le { u32::from(t[p]) | (u32::from(t[p + 1]) << 8) } else { u16be(t, p) } };
    let r32 = |p: usize| -> u64 { if le { u32le(t, p) } else { u32be(t, p) } };
    let ifd = r32(4);
    if ifd + 2 > t.len() as u64 {
        return 1;
    }
    let ifd = ifd as usize;
    let n = r16(ifd) as usize;
    for i in 0..n {
        let p = ifd + 2 + 12 * i;
        if p + 12 > t.len() {
            return 1;
        }
        if r16(p) == 0x0112 {
            if r16(p + 2) != 3 || r32(p + 4) < 1 {
                return 1;
            }
            let v = r16(p + 8);
            return if (1..=8).contains(&v) { v as u16 } else { 1 };
        }
    }
    1
}

fn without_exif_prefix(d: &[u8]) -> &[u8] {
    if starts_with(d, 0, b"Exif\0\0") {
        &d[6..]
    } else {
        d
    }
}

/// Orientation as the only tag; 1 → an empty IFD0 (only HEIF writes that one).
pub fn minimal_tiff(orientation: u16) -> Vec<u8> {
    if orientation == 1 {
        return vec![0x4d, 0x4d, 0, 0x2a, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0];
    }
    let o = orientation as u8;
    vec![0x4d, 0x4d, 0, 0x2a, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, o, 0, 0, 0, 0, 0, 0]
}

// ---- JPEG -------------------------------------------------------------------------------------------------------

fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut s = Vec::with_capacity(payload.len() + 4);
    s.extend_from_slice(&[0xFF, marker]);
    s.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    s.extend_from_slice(payload);
    s
}

/// The segment to keep for an APPn/COM (rebuilt to its needed fields), or None to drop it. Other markers: kept.
fn jpeg_segment(marker: u8, payload: &[u8]) -> Option<Vec<u8>> {
    match marker {
        // JFIF only, its 14-byte header with the thumbnail size set to 0 (drops a JFIF thumbnail); JFXX and the rest go
        0xE0 => (payload.starts_with(b"JFIF\0") && payload.len() >= 14).then(|| {
            let mut p = payload[..12].to_vec();
            p.extend_from_slice(&[0, 0]);
            segment(0xE0, &p)
        }),
        0xE2 => payload.starts_with(b"ICC_PROFILE\0").then(|| segment(0xE2, payload)),
        // Adobe: its 12 bytes of fields only ("Adobe", version, flags0, flags1, colour transform)
        0xEE => (payload.starts_with(b"Adobe") && payload.len() >= 12).then(|| segment(0xEE, &payload[..12])),
        0xE0..=0xEF | 0xFE => None,
        _ => Some(segment(marker, payload)),
    }
}

fn strip_jpeg(d: &[u8]) -> R<(Vec<u8>, u16)> {
    let n = d.len();
    let mut head: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut tail: Vec<u8> = Vec::new();
    let mut orientation: Option<u16> = None;
    let mut i = 2;
    let mut in_head = true;
    while i < n {
        if d[i] != 0xFF {
            return fail(format!("jpeg: expected a marker at {i}"));
        }
        while i < n && d[i] == 0xFF {
            i += 1;
        }
        if i >= n {
            return fail("jpeg: ends inside a marker");
        }
        let m = d[i];
        i += 1;
        if m == 0x00 {
            return fail("jpeg: stray 0xFF00");
        }
        if m == 0xD9 {
            tail.extend_from_slice(&[0xFF, 0xD9]);
            break;
        }
        if m == 0x01 || (0xD0..=0xD7).contains(&m) {
            if in_head {
                head.push((m, vec![0xFF, m]));
            } else {
                tail.extend_from_slice(&[0xFF, m]);
            }
            continue;
        }
        if m == 0xD8 {
            return fail("jpeg: second SOI");
        }
        if i + 2 > n {
            return fail("jpeg: ends inside a segment length");
        }
        let ln = u16be(d, i) as usize;
        if ln < 2 || i + ln > n {
            return fail("jpeg: segment length past the end");
        }
        let payload = &d[i + 2..i + ln];
        i += ln;
        if m == 0xE1 && in_head && orientation.is_none() && payload.starts_with(b"Exif\0\0") {
            orientation = Some(orientation_from_tiff(&payload[6..]));
        }
        if let Some(seg) = jpeg_segment(m, payload) {
            if in_head {
                head.push((m, seg));
            } else {
                tail.extend_from_slice(&seg);
            }
        }
        if m == 0xDA {
            in_head = false;
            // entropy-coded data: up to the next marker that isn't a stuffed byte, a restart or fill
            let mut j = i;
            while j < n {
                if d[j] == 0xFF && j + 1 < n {
                    let b = d[j + 1];
                    if b == 0x00 || (0xD0..=0xD7).contains(&b) {
                        j += 2;
                        continue;
                    }
                    if b == 0xFF {
                        j += 1;
                        continue;
                    }
                    break;
                }
                j += 1;
            }
            tail.extend_from_slice(&d[i..j]);
            i = j;
            if j >= n {
                break; // truncated scan: kept as it is
            }
        }
    }
    let o = orientation.unwrap_or(1);
    let mut out = Vec::with_capacity(n);
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut k = 0;
    while k < head.len() && head[k].0 == 0xE0 {
        out.extend_from_slice(&head[k].1);
        k += 1;
    }
    if o != 1 {
        let mut p = b"Exif\0\0".to_vec();
        p.extend_from_slice(&minimal_tiff(o));
        out.extend_from_slice(&segment(0xE1, &p));
    }
    for (_, seg) in &head[k..] {
        out.extend_from_slice(seg);
    }
    out.extend_from_slice(&tail);
    Ok((out, o))
}

// ---- PNG --------------------------------------------------------------------------------------------------------

const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
/// The chunks that draw the picture; everything else (eXIf, tEXt, zTXt, iTXt = XMP, tIME, sPLT, private chunks) goes.
pub const PNG_KEEP: [&[u8; 4]; 19] = [
    b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"cHRM", b"gAMA", b"iCCP", b"sBIT", b"sRGB", b"cICP", b"mDCV", b"cLLI",
    b"bKGD", b"hIST", b"pHYs", b"acTL", b"fcTL", b"fdAT",
];
const PNG_IEND: [u8; 12] = [0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82];
/// iCCP's free-text profile name is replaced by this one (the profile itself stays: it is the picture's colours).
const ICCP_NAME: &[u8] = b"ICC profile";

struct Chunk {
    ty: [u8; 4],
    start: usize,
    end: usize,
    data_at: usize,
    data_end: usize,
}

fn png_chunks(d: &[u8]) -> R<Vec<Chunk>> {
    let mut out = Vec::new();
    let mut i = 8;
    let n = d.len();
    while i < n {
        if i + 8 > n {
            return fail("png: ends inside a chunk header");
        }
        let ln = u32be(d, i) as usize;
        let ty = [d[i + 4], d[i + 5], d[i + 6], d[i + 7]];
        if ln > 0x7FFF_FFFF || i + 12 + ln > n {
            return fail("png: chunk length past the end");
        }
        out.push(Chunk { ty, start: i, end: i + 12 + ln, data_at: i + 8, data_end: i + 8 + ln });
        i += 12 + ln;
        if &ty == b"IEND" {
            break;
        }
    }
    Ok(out)
}

pub fn crc32(b: &[u8]) -> u32 {
    let mut c: u32 = 0xFFFF_FFFF;
    for &x in b {
        c ^= u32::from(x);
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    c ^ 0xFFFF_FFFF
}

fn png_chunk(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut typed = ty.to_vec();
    typed.extend_from_slice(data);
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(&typed);
    c.extend_from_slice(&crc32(&typed).to_be_bytes());
    c
}

fn strip_png(d: &[u8]) -> R<(Vec<u8>, u16)> {
    let chunks = png_chunks(d)?;
    let mut o = 1;
    for c in &chunks {
        if &c.ty == b"IDAT" {
            break;
        }
        if &c.ty == b"eXIf" {
            o = orientation_from_tiff(without_exif_prefix(&d[c.data_at..c.data_end]));
            break;
        }
    }
    let mut out = Vec::with_capacity(d.len());
    out.extend_from_slice(&PNG_SIG);
    let mut wrote = false;
    for c in &chunks {
        if &c.ty == b"IDAT" && !wrote {
            wrote = true;
            if o != 1 {
                out.extend_from_slice(&png_chunk(b"eXIf", &minimal_tiff(o)));
            }
        }
        if &c.ty == b"IEND" {
            out.extend_from_slice(&PNG_IEND); // an IEND carries nothing
        } else if &c.ty == b"iCCP" {
            let Some(z) = d[c.data_at..c.data_end].iter().position(|&x| x == 0) else { return fail("png: iCCP without a name") };
            let mut data = ICCP_NAME.to_vec();
            data.extend_from_slice(&d[c.data_at + z..c.data_end]);
            out.extend_from_slice(&png_chunk(b"iCCP", &data));
        } else if PNG_KEEP.iter().any(|k| **k == c.ty) {
            out.extend_from_slice(&d[c.start..c.end]);
        }
    }
    Ok((out, o))
}

// ---- WebP -------------------------------------------------------------------------------------------------------

const WEBP_KEEP: [&[u8; 4]; 7] = [b"VP8X", b"ICCP", b"ANIM", b"ANMF", b"ALPH", b"VP8 ", b"VP8L"];
/// Inside an animation frame: only the picture's own chunks.
const ANMF_KEEP: [&[u8; 4]; 3] = [b"ALPH", b"VP8 ", b"VP8L"];

fn riff_chunks(d: &[u8], start: usize, end: usize, what: &str) -> R<Vec<Chunk>> {
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        if i + 8 > end {
            return fail(format!("webp: ends inside a {what} header"));
        }
        let ty = [d[i], d[i + 1], d[i + 2], d[i + 3]];
        let ln = u32le(d, i + 4) as usize;
        if i + 8 + ln > end {
            return fail(format!("webp: {what} length past the end"));
        }
        let padded = ln + (ln & 1);
        out.push(Chunk { ty, start: i, end: (i + 8 + padded).min(end), data_at: i + 8, data_end: i + 8 + ln });
        i += 8 + padded;
    }
    Ok(out)
}

fn strip_webp(d: &[u8]) -> R<(Vec<u8>, u16)> {
    if d.len() < 12 {
        return fail("webp: too short");
    }
    let riff = u32le(d, 4) as usize;
    let end = 8 + riff;
    if riff < 4 || end > d.len() {
        return fail("webp: RIFF size past the end");
    }
    let chunks = riff_chunks(d, 12, end, "chunk")?;
    let kept = |ty: &[u8; 4], set: &[&[u8; 4]]| set.iter().any(|k| *k == ty);
    // an animation frame: 16 bytes of frame header, then its own sub-chunks (only the picture ones are kept)
    let mut frames: Vec<Option<Vec<u8>>> = vec![None; chunks.len()];
    for (k, c) in chunks.iter().enumerate() {
        if &c.ty != b"ANMF" {
            continue;
        }
        if c.data_end - c.data_at < 16 {
            return fail("webp: short ANMF");
        }
        let subs = riff_chunks(d, c.data_at + 16, c.data_end, "frame chunk")?;
        if subs.iter().all(|s| kept(&s.ty, &ANMF_KEEP)) {
            continue;
        }
        let mut f = d[c.data_at..c.data_at + 16].to_vec();
        for s in subs.iter().filter(|s| kept(&s.ty, &ANMF_KEEP)) {
            f.extend_from_slice(&d[s.start..s.end]);
            if (s.end - s.start) & 1 == 1 {
                f.push(0);
            }
        }
        frames[k] = Some(f);
    }
    if frames.iter().all(Option::is_none) && chunks.iter().all(|c| kept(&c.ty, &WEBP_KEEP)) {
        return Ok((d[..end].to_vec(), 1)); // nothing to remove (anything after the RIFF goes)
    }
    let mut o = 1;
    if let Some(c) = chunks.iter().find(|c| &c.ty == b"EXIF") {
        o = orientation_from_tiff(without_exif_prefix(&d[c.data_at..c.data_end]));
    }
    let mut body = Vec::with_capacity(d.len());
    for (k, c) in chunks.iter().enumerate() {
        if !kept(&c.ty, &WEBP_KEEP) {
            continue;
        }
        let start = body.len();
        if let Some(f) = &frames[k] {
            body.extend_from_slice(b"ANMF");
            body.extend_from_slice(&(f.len() as u32).to_le_bytes());
            body.extend_from_slice(f);
        } else {
            body.extend_from_slice(&d[c.start..c.end]);
            if &c.ty == b"VP8X" {
                if c.data_end - c.data_at < 10 {
                    return fail("webp: short VP8X");
                }
                let mut flags = d[c.data_at] & !0x0C;
                if o != 1 {
                    flags |= 0x08;
                }
                body[start + 8] = flags;
            }
        }
        if (body.len() - start) & 1 == 1 {
            body.push(0);
        }
    }
    if o != 1 {
        if !chunks.iter().any(|c| &c.ty == b"VP8X") {
            return fail("webp: orientation without VP8X");
        }
        let t = minimal_tiff(o);
        body.extend_from_slice(b"EXIF");
        body.extend_from_slice(&(t.len() as u32).to_le_bytes());
        body.extend_from_slice(&t);
    }
    let mut out = Vec::with_capacity(12 + body.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    Ok((out, o))
}

// ---- HEIF (ISOBMFF) ---------------------------------------------------------------------------------------------

const HEIF_BRANDS: [&[u8; 4]; 11] = [b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1", b"avif", b"avis", b"mif2"];
/// Metadata items: never needed to draw the picture.
const HEIF_BLANK: [&[u8; 4]; 3] = [b"Exif", b"mime", b"uri "];
/// Any other top-level box (Samsung sefd, uuid, free, …) is cleared: a zero-filled "free" box of the same size.
const HEIF_TOP_KEEP: [&[u8; 4]; 4] = [b"ftyp", b"meta", b"mdat", b"moov"];
/// Inside moov and its traks: QuickTime user data (GPS, author): cleared.
const HEIF_USER_DATA: [&[u8; 4]; 2] = [b"udta", b"meta"];
/// XML metadata as a box of the main meta: cleared.
const HEIF_META_XML: [&[u8; 4]; 2] = [b"xml ", b"bxml"];

#[derive(Clone, Copy)]
struct BoxAt {
    ty: [u8; 4],
    start: usize,
    content: usize,
    end: usize,
}

fn box_at(d: &[u8], i: usize, end: usize) -> R<BoxAt> {
    if i + 8 > end {
        return fail("heif: ends inside a box header");
    }
    let mut size = u32be(d, i);
    let ty = [d[i + 4], d[i + 5], d[i + 6], d[i + 7]];
    let mut hdr = 8u64;
    if size == 1 {
        if i + 16 > end {
            return fail("heif: ends inside a large size");
        }
        let hi = u32be(d, i + 8);
        if hi > 0x1F_FFFF {
            return fail("heif: box size past the end");
        }
        size = hi * 0x1_0000_0000 + u32be(d, i + 12);
        hdr = 16;
    } else if size == 0 {
        size = (end - i) as u64;
    }
    if size < hdr || i as u64 + size > end as u64 {
        return fail("heif: box size past the end");
    }
    Ok(BoxAt { ty, start: i, content: i + hdr as usize, end: i + size as usize })
}

fn boxes(d: &[u8], start: usize, end: usize) -> R<Vec<BoxAt>> {
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        let b = box_at(d, i, end)?;
        out.push(b);
        i = b.end;
    }
    Ok(out)
}

pub fn is_heif(d: &[u8]) -> bool {
    if d.len() < 16 || !starts_with(d, 4, b"ftyp") {
        return false;
    }
    let mut size = u32be(d, 0);
    let mut hdr = 8usize;
    if size == 1 {
        let hi = u32be(d, 8);
        if hi > 0x1F_FFFF {
            return false;
        }
        size = hi * 0x1_0000_0000 + u32be(d, 12);
        hdr = 16;
    }
    if size < hdr as u64 + 8 || size > d.len() as u64 {
        return false;
    }
    let size = size as usize;
    let brand = |p: usize| HEIF_BRANDS.iter().any(|b| **b == d[p..p + 4]);
    if brand(hdr) {
        return true;
    }
    let mut k = hdr + 8;
    while k + 4 <= size {
        if brand(k) {
            return true;
        }
        k += 4;
    }
    false
}

fn uint(d: &[u8], p: usize, size: usize, end: usize) -> R<(u64, usize)> {
    if size == 0 {
        return Ok((0, p));
    }
    if p + size > end {
        return fail("heif: ends inside a field");
    }
    let mut v: u64 = 0;
    for k in 0..size {
        v = v.checked_mul(256).ok_or_else(|| StripError("heif: field too large".into()))? + u64::from(d[p + k]);
    }
    Ok((v, p + size))
}

fn strip_heif(d: &[u8]) -> R<(Vec<u8>, u16)> {
    // top level: every box must parse (unreadable bytes could be anything); meta, the mdats, and the boxes to clear
    let mut meta: Option<BoxAt> = None;
    let mut mdats: Vec<BoxAt> = Vec::new();
    let mut cleared: Vec<BoxAt> = Vec::new();
    for b in boxes(d, 0, d.len())? {
        if &b.ty == b"meta" && meta.is_none() {
            meta = Some(b);
        } else if &b.ty == b"mdat" {
            mdats.push(b);
        } else if &b.ty == b"moov" {
            for b2 in boxes(d, b.content, b.end)? {
                if HEIF_USER_DATA.iter().any(|k| **k == b2.ty) {
                    cleared.push(b2);
                } else if &b2.ty == b"trak" {
                    for b3 in boxes(d, b2.content, b2.end)? {
                        if HEIF_USER_DATA.iter().any(|k| **k == b3.ty) {
                            cleared.push(b3);
                        }
                    }
                }
            }
        } else if !HEIF_TOP_KEEP.iter().any(|k| **k == b.ty) || &b.ty == b"meta" {
            cleared.push(b); // incl. a second meta
        }
    }
    let Some(meta) = meta else { return fail("heif: no meta box") };
    if meta.content + 4 > meta.end {
        return fail("heif: short meta");
    }
    let (mut iinf, mut iloc, mut idat) = (None, None, None);
    for b in boxes(d, meta.content + 4, meta.end)? {
        match &b.ty {
            b"iinf" if iinf.is_none() => iinf = Some(b),
            b"iloc" if iloc.is_none() => iloc = Some(b),
            b"idat" if idat.is_none() => idat = Some(b),
            t if HEIF_META_XML.iter().any(|k| *k == t) => cleared.push(b),
            _ => {}
        }
    }
    let (Some(iinf), Some(iloc)) = (iinf, iloc) else { return fail("heif: no iinf/iloc") };
    if iinf.content + 4 > iinf.end || iloc.content + 4 > iloc.end {
        return fail("heif: short iinf/iloc");
    }
    // the items to blank: Exif (its orientation is written back), XMP and any other mime / uri item
    let mut targets: HashMap<u64, [u8; 4]> = HashMap::new();
    let mut seen: HashSet<u64> = HashSet::new();
    let v = d[iinf.content];
    let p0 = iinf.content + 4 + if v == 0 { 2 } else { 4 };
    if p0 > iinf.end {
        return fail("heif: short iinf");
    }
    for b in boxes(d, p0, iinf.end)? {
        if &b.ty != b"infe" || b.content + 4 > b.end {
            continue;
        }
        let iv = d[b.content];
        if iv < 2 {
            continue;
        }
        let (item_id, q) = uint(d, b.content + 4, if iv == 2 { 2 } else { 4 }, b.end)?;
        if !seen.insert(item_id) {
            return fail("heif: duplicate item id");
        }
        let q = q + 2; // protection index
        if q + 4 > b.end {
            return fail("heif: short infe");
        }
        let itype = [d[q], d[q + 1], d[q + 2], d[q + 3]];
        if HEIF_BLANK.iter().any(|t| **t == itype) {
            targets.insert(item_id, itype);
        }
    }
    let kind_of = |id: u64| targets.get(&id).copied();
    // every item's location
    let lv = d[iloc.content];
    let end = iloc.end;
    let mut p = iloc.content + 4;
    if p + 2 > end {
        return fail("heif: short iloc");
    }
    let offset_size = (d[p] >> 4) as usize;
    let length_size = (d[p] & 15) as usize;
    let base_size = (d[p + 1] >> 4) as usize;
    let index_size = if lv == 1 || lv == 2 { (d[p + 1] & 15) as usize } else { 0 };
    p += 2;
    let ok = |x: usize| x == 0 || x == 4 || x == 8;
    if lv > 2 || !ok(offset_size) || !ok(length_size) || !ok(base_size) || !ok(index_size) {
        return fail("heif: iloc sizes");
    }
    let fieldless = offset_size == 0 && length_size == 0 && index_size == 0; // an extent costs no bytes: at most one each
    let (count, np) = uint(d, p, if lv < 2 { 2 } else { 4 }, end)?;
    p = np;
    let mut located: HashSet<u64> = HashSet::new();
    let mut target_ranges: Vec<(u64, u64, u64)> = Vec::new(); // (start, end, item id)
    let mut other_ranges: Vec<(u64, u64)> = Vec::new();
    for _ in 0..count {
        let (item_id, np) = uint(d, p, if lv < 2 { 2 } else { 4 }, end)?;
        p = np;
        let mut method = 0;
        if lv == 1 || lv == 2 {
            let (m, np) = uint(d, p, 2, end)?;
            method = m & 15;
            p = np;
        }
        let (dref, np) = uint(d, p, 2, end)?;
        let (base, np) = uint(d, np, base_size, end)?;
        let (nex, np) = uint(d, np, 2, end)?;
        p = np;
        if fieldless && nex > 1 {
            return fail("heif: iloc extents without fields");
        }
        let mut extents = Vec::new();
        for _ in 0..nex {
            let (_, np) = uint(d, p, index_size, end)?;
            let (off, np) = uint(d, np, offset_size, end)?;
            let (ln, np) = uint(d, np, length_size, end)?;
            p = np;
            extents.push((off, ln));
        }
        let kind = kind_of(item_id);
        if kind.is_some() && !located.insert(item_id) {
            return fail("heif: an item located twice");
        }
        if dref != 0 {
            continue; // in another file: nothing of it is here
        }
        let (lo, hi) = match (method, &idat, kind) {
            (0, _, _) => (0u64, d.len() as u64),
            (1, Some(b), _) => (b.content as u64, b.end as u64),
            (1, None, _) => return fail("heif: idat item without idat"),
            (_, _, None) => continue, // made from other items (their data is located on its own)
            _ => return fail("heif: metadata item by reference"),
        };
        let mut rs = Vec::new();
        for (off, ln) in extents {
            let a = lo.checked_add(base).and_then(|x| x.checked_add(off));
            let b = match (a, ln) {
                (Some(_), 0) => Some(hi),
                (Some(a), _) => a.checked_add(ln),
                _ => None,
            };
            match (a, b) {
                (Some(a), Some(b)) if a >= lo && b <= hi && a <= b => rs.push((a, b)),
                _ => return fail("heif: item extent past the end"),
            }
        }
        if kind.is_none() {
            other_ranges.extend(rs);
            continue;
        }
        for (a, b) in rs {
            // the mdats are in file order: sorted, disjoint; the last one starting at or before a
            let k = mdats.partition_point(|m| m.content as u64 <= a);
            if method == 0 && !(k > 0 && b <= mdats[k - 1].end as u64) {
                return fail("heif: metadata outside the media data");
            }
            target_ranges.push((a, b, item_id));
        }
    }
    if targets.keys().any(|id| !located.contains(id)) {
        return fail("heif: a metadata item without a location");
    }
    // never write over another item or over a metadata extent twice (sorted sweeps: no quadratic work)
    let mut sorted: Vec<(u64, u64)> = target_ranges.iter().filter(|r| r.1 > r.0).map(|r| (r.0, r.1)).collect();
    sorted.sort_unstable();
    if sorted.windows(2).any(|w| w[1].0 < w[0].1) {
        return fail("heif: overlapping metadata extents");
    }
    // the type and the content are rewritten; disjoint
    let mut cl: Vec<(u64, u64)> = cleared.iter().map(|b| (b.start as u64 + 4, b.end as u64)).collect();
    cl.sort_unstable();
    for &(c, e) in &other_ranges {
        if e <= c {
            continue;
        }
        let k = sorted.partition_point(|r| r.1 <= c);
        if k < sorted.len() && sorted[k].0 < e {
            return fail("heif: metadata overlaps the picture");
        }
        let k = cl.partition_point(|r| r.1 <= c);
        if k < cl.len() && cl[k].0 < e {
            return fail("heif: picture data in a box that is cleared");
        }
    }
    let mut out = d.to_vec();
    for b in &cleared {
        out[b.start + 4..b.start + 8].copy_from_slice(b"free");
        out[b.content..b.end].fill(0);
    }
    let mut o = 1;
    let mut by_id: BTreeMap<u64, Vec<(usize, usize)>> = BTreeMap::new();
    for &(a, b, id) in &target_ranges {
        by_id.entry(id).or_default().push((a as usize, b as usize));
    }
    for (id, ranges) in by_id {
        let total: usize = ranges.iter().map(|(a, b)| b - a).sum();
        let mut fill = vec![0u8; total];
        if kind_of(id) == Some(*b"Exif") {
            let mut data = Vec::with_capacity(total);
            for &(a, b) in &ranges {
                data.extend_from_slice(&d[a..b]);
            }
            if data.len() >= 4 {
                let hoff = u32be(&data, 0);
                if 4 + hoff <= data.len() as u64 {
                    let t = &data[4 + hoff as usize..];
                    o = orientation_from_tiff(if hoff == 0 { without_exif_prefix(t) } else { t });
                }
            }
            let tf = minimal_tiff(o);
            if 4 + tf.len() <= total {
                fill[4..4 + tf.len()].copy_from_slice(&tf); // the 4-byte header offset stays 0
            }
        }
        let mut k = 0;
        for (a, b) in ranges {
            out[a..b].copy_from_slice(&fill[k..k + (b - a)]);
            k += b - a;
        }
    }
    Ok((out, o))
}

// ---- sniff (12.1.16 2-2) ----------------------------------------------------------------------------------------

/// How many bytes the sniff reads (the web reads only this much before deciding).
pub const SNIFF_HEAD: usize = 65536;
/// The formats the cleaner cleans. Outside the list: GIF (passes through, the documented v1 exception) and TIFF (the
/// upload refuses it: launch-gate 13:55).
pub const CLEANED_FORMATS: [&str; 4] = ["jpeg", "png", "webp", "heif"];

/// A HEIF picture by its ftyp's MAJOR brand, read from the head whatever the box's size (a 70 KB ftyp is still a
/// HEIC; a video whose ftyp only lists a HEIF brand among its compatible ones is not a picture).
fn sniff_heif(d: &[u8]) -> bool {
    if d.len() < 12 || !starts_with(d, 4, b"ftyp") {
        return false;
    }
    let hdr = if u32be(d, 0) == 1 { 16 } else { 8 };
    d.len() >= hdr + 4 && HEIF_BRANDS.iter().any(|b| **b == d[hdr..hdr + 4])
}

/// What the bytes are, from their first [`SNIFF_HEAD`] bytes, never the file's type or name: jpeg | png | webp |
/// heif | gif | tiff | raw, or None = not a picture. tiff = classic TIFF (TIFF, DNG, CR2, NEF, ARW, …) and BigTIFF;
/// raw = camera RAWs with their own signature (ORF, RW2/RWL, RAF, CR3): the upload refuses both. The web's `sniffImage`.
pub fn sniff_image(head: &[u8]) -> Option<&'static str> {
    let d = &head[..head.len().min(SNIFF_HEAD)];
    if d.len() >= 3 && d[0] == 0xFF && d[1] == 0xD8 && d[2] == 0xFF {
        Some("jpeg")
    } else if d.len() >= 8 && d[..8] == PNG_SIG {
        Some("png")
    } else if starts_with(d, 0, b"RIFF") && starts_with(d, 8, b"WEBP") {
        Some("webp")
    } else if sniff_heif(d) {
        Some("heif")
    } else if starts_with(d, 0, b"GIF87a") || starts_with(d, 0, b"GIF89a") {
        Some("gif")
    } else if [b"II*\0", b"MM\0*", b"II+\0", b"MM\0+"].iter().any(|m| starts_with(d, 0, *m)) {
        Some("tiff")
    } else if [&b"IIRO"[..], b"IIRS", b"MMOR", b"IIU\0", b"FUJIFILMCCD-RAW"].iter().any(|m| starts_with(d, 0, m))
        || (starts_with(d, 4, b"ftyp") && starts_with(d, 8, b"crx "))
    {
        Some("raw")
    } else {
        None
    }
}

/// Whether an upload of these bytes goes through the cleaner (12.1.16 2-2: the bytes decide).
pub fn is_cleaned_picture(d: &[u8]) -> bool {
    sniff_image(d).is_some_and(|f| CLEANED_FORMATS.contains(&f))
}

// ---- entry ------------------------------------------------------------------------------------------------------

/// Strip the identifying metadata of an image's bytes (see the module docs). An error means the structure couldn't be
/// walked: the caller must not upload the original then.
pub fn strip_image_metadata(d: &[u8]) -> Result<Stripped, StripError> {
    let (format, (bytes, orientation)) = if d.len() >= 3 && d[0] == 0xFF && d[1] == 0xD8 && d[2] == 0xFF {
        (StripFormat::Jpeg, strip_jpeg(d)?)
    } else if d.len() >= 8 && d[..8] == PNG_SIG {
        (StripFormat::Png, strip_png(d)?)
    } else if starts_with(d, 0, b"RIFF") && starts_with(d, 8, b"WEBP") {
        (StripFormat::Webp, strip_webp(d)?)
    } else if is_heif(d) {
        (StripFormat::Heif, strip_heif(d)?)
    } else {
        (StripFormat::Other, (d.to_vec(), 1))
    };
    Ok(Stripped { format, bytes, orientation })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testvectors/exif-strip");

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
    }

    /// The shared vectors: every input → the expected output byte for byte (or an error), the format and the
    /// orientation; no output carries a secret or the GPS seconds.
    #[test]
    fn shared_vectors() {
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/vectors.json")).unwrap()).unwrap();
        let secrets: Vec<Vec<u8>> = doc["secrets"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().chars().map(|c| c as u8).collect()).collect();
        let gps: Vec<Vec<u8>> = doc["gpsRationals"].as_array().unwrap().iter().map(|s| hex(s.as_str().unwrap())).collect();
        let cases = doc["cases"].as_array().unwrap();
        assert!(cases.len() >= 20, "the vectors are all there");
        for c in cases {
            let name = c["name"].as_str().unwrap();
            let input = std::fs::read(format!("{DIR}/in/{name}")).unwrap();
            let got = strip_image_metadata(&input);
            if c["error"].as_bool().unwrap() {
                assert!(got.is_err(), "{name}: expected an error");
                continue;
            }
            let got = got.unwrap_or_else(|e| panic!("{name}: {e}"));
            let want = std::fs::read(format!("{DIR}/out/{name}")).unwrap();
            assert_eq!(got.format.as_str(), c["format"].as_str().unwrap(), "{name}: format");
            assert_eq!(u64::from(got.orientation), c["orientation"].as_u64().unwrap(), "{name}: orientation");
            assert!(got.bytes == want, "{name}: bytes differ from the reference");
            if got.format != StripFormat::Other {
                for s in secrets.iter().chain(gps.iter()) {
                    assert!(!contains(&got.bytes, s), "{name}: still carries {:?}", String::from_utf8_lossy(s));
                }
            }
        }
    }

    /// Every photo input carries GPS (so the check above can fail: the planted control).
    #[test]
    fn inputs_do_carry_gps() {
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/vectors.json")).unwrap()).unwrap();
        let gps: Vec<Vec<u8>> = doc["gpsRationals"].as_array().unwrap().iter().map(|s| hex(s.as_str().unwrap())).collect();
        let mut with_gps = 0;
        for c in doc["cases"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let input = std::fs::read(format!("{DIR}/in/{name}")).unwrap();
            if gps.iter().any(|g| contains(&input, g)) {
                with_gps += 1;
            }
        }
        assert!(with_gps >= 15, "only {with_gps} inputs carry GPS");
    }

    #[test]
    fn orientation_survives_as_the_only_tag() {
        for o in 1..=8u16 {
            assert_eq!(orientation_from_tiff(&minimal_tiff(o)), o);
        }
        assert_eq!(orientation_from_tiff(b"XX*\0garbage"), 1);
        assert_eq!(orientation_from_tiff(&[]), 1);
    }

    /// 12.1.16 2-2: the sniff of every upload-rule case is the reference's.
    #[test]
    fn sniff_matches_the_reference() {
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/upload-rule.json")).unwrap()).unwrap();
        assert_eq!(doc["sniffHead"].as_u64(), Some(SNIFF_HEAD as u64));
        for c in doc["cases"].as_array().unwrap() {
            let name = c["in"].as_str().unwrap();
            let d = std::fs::read(format!("{DIR}/in/{name}")).unwrap();
            assert_eq!(sniff_image(&d), c["sniff"].as_str(), "{name}");
        }
    }

    #[test]
    fn hostile_structures_are_refused() {
        for name in [
            "heic-amplify.heic",
            "heic-dup-item-id.heic",
            "heic-exif-no-location.heic",
            "heic-exif-overlaps-picture.heic",
            "heic-extent-over-header.heic",
            "heic-zero-field-extents.heic",
            "heic-picture-in-uuid.heic",
            "heic-picture-extent-broken.heic",
        ] {
            assert!(strip_image_metadata(&std::fs::read(format!("{DIR}/in/{name}")).unwrap()).is_err(), "{name}");
        }
    }

    #[test]
    fn crc32_matches_png() {
        // the IEND chunk's well-known CRC
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    /// The differential run against the reference on seeded mutations (not in the default run: the corpus is made on
    /// demand): `python3 scripts/exif-strip-oracle.py fuzz DIR 5000`, then
    /// `EXIF_FUZZ_DIR=DIR cargo test -p tinhat-widgets exifstrip -- --ignored`.
    #[test]
    #[ignore]
    fn differential_against_the_reference() {
        let dir = std::env::var("EXIF_FUZZ_DIR").expect("EXIF_FUZZ_DIR");
        let refs: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/ref.json")).unwrap()).unwrap();
        let mut bad = 0;
        let refs = refs.as_array().unwrap();
        for r in refs {
            let name = r["name"].as_str().unwrap();
            let got = strip_image_metadata(&std::fs::read(format!("{dir}/{name}")).unwrap());
            let same = match (&got, r["error"].as_bool().unwrap()) {
                (Err(_), true) => true,
                (Ok(s), false) => {
                    s.format.as_str() == r["format"].as_str().unwrap()
                        && u64::from(s.orientation) == r["orientation"].as_u64().unwrap()
                        && s.bytes == std::fs::read(format!("{dir}/{name}.out")).unwrap()
                }
                _ => false,
            };
            if !same {
                bad += 1;
                eprintln!("DIFF {name}: {:?}", got.map(|s| (s.format, s.orientation)));
            }
        }
        eprintln!("rust vs the reference: {} case(s), {bad} differ", refs.len());
        assert_eq!(bad, 0);
    }

    #[test]
    fn hostile_lengths_do_not_panic() {
        let mut seeds: Vec<Vec<u8>> = vec![
            vec![0xFF, 0xD8, 0xFF],
            vec![0xFF, 0xD8, 0xFF, 0xE1],
            PNG_SIG.to_vec(),
            b"RIFF\xff\xff\xff\xffWEBP".to_vec(),
            b"\0\0\0\x10ftypheic\0\0\0\0".to_vec(),
            b"\0\0\0\x10ftypheic\0\0\0\0\0\0\0\x01meta\xff\xff\xff\xff\xff\xff\xff\xff".to_vec(),
        ];
        // every prefix of every vector input
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/vectors.json")).unwrap()).unwrap();
        for c in doc["cases"].as_array().unwrap() {
            let input = std::fs::read(format!("{DIR}/in/{}", c["name"].as_str().unwrap())).unwrap();
            for cut in (0..input.len()).step_by(7) {
                seeds.push(input[..cut].to_vec());
            }
        }
        for s in seeds {
            let _ = strip_image_metadata(&s);
        }
    }
}
