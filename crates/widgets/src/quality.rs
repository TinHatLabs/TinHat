//! The Call stats widget's shared maths — port of the web's `src/widgets/quality.ts` (contract WIDGETS.md §5 +
//! WIDGETS-UI.md "Call stats"; vectors `testvectors/widgets/quality/*`): quality bands, LiveKit ConnectionQuality →
//! band, one track's receive numbers from two WebRTC stats samples, the lane bar's bitrate scale, codec / resolution /
//! bitrate labels and the sparkline's geometry.

use serde_json::{Map, Value};

use crate::{finite, js_max, js_min, js_number, num};

/// A quality band (WIDGETS.md §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Band {
    Good,
    Ok,
    Poor,
}

impl Band {
    pub fn as_str(self) -> &'static str {
        match self {
            Band::Good => "good",
            Band::Ok => "ok",
            Band::Poor => "poor",
        }
    }
    pub fn parse(s: &str) -> Option<Band> {
        match s {
            "good" => Some(Band::Good),
            "ok" => Some(Band::Ok),
            "poor" => Some(Band::Poor),
            _ => None,
        }
    }
}

/// a = voice, v = camera, s = screen share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    A,
    V,
    S,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::A => "a",
            Kind::V => "v",
            Kind::S => "s",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "a" => Some(Kind::A),
            "v" => Some(Kind::V),
            "s" => Some(Kind::S),
            _ => None,
        }
    }
    /// KIND_RANK: a, v, s.
    pub fn rank(self) -> u8 {
        match self {
            Kind::A => 0,
            Kind::V => 1,
            Kind::S => 2,
        }
    }
}

/// What one side measures about one received track (the `tinhat.rxq` entry; loss in per-mille). `None` = undefined.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackNumbers {
    pub k: Kind,
    pub kbps: Option<f64>,
    /// packet loss, per-mille (0‥1000)
    pub loss: Option<f64>,
    /// jitter, ms
    pub jit: Option<f64>,
    /// round-trip time, ms
    pub rtt: Option<f64>,
    pub w: Option<f64>,
    pub h: Option<f64>,
    pub fps: Option<f64>,
    /// codec label ≤ 8 chars ("AV1", "H.264", "Opus")
    pub c: Option<String>,
}

impl TrackNumbers {
    pub fn new(k: Kind) -> Self {
        TrackNumbers { k, kbps: None, loss: None, jit: None, rtt: None, w: None, h: None, fps: None, c: None }
    }

    /// The numeric fields in NUM_KEYS order.
    fn get(&self, key: NumKey) -> Option<f64> {
        match key {
            NumKey::Kbps => self.kbps,
            NumKey::Loss => self.loss,
            NumKey::Jit => self.jit,
            NumKey::Rtt => self.rtt,
            NumKey::W => self.w,
            NumKey::H => self.h,
            NumKey::Fps => self.fps,
        }
    }
    fn set(&mut self, key: NumKey, v: f64) {
        let slot = match key {
            NumKey::Kbps => &mut self.kbps,
            NumKey::Loss => &mut self.loss,
            NumKey::Jit => &mut self.jit,
            NumKey::Rtt => &mut self.rtt,
            NumKey::W => &mut self.w,
            NumKey::H => &mut self.h,
            NumKey::Fps => &mut self.fps,
        };
        *slot = Some(v);
    }

    /// Its band (`bandOf`).
    pub fn band(&self) -> Option<Band> {
        band_of(self.loss, self.jit, self.rtt)
    }

