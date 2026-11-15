//! The `[recv-stats]` log line: what this app, as a RECEIVER, did about one remote video track. It names who asks a
//! sender for keyframes: the sender only sees a PLI count, so each receiver's own log has to say it was the one.
//!
//! Shared logic: the web's `src/features/calls/recvStats.ts` builds the same line, and both run
//! `testvectors/recv-stats/format.json`. A figure the receiver doesn't report is left out, never written as a zero.

/// How often a call logs the line.
pub const RECV_LOG_MS: f64 = 30_000.0;
/// The samplers tick on their own timers: this much early still counts as due.
const SLACK_MS: f64 = 250.0;
const ID_MAX: usize = 128;
const DECODER_MAX: usize = 64;

/// One remote video track's receiver counters, as totals since the track started.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecvCounters {
    pub pli: Option<u64>,
    pub nack: Option<u64>,
    pub fir: Option<u64>,
    pub decoded: Option<u64>,
    pub dropped: Option<u64>,
    pub kf: Option<u64>,
    pub w: Option<u64>,
    pub h: Option<u64>,
    pub decoder: Option<String>,
}

pub fn log_due(last: Option<f64>, now: f64) -> bool {
    last.map_or(true, |l| now - l >= RECV_LOG_MS - SLACK_MS)
}

/// Keeps one log line one line: what isn't printable ASCII (or is a quote) becomes `_`.
fn safe(s: &str, max: usize, spaces: bool) -> String {
    s.chars()
        .take(max)
        .map(|c| if (c.is_ascii_graphic() && c != '\'') || (spaces && c == ' ') { c } else { '_' })
        .collect()
}

fn id(s: &str) -> String {
    if s.is_empty() {
        "?".to_string()
    } else {
        safe(s, ID_MAX, false)
    }
}

/// `prev` is the same track's counters at its last line and `dt` the seconds since then (both None on its first line).
pub fn format_line(who: &str, src: &str, sid: &str, cur: &RecvCounters, prev: Option<&RecvCounters>, dt: Option<u64>) -> String {
    let mut line = format!("[recv-stats] who={} src={src} sid={}", id(who), id(sid));
    if let (Some(w), Some(h)) = (cur.w, cur.h) {
        if w > 0 && h > 0 {
            line += &format!(" res={w}x{h}");
        }
    }
    if let Some(d) = cur.decoder.as_deref().filter(|d| !d.is_empty()) {
        line += &format!(" decoder='{}'", safe(d, DECODER_MAX, true));
    }
    if let Some(dt) = dt {
        line += &format!(" dt={dt}");
    }
    let of = |name: &str, c: Option<u64>, p: Option<u64>| -> String {
        let Some(c) = c else { return String::new() };
        // a counter that went backwards restarted from zero
        let delta = p.map(|p| if c >= p { c - p } else { c });
        match delta {
            Some(d) if prev.is_some() => format!(" {name}={c} d_{name}={d}"),
            _ => format!(" {name}={c}"),
        }
    };
    let p = prev.cloned().unwrap_or_default();
    line += &of("pli", cur.pli, p.pli);
    line += &of("nack", cur.nack, p.nack);
    line += &of("fir", cur.fir, p.fir);
    line += &of("decoded", cur.decoded, p.decoded);
    line += &of("dropped", cur.dropped, p.dropped);
    line += &of("kf", cur.kf, p.kf);
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn vectors() -> Value {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testvectors/recv-stats/format.json");
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    fn counters(v: &Value) -> RecvCounters {
        RecvCounters {
            pli: v["pli"].as_u64(),
            nack: v["nack"].as_u64(),
            fir: v["fir"].as_u64(),
            decoded: v["decoded"].as_u64(),
            dropped: v["dropped"].as_u64(),
            kf: v["kf"].as_u64(),
            w: v["w"].as_u64(),
            h: v["h"].as_u64(),
            decoder: v["decoder"].as_str().map(String::from),
        }
    }

    #[test]
    fn log_period_matches_the_shared_vectors() {
        assert_eq!(RECV_LOG_MS, vectors()["logMs"].as_f64().unwrap());
    }

    #[test]
    fn format_matches_the_shared_vectors() {
        let v = vectors();
        for c in v["format"].as_array().unwrap() {
            let cur = counters(&c["cur"]);
            let prev = (!c["prev"].is_null()).then(|| counters(&c["prev"]));
            let line = format_line(
                c["who"].as_str().unwrap(),
                c["src"].as_str().unwrap(),
                c["sid"].as_str().unwrap(),
                &cur,
                prev.as_ref(),
                c["dt"].as_u64(),
            );
            assert_eq!(line, c["line"].as_str().unwrap(), "{}", c["name"]);
        }
    }

    #[test]
    fn due_matches_the_shared_vectors() {
        let v = vectors();
        for c in v["due"].as_array().unwrap() {
            assert_eq!(log_due(c["last"].as_f64(), c["now"].as_f64().unwrap()), c["due"].as_bool().unwrap(), "{}", c["name"]);
        }
    }
}
