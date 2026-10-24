//! animimg — how long an animated GIF / WebP plays (GPU.md §2.6): frame count, one loop's length and the file's own
//! loop count, read from the bytes (no decoding). A line-for-line port of Tin Hat Web's `src/core/animimg.ts`; the
//! tests carry the same case names as `animimg.test.ts`. Pure.
//!
//! ```text
//!   GIF   GIF87a/GIF89a: every image descriptor (0x2C) is a frame; its delay is the preceding Graphic Control
//!         Extension's (0xF9, centiseconds; 0–1 → 100 ms like every browser); the NETSCAPE2.0 / ANIMEXTS1.0 block
//!         carries the loop count (0 = forever; n = n + 1 plays; no block = one play).
//!   WebP  RIFF/WEBP: VP8X flag 0x02 = animation, ANIM = loop count (0 = forever, n = n plays), one ANMF per frame
//!         with a 24-bit duration in ms (0–10 → 100 ms, as Chromium).
//! ```
//! Anything else (JPEG, PNG — APNG is not handled —, a truncated or broken file) → not animated. Reads past the end
//! give 0, like the reference's `undefined` in its bit operations.

use crate::lowgpu::gif_frame_ms;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimKind {
    Gif,
    Webp,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimInfo {
    pub kind: AnimKind,
    /// more than one frame
    pub animated: bool,
    pub frames: u32,
    /// one loop, in ms (0 when unknown)
    pub loop_ms: f64,
    /// total plays: 0 = forever, else the number of times the file plays
    pub plays: u32,
    pub width: u32,
    pub height: u32,
}

const NOT: AnimInfo = AnimInfo { kind: AnimKind::Other, animated: false, frames: 0, loop_ms: 0.0, plays: 0, width: 0, height: 0 };

fn byte(b: &[u8], at: usize) -> u32 {
    b.get(at).copied().unwrap_or(0) as u32
}
fn u16le(b: &[u8], at: usize) -> u32 {
    byte(b, at) | (byte(b, at + 1) << 8)
}
fn u24le(b: &[u8], at: usize) -> u32 {
    byte(b, at) | (byte(b, at + 1) << 8) | (byte(b, at + 2) << 16)
}
fn u32le(b: &[u8], at: usize) -> u64 {
    (byte(b, at) as u64) | ((byte(b, at + 1) as u64) << 8) | ((byte(b, at + 2) as u64) << 16) | ((byte(b, at + 3) as u64) << 24)
}
fn ascii_is(b: &[u8], at: usize, s: &str) -> bool {
    b.get(at..at + s.len()).map(|x| x == s.as_bytes()).unwrap_or(false)
}

/// Is this worth inspecting? (a cheap sniff of the first bytes)
pub fn sniff_kind(b: &[u8]) -> AnimKind {
    if b.len() >= 6 && (ascii_is(b, 0, "GIF89a") || ascii_is(b, 0, "GIF87a")) {
        return AnimKind::Gif;
    }
    if b.len() >= 12 && ascii_is(b, 0, "RIFF") && ascii_is(b, 8, "WEBP") {
        return AnimKind::Webp;
    }
    AnimKind::Other
}

pub fn inspect_animation(b: &[u8]) -> AnimInfo {
    match sniff_kind(b) {
        AnimKind::Gif => inspect_gif(b),
        AnimKind::Webp => inspect_webp(b),
        AnimKind::Other => NOT,
    }
}

fn skip_sub_blocks(b: &[u8], mut at: usize) -> usize {
    while at < b.len() {
        let n = b[at] as usize;
        at += 1;
        if n == 0 {
            return at;
        }
        at += n;
    }
    at
}

fn inspect_gif(b: &[u8]) -> AnimInfo {
    let width = u16le(b, 6);
    let height = u16le(b, 8);
    let packed = byte(b, 10);
    let mut at: usize = 13;
    if packed & 0x80 != 0 {
        at += 3 * (1usize << ((packed & 7) + 1));
    }
    let mut frames: u32 = 0;
    let mut loop_ms: f64 = 0.0;
    let mut delay: u32 = 0;
    let mut lp: Option<u32> = None;
    while at < b.len() {
        let block = b[at];
        if block == 0x3b {
            break; // trailer
        }
        if block == 0x21 {
            let label = byte(b, at + 1);
            if label == 0xf9 && byte(b, at + 2) == 4 {
                delay = u16le(b, at + 4);
                at = skip_sub_blocks(b, at + 2);
            } else if label == 0xff && byte(b, at + 2) == 11 {
                let p = at + 14;
                if (ascii_is(b, at + 3, "NETSCAPE2.0") || ascii_is(b, at + 3, "ANIMEXTS1.0")) && byte(b, p) >= 3 && byte(b, p + 1) == 1 {
                    lp = Some(u16le(b, p + 2));
                }
                at = skip_sub_blocks(b, p);
            } else {
                at = skip_sub_blocks(b, at + 2);
            }
            continue;
        }
        if block == 0x2c {
            let ipacked = byte(b, at + 9);
            let mut p = at + 10;
            if ipacked & 0x80 != 0 {
                p += 3 * (1usize << ((ipacked & 7) + 1));
            }
            p += 1; // LZW minimum code size
            at = skip_sub_blocks(b, p);
            if at > b.len() {
                break; // a frame cut short: not counted
            }
            frames += 1;
            loop_ms += gif_frame_ms(delay) as f64;
            delay = 0;
            continue;
        }
        break; // unknown block: stop reading
    }
    let plays = match lp {
        None => 1,
        Some(0) => 0,
        Some(n) => n + 1,
    };
    AnimInfo { kind: AnimKind::Gif, animated: frames > 1, frames, loop_ms: if frames > 1 { loop_ms } else { 0.0 }, plays, width, height }
}

fn inspect_webp(b: &[u8]) -> AnimInfo {
    let end = (b.len() as u64).min(8 + u32le(b, 4));
    let mut at: u64 = 12;
    let mut flag_anim = false;
    let mut lp: u32 = 0;
    let mut frames: u32 = 0;
    let mut loop_ms: f64 = 0.0;
    let mut width: u32 = 0;
    let mut height: u32 = 0;
    while at + 8 <= end {
        let a = at as usize;
        let size = u32le(b, a + 4);
        let data = at + 8;
        let d = data as usize;
        if ascii_is(b, a, "VP8X") {
            flag_anim = byte(b, d) & 0x02 != 0;
            width = u24le(b, d + 4) + 1;
            height = u24le(b, d + 7) + 1;
        } else if ascii_is(b, a, "ANIM") {
            lp = u16le(b, d + 4);
        } else if ascii_is(b, a, "ANMF") {
            if data + 16 > end {
                break;
            }
            frames += 1;
            let dur = u24le(b, d + 12);
            loop_ms += if dur <= 10 { 100.0 } else { dur as f64 };
        }
        at = data + size + (size & 1);
    }
    let animated = flag_anim && frames > 1;
    AnimInfo { kind: AnimKind::Webp, animated, frames, loop_ms: if animated { loop_ms } else { 0.0 }, plays: lp, width, height }
}

// The web's animimg.test.ts, case for case (same names; synthetic files built byte by byte).
#[cfg(test)]
mod tests {
    use super::*;

    fn le16(v: u32) -> Vec<u8> {
        vec![(v & 255) as u8, ((v >> 8) & 255) as u8]
    }
    fn le24(v: u32) -> Vec<u8> {
        vec![(v & 255) as u8, ((v >> 8) & 255) as u8, ((v >> 16) & 255) as u8]
    }
    fn le32(v: u32) -> Vec<u8> {
        vec![(v & 255) as u8, ((v >> 8) & 255) as u8, ((v >> 16) & 255) as u8, ((v >> 24) & 255) as u8]
    }

    /// A GIF with `delays` (centiseconds) — one 2×2 frame each — and an optional NETSCAPE loop count.
    fn gif(delays: &[u32], lp: Option<u32>, gct: bool, lct: bool, truncate: usize) -> Vec<u8> {
        let mut o: Vec<u8> = b"GIF89a".to_vec();
        o.extend(le16(2));
        o.extend(le16(2));
        o.extend([if gct { 0x80 } else { 0x00 }, 0, 0]);
        if gct {
            o.extend([0, 0, 0, 255, 255, 255]); // 2-colour global table
        }
        if let Some(l) = lp {
            o.extend([0x21, 0xff, 11]);
            o.extend(b"NETSCAPE2.0");
            o.extend([3, 1]);
            o.extend(le16(l));
            o.push(0);
        }
        o.extend([0x21, 0xfe, 3]);
        o.extend(b"hi!");
        o.push(0); // a comment extension (skipped)
        for d in delays {
            o.extend([0x21, 0xf9, 4, 0]);
            o.extend(le16(*d));
            o.extend([0, 0]);
            o.extend([0x2c, 0, 0, 0, 0]);
            o.extend(le16(2));
            o.extend(le16(2));
            o.push(if lct { 0x80 } else { 0 });
            if lct {
                o.extend([10, 20, 30, 40, 50, 60]);
            }
            o.extend([2, 2, 0x4c, 0x01, 0]); // LZW min size 2, one 2-byte sub-block, terminator
        }
        o.push(0x3b);
        o.truncate(o.len() - truncate);
        o
    }

    /// An animated WebP: VP8X (animation flag), ANIM (loop), one ANMF per duration (ms).
    fn webp(durs: &[u32], lp: u32, anim_flag: bool) -> Vec<u8> {
        fn chunk(tag: &str, data: Vec<u8>) -> Vec<u8> {
            let mut c = tag.as_bytes().to_vec();
            c.extend(le32(data.len() as u32));
            let odd = data.len() & 1 == 1;
            c.extend(data);
            if odd {
                c.push(0);
            }
            c
        }
        let mut vp8x = vec![if anim_flag { 0x02 } else { 0 }, 0, 0, 0];
        vp8x.extend(le24(9));
        vp8x.extend(le24(4)); // 10 × 5
        let mut anim = vec![0, 0, 0, 0];
        anim.extend(le16(lp));
        let mut body = b"WEBP".to_vec();
        body.extend(chunk("VP8X", vp8x));
        body.extend(chunk("ANIM", anim));
        for d in durs {
            let mut f = Vec::new();
            for v in [0, 0, 9, 4, *d] {
                f.extend(le24(v));
            }
            f.extend([0, 1, 2, 3]);
            body.extend(chunk("ANMF", f));
        }
        let mut o = b"RIFF".to_vec();
        o.extend(le32(body.len() as u32));
        o.extend(body);
        o
    }

    // GIF
    #[test]
    fn gif_frames_and_length() {
        let a = inspect_animation(&gif(&[10, 20, 30], Some(0), true, false, 0));
        assert!(a.kind == AnimKind::Gif && a.animated && a.frames == 3 && a.loop_ms == 600.0, "{a:?}");
    }
    #[test]
    fn gif_loop_forever() {
        assert_eq!(inspect_animation(&gif(&[10, 20, 30], Some(0), true, false, 0)).plays, 0);
    }
    #[test]
    fn gif_size() {
        let a = inspect_animation(&gif(&[10, 20, 30], Some(0), true, false, 0));
        assert!(a.width == 2 && a.height == 2);
    }
    #[test]
    fn gif_zero_delay_is_100ms() {
        assert_eq!(inspect_animation(&gif(&[0, 1, 5], Some(0), true, false, 0)).loop_ms, 250.0);
    }
    #[test]
    fn gif_loop_count_n_plays_n_plus_1() {
        assert_eq!(inspect_animation(&gif(&[10, 10], Some(2), true, false, 0)).plays, 3);
    }
    #[test]
    fn gif_no_netscape_plays_once() {
        assert_eq!(inspect_animation(&gif(&[10, 10], None, true, false, 0)).plays, 1);
    }
    #[test]
    fn gif_single_frame_not_animated() {
        assert!(!inspect_animation(&gif(&[10], Some(0), true, false, 0)).animated);
    }
    #[test]
    fn gif_local_colour_tables() {
        assert_eq!(inspect_animation(&gif(&[10, 10, 10, 10], Some(0), false, true, 0)).frames, 4);
    }
    #[test]
    fn gif_truncated_counts_whole_frames() {
        let a = inspect_animation(&gif(&[10, 10, 10], Some(0), true, false, 4));
        assert!(a.frames == 2 && a.animated, "{a:?}");
    }
    #[test]
    fn gif87a_sniffed() {
        let mut b = b"GIF87a".to_vec();
        b.extend([0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(sniff_kind(&b), AnimKind::Gif);
    }

    // WebP
    #[test]
    fn webp_frames_and_length() {
        let a = inspect_animation(&webp(&[100, 200, 5], 0, true));
        assert!(a.kind == AnimKind::Webp && a.animated && a.frames == 3 && a.loop_ms == 400.0, "{a:?}");
    }
    #[test]
    fn webp_loop_forever() {
        assert_eq!(inspect_animation(&webp(&[100, 200, 5], 0, true)).plays, 0);
    }
    #[test]
    fn webp_canvas_size() {
        let a = inspect_animation(&webp(&[100, 200, 5], 0, true));
        assert!(a.width == 10 && a.height == 5);
    }
    #[test]
    fn webp_loop_count() {
        assert_eq!(inspect_animation(&webp(&[100, 100], 2, true)).plays, 2);
    }
    #[test]
    fn webp_no_anim_flag_is_still() {
        assert!(!inspect_animation(&webp(&[100, 100], 0, false)).animated);
    }
    #[test]
    fn webp_single_frame_is_still() {
        assert!(!inspect_animation(&webp(&[100], 0, true)).animated);
    }

    // others
    #[test]
    fn jpeg_is_other() {
        let mut b = vec![0xff, 0xd8, 0xff, 0xe0, 0, 16];
        b.extend(b"JFIF");
        assert_eq!(inspect_animation(&b).kind, AnimKind::Other);
    }
    #[test]
    fn png_is_other() {
        let mut b = vec![0x89];
        b.extend(b"PNG");
        b.extend([13, 10, 26, 10]);
        assert!(!inspect_animation(&b).animated);
    }
    #[test]
    fn garbage_is_not_animated() {
        assert!(!inspect_animation(b"GIF89a").animated && !inspect_animation(&[]).animated);
    }
}