    /// The object the reference holds (only the fields that are set).
    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("k".into(), Value::String(self.k.as_str().into()));
        for key in NUM_KEYS {
            if let Some(v) = self.get(key) {
                o.insert(key.name().into(), num(v));
            }
        }
        if let Some(c) = &self.c {
            o.insert("c".into(), Value::String(c.clone()));
        }
        Value::Object(o)
    }

    /// Read a reference-shaped object: `k` must be a/v/s (else None); numbers are taken when they are JSON numbers,
    /// `c` when it is a string (both unvalidated — pass the result through [`clamp_numbers`]).
    pub fn from_json(v: &Value) -> Option<TrackNumbers> {
        let o = v.as_object()?;
        let k = Kind::parse(o.get("k")?.as_str()?)?;
        let mut n = TrackNumbers::new(k);
        for key in NUM_KEYS {
            if let Some(x) = o.get(key.name()).and_then(Value::as_f64) {
                n.set(key, x);
            }
        }
        if let Some(Value::String(c)) = o.get("c") {
            n.c = Some(c.clone());
        }
        Some(n)
    }
}

/// Round half up, never below 0 (the one integer rounding of the widget maths).
pub fn r(x: f64) -> f64 {
    if x <= 0.0 {
        0.0
    } else {
        (x + 0.5).floor()
    }
}

// ---- quality bands (§5) ---------------------------------------------------------------------------------------------

pub const GOOD_LOSS: f64 = 20.0;
pub const GOOD_JIT: f64 = 30.0;
pub const GOOD_RTT: f64 = 150.0;
pub const POOR_LOSS: f64 = 80.0;
pub const POOR_JIT: f64 = 80.0;
pub const POOR_RTT: f64 = 400.0;

/// good < 2 % loss AND < 30 ms jitter AND < 150 ms RTT; poor ≥ 8 % OR ≥ 80 ms OR ≥ 400 ms; else ok. A missing number
/// neither helps nor hurts; nothing measured at all = None (unknown).
pub fn band_of(loss: Option<f64>, jit: Option<f64>, rtt: Option<f64>) -> Option<Band> {
    if loss.is_none() && jit.is_none() && rtt.is_none() {
        return None;
    }
    if loss.is_some_and(|x| x >= POOR_LOSS) || jit.is_some_and(|x| x >= POOR_JIT) || rtt.is_some_and(|x| x >= POOR_RTT) {
        return Some(Band::Poor);
    }
    if loss.is_none_or(|x| x < GOOD_LOSS) && jit.is_none_or(|x| x < GOOD_JIT) && rtt.is_none_or(|x| x < GOOD_RTT) {
        return Some(Band::Good);
    }
    Some(Band::Ok)
}

/// The worst of several bands (None entries ignored; none = None).
pub fn worst_band<I: IntoIterator<Item = Option<Band>>>(bands: I) -> Option<Band> {
    bands.into_iter().flatten().max()
}

/// LiveKit ConnectionQuality → the person's dot: excellent/good → good, poor → ok, lost → poor, anything else → None.
pub fn cq_band(q: Option<&str>) -> Option<Band> {
    match q {
        Some("excellent") | Some("good") => Some(Band::Good),
        Some("poor") => Some(Band::Ok),
        Some("lost") => Some(Band::Poor),
        _ => None,
    }
}

// ---- one received track from two stats samples ----------------------------------------------------------------------

/// One WebRTC `inbound-rtp` sample of a received track, normalised (both apps read the same fields).
#[derive(Clone, Debug, PartialEq)]
pub struct RxSample {
    pub k: Kind,
    /// sample time, ms
    pub at: f64,
    /// bytesReceived, packetsReceived, packetsLost (cumulative; packetsLost may go down — duplicates)
    pub bytes: f64,
    pub packets: f64,
    pub lost: f64,
    /// jitter, seconds (inbound-rtp.jitter)
    pub jitter: Option<f64>,
    /// round-trip time to the server, seconds (the receiving transport's candidate-pair currentRoundTripTime)
    pub rtt: Option<f64>,
    /// video: framesPerSecond, frameWidth, frameHeight
    pub fps: Option<f64>,
    pub w: Option<f64>,
    pub h: Option<f64>,
    /// the codec's mimeType ("video/AV1", "audio/opus")
    pub codec: Option<String>,
}

impl RxSample {
    /// Read a reference-shaped sample (the vectors' `prev` / `cur`); None when `k` is not a/v/s.
    pub fn from_json(v: &Value) -> Option<RxSample> {
        let o = v.as_object()?;
        let k = Kind::parse(o.get("k")?.as_str()?)?;
        let n = |key: &str| o.get(key).and_then(Value::as_f64);
        Some(RxSample {
            k,
            at: n("at").unwrap_or(f64::NAN),
            bytes: n("bytes").unwrap_or(f64::NAN),
            packets: n("packets").unwrap_or(f64::NAN),
            lost: n("lost").unwrap_or(f64::NAN),
            jitter: n("jitter"),
            rtt: n("rtt"),
            fps: n("fps"),
            w: n("w"),
            h: n("h"),
            codec: o.get("codec").and_then(Value::as_str).map(str::to_string),
        })
    }
}

/// The numbers of one track over the interval prev → cur. None: no prev; a time, byte or packet count that is not a
/// finite number; cur.at ≤ prev.at; or bytesReceived / packetsReceived went DOWN (the stats object was replaced — a
/// resubscription: start again from cur). packetsLost may go down (late / duplicate packets): its delta is floored at 0.
pub fn rx_numbers(prev: Option<&RxSample>, cur: &RxSample) -> Option<TrackNumbers> {
    let prev = prev?;
    for v in [prev.at, prev.bytes, prev.packets, prev.lost, cur.at, cur.bytes, cur.packets, cur.lost] {
        if !v.is_finite() {
            return None;
        }
    }
    if !(cur.at > prev.at) || cur.bytes < prev.bytes || cur.packets < prev.packets {
        return None;
    }
    let dt = cur.at - prev.at;
    let mut out = TrackNumbers::new(cur.k);
    out.kbps = Some(r(((cur.bytes - prev.bytes) * 8.0) / dt));
    let lost = js_max(0.0, cur.lost - prev.lost);
    let got = js_max(0.0, cur.packets - prev.packets);
    out.loss = Some(if lost + got > 0.0 { r((1000.0 * lost) / (lost + got)) } else { 0.0 });
    if let Some(j) = cur.jitter.filter(|x| x.is_finite()) {
        out.jit = Some(r(j * 1000.0));
    }
    if let Some(t) = cur.rtt.filter(|x| x.is_finite()) {
        out.rtt = Some(r(t * 1000.0));
    }
    if cur.k != Kind::A {
        if let (Some(w), Some(h)) = (cur.w, cur.h) {
            if w > 0.0 && h > 0.0 {
                out.w = Some(r(w));
                out.h = Some(r(h));
            }
        }
        if let Some(f) = cur.fps.filter(|x| x.is_finite()) {
            out.fps = Some(r(f));
        }
    }
    let c = codec_name(cur.codec.as_deref());
    if !c.is_empty() {
        out.c = Some(c);
    }
    Some(clamp_numbers(&out))
}

/// The numeric keys, in NUM_KEYS order, with their limits (the `tinhat.rxq` clamps).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumKey {
    Kbps,
    Loss,
    Jit,
    Rtt,
    W,
    H,
    Fps,
}

pub const NUM_KEYS: [NumKey; 7] = [NumKey::Kbps, NumKey::Loss, NumKey::Jit, NumKey::Rtt, NumKey::W, NumKey::H, NumKey::Fps];

impl NumKey {
    pub fn name(self) -> &'static str {
        match self {
            NumKey::Kbps => "kbps",
            NumKey::Loss => "loss",
            NumKey::Jit => "jit",
            NumKey::Rtt => "rtt",
            NumKey::W => "w",
            NumKey::H => "h",
            NumKey::Fps => "fps",
        }
    }
    pub fn limit(self) -> f64 {
        match self {
            NumKey::Kbps => 1_000_000.0,
            NumKey::Loss => 1000.0,
            NumKey::Jit | NumKey::Rtt => 60_000.0,
            NumKey::W | NumKey::H => 16_384.0,
            NumKey::Fps => 1000.0,
        }
    }
}

/// Clamp every number to its limit (r() applied), drop a codec that isn't a valid label.
pub fn clamp_numbers(n: &TrackNumbers) -> TrackNumbers {
    let mut out = TrackNumbers::new(n.k);
    for key in NUM_KEYS {
        if let Some(v) = n.get(key).filter(|x| x.is_finite()) {
            out.set(key, js_min(key.limit(), r(v)));
        }
    }
    if let Some(c) = n.c.as_ref().filter(|c| valid_codec_label(c)) {
        out.c = Some(c.clone());
    }
    out
}

// ---- labels ---------------------------------------------------------------------------------------------------------

fn known_codec(sub: &str) -> Option<&'static str> {
    Some(match sub {
        "opus" => "Opus",
        "av1" => "AV1",
        "h264" => "H.264",
        "h265" | "hevc" => "H.265",
        "vp8" => "VP8",
        "vp9" => "VP9",
        "red" => "RED",
        "pcmu" => "PCMU",
        "pcma" => "PCMA",
        "g722" => "G722",
        _ => return None,
    })
}

/// A codec label from a mimeType ("video/H264" → "H.264", "audio/opus" → "Opus"). ASCII-only rules (the same bytes in
/// every language): the part after the last "/", ASCII whitespace (U+0009–U+000D, U+0020) trimmed, ASCII letters
/// lower-cased for the (own-key) lookup; unknown subtypes: ASCII letters upper-cased, only A–Z 0–9 . + - kept, cut to
/// 8; "" when nothing is left.
pub fn codec_name(mime: Option<&str>) -> String {
    let Some(mime) = mime else { return String::new() };
    let sub = match mime.rfind('/') {
        Some(i) => &mime[i + 1..],
        None => mime,
    };
    let is_ws = |c: char| c == ' ' || ('\u{9}'..='\u{d}').contains(&c);
    let sub = sub.trim_matches(is_ws);
    if sub.is_empty() {
        return String::new();
    }
    let lower: String = sub.chars().map(|c| c.to_ascii_lowercase()).collect();
    if let Some(k) = known_codec(&lower) {
        return k.to_string();
    }
    let mut s = String::new();
    for ch in lower.chars() {
        let up = ch.to_ascii_uppercase();
        if up.is_ascii_uppercase() || up.is_ascii_digit() || matches!(up, '.' | '+' | '-') {
            s.push(up);
        }
        if s.len() == 8 {
            break;
        }
    }
    s
}

/// A codec label a peer may send: 1–8 of A–Z a–z 0–9 . + -
pub fn valid_codec_label(s: &str) -> bool {
    let n = s.chars().count();
    (1..=8).contains(&n) && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

/// "720p30": the short side + p + fps ("720p" without fps); "" without a size.
pub fn res_chip(w: Option<f64>, h: Option<f64>, fps: Option<f64>) -> String {
    let (Some(w), Some(h)) = (w, h) else { return String::new() };
    if !(w > 0.0 && h > 0.0) {
        return String::new();
    }
    let p = js_min(w, h);
    match fps {
        Some(f) if f > 0.0 => format!("{}p{}", js_number(p), js_number(f)),
        _ => format!("{}p", js_number(p)),
    }
}

/// "48 kb/s", "1.4 Mb/s", "120 Mb/s" (kb/s below 1000; one decimal below 100 Mb/s).
pub fn fmt_rate(kbps: f64) -> String {
    let k = r(kbps);
    if k < 1000.0 {
        return format!("{} kb/s", js_number(k));
    }
    if k < 100_000.0 {
        let tenths = ((k + 50.0) / 100.0).floor();
        return format!("{}.{} Mb/s", js_number((tenths / 10.0).floor()), js_number(tenths % 10.0));
    }
    format!("{} Mb/s", js_number(((k + 500.0) / 1000.0).floor()))
}

/// "2.4 %" from per-mille (one decimal).
pub fn fmt_loss(pm: f64) -> String {
    let p = r(pm);
    format!("{}.{} %", js_number((p / 10.0).floor()), js_number(p % 10.0))
}

// ---- the lane bar ---------------------------------------------------------------------------------------------------

/// The lane bar's length in per-mille: doublings of bitrate from 8 kb/s (0) to 8192 kb/s (1000), linear between the
/// powers of two — voice ≈ 200‰, 720p camera ≈ 750‰, a 1080p share ≈ 950‰.
pub fn rate_fill_pm(kbps: f64) -> f64 {
    let k = r(kbps);
    if k <= 8.0 {
        return 0.0;
    }
    if k >= 8192.0 {
        return 1000.0;
    }
    let mut i = 0.0;
    let mut lo = 8.0;
    while lo * 2.0 <= k {
        lo *= 2.0;
        i += 1.0;
    }
    let num = (i * lo + (k - lo)) * 1000.0;
    let den = lo * 10.0;
    ((2.0 * num + den) / (2.0 * den)).floor()
}

// ---- the sparkline --------------------------------------------------------------------------------------------------

pub const SPARK_SAMPLES: usize = 60;

/// Headroom above the window's peak (WIDGETS.md v1.2): the peak sits at 80 % of the height — a steady stream draws a
/// line, not a block.
pub const SPARK_HEADROOM: f64 = 1.25;

/// The sparkline's points in centi-pixels ([x, y] × 100, integers) for the last ≤ 60 values in a w × h box: the newest
/// sample at the right edge (x = w − 1), one step = (w − 2) / 59; top = the window's max × 1.25; y = (h − 1) −
/// (v / top) × (h − 2) (an all-zero window is a flat line at the bottom). `None` values (not finite numbers) and values
/// ≤ 0 count as 0.
pub fn spark_points(values: &[Option<f64>], w: f64, h: f64) -> Vec<(f64, f64)> {
    let tail = &values[values.len().saturating_sub(SPARK_SAMPLES)..];
    let vals: Vec<f64> = tail.iter().map(|v| v.filter(|x| x.is_finite() && *x > 0.0).unwrap_or(0.0)).collect();
    let n = vals.len();
    if n == 0 {
        return Vec::new();
    }
    let mut max = 0.0;
    for v in &vals {
        if *v > max {
            max = *v;
        }
    }
    let top = max * SPARK_HEADROOM;
    let step = (w - 2.0) / (SPARK_SAMPLES as f64 - 1.0);
    vals.iter()
        .enumerate()
        .map(|(i, v)| {
            let x = 1.0 + ((SPARK_SAMPLES - n + i) as f64) * step;
            let y = h - 1.0 - (if top > 0.0 { (v / top) * (h - 2.0) } else { 0.0 });
            (r(x * 100.0), r(y * 100.0))
        })
        .collect()
}

/// A JSON value → an optional finite number (the reference's `typeof v === "number" && Number.isFinite(v)`).
pub fn finite_of(v: Option<&Value>) -> Option<f64> {
    finite(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_checks_of_the_reference() {
        assert_eq!(band_of(Some(19.0), Some(29.0), Some(149.0)), Some(Band::Good));
        assert_eq!(band_of(Some(20.0), None, None), Some(Band::Ok));
        assert_eq!(band_of(None, None, Some(400.0)), Some(Band::Poor));
        assert_eq!(band_of(None, None, None), None);
        assert_eq!(rate_fill_pm(48.0), 250.0);
        assert!(rate_fill_pm(1450.0) > 700.0 && rate_fill_pm(1450.0) < 800.0);
        assert!(rate_fill_pm(6100.0) > 900.0);
        let prev = RxSample { k: Kind::A, at: 0.0, bytes: 0.0, packets: 0.0, lost: 0.0, jitter: None, rtt: None, fps: None, w: None, h: None, codec: None };
        let cur = RxSample { at: 1000.0, bytes: 6000.0, packets: 50.0, lost: 1.0, jitter: Some(0.0123), rtt: Some(0.041), codec: Some("audio/opus".into()), ..prev.clone() };
        let n = rx_numbers(Some(&prev), &cur).unwrap();
        assert_eq!(n.to_json().to_string(), r#"{"c":"Opus","jit":12,"k":"a","kbps":48,"loss":20,"rtt":41}"#);
    }
}
