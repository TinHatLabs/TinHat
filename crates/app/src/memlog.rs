//! 12.1.12: a periodic memory line in the log (`[mem] …`, every 60 s).
//!
//! A report of "Tin Hat takes 8 GB" carried no memory numbers at all, so the log could not say what grew or since
//! when. This line puts the process's own figures in every log tail: resident (rss), proportional (pss), anonymous
//! (anon) and swapped memory from /proc/self/smaps_rollup, glibc's in-use heap (heap), the thread count, and the
//! video frames handed to libwebrtc since the last line (fed), with the growth per minute and per fed frame, so a leak
//! that scales with frames shows as a steady KB/frame. Only this process's own counters: nothing about the system,
//! other processes or the user. Windows logs the working set and private bytes (GetProcessMemoryInfo).
//!
//! The parsing and the line are pure (tested below); `spawn` is the thread around them.

use std::time::{Duration, Instant};

pub const PERIOD: Duration = Duration::from_secs(60);

/// One sample, in KiB. None = not measured (not available on this platform, or the read failed): left out of the line,
/// never logged as a zero.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MemSample {
    pub rss_kb: Option<u64>,
    pub pss_kb: Option<u64>,
    pub anon_kb: Option<u64>,
    pub swap_kb: Option<u64>,
    /// Windows: private bytes (committed private memory)
    pub private_kb: Option<u64>,
    pub heap_kb: Option<u64>,
    pub threads: Option<u32>,
    /// video frames handed to libwebrtc since the process started (camera + share, keepalive repeats included)
    pub frames_fed: u64,
}

/// The fields of /proc/self/smaps_rollup this line reads (values are "<n> kB").
pub fn parse_smaps_rollup(text: &str, s: &mut MemSample) {
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(key), Some(val)) = (it.next(), it.next()) else { continue };
        let Ok(v) = val.parse::<u64>() else { continue };
        match key {
            "Rss:" => s.rss_kb = Some(v),
            "Pss:" => s.pss_kb = Some(v),
            "Anonymous:" => s.anon_kb = Some(v),
            "Swap:" => s.swap_kb = Some(v),
            _ => {}
        }
    }
}

/// The run-time fence for the clean-exit signal setup: it must find this process with one thread. (The log line, ok.)
/// A thread started earlier keeps the default action for SIGTERM/INT/HUP, so a logout or a kill would end the app at once.
pub fn signal_setup_report(threads: Option<u32>) -> (String, bool) {
    match threads {
        Some(1) => ("[exit] signal setup: Threads=1".to_string(), true),
        Some(n) => (format!("ERROR [exit] signal setup: Threads={n}, expected 1: a thread started before this point skips the clean exit"), false),
        None => ("ERROR [exit] signal setup: the thread count could not be read".to_string(), false),
    }
}

/// `Threads:` from /proc/self/status.
pub fn parse_threads(status: &str) -> Option<u32> {
    status.lines().find_map(|l| l.strip_prefix("Threads:").and_then(|v| v.trim().parse().ok()))
}

fn mb(kb: u64) -> String {
    format!("{:.0}MB", kb as f64 / 1024.0)
}

/// The log line for `now`, with the growth since `prev` (taken `secs` earlier).
pub fn format_line(now: &MemSample, prev: Option<(&MemSample, f64)>) -> String {
    let mut out = String::from("[mem]");
    for (name, v) in [
        ("rss", now.rss_kb),
        ("pss", now.pss_kb),
        ("anon", now.anon_kb),
        ("swap", now.swap_kb),
        ("private", now.private_kb),
        ("heap", now.heap_kb),
    ] {
        if let Some(v) = v {
            out.push_str(&format!(" {name}={}", mb(v)));
        }
    }
    if out == "[mem]" {
        out.push_str(" unavailable");
    }
    if let Some(t) = now.threads {
        out.push_str(&format!(" threads={t}"));
    }
    if let Some((p, secs)) = prev {
        let fed = now.frames_fed.saturating_sub(p.frames_fed);
        let delta = |a: Option<u64>, b: Option<u64>| Some(a? as i64 - b? as i64);
        let d_rss = delta(now.rss_kb, p.rss_kb);
        // the growing private memory: anonymous on Linux, private bytes on Windows
        let (grow_name, d_grow) = match delta(now.anon_kb, p.anon_kb) {
            Some(d) => ("anon", Some(d)),
            None => ("private", delta(now.private_kb, p.private_kb)),
        };
        out.push_str(&format!(" fed={fed}"));
        if secs > 0.0 {
            if let Some(d) = d_rss {
                out.push_str(&format!(" d_rss={:+.1}MB/min", d as f64 / 1024.0 * 60.0 / secs));
            }
            if let Some(d) = d_grow {
                out.push_str(&format!(" d_{grow_name}={:+.1}MB/min", d as f64 / 1024.0 * 60.0 / secs));
            }
        }
        if let (true, Some(d)) = (fed > 0, d_grow) {
            out.push_str(&format!(" {grow_name}_per_frame={:+.1}KB", d as f64 / fed as f64));
        }
    }
    out
}

/// glibc's in-use heap: mallinfo2's arena bytes in use + mmapped chunks (glibc >= 2.33; allocator-retained chunks such
/// as tcache entries count as in use). It locks each arena and walks its free lists: fine once a minute.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn heap_kb() -> Option<u64> {
    // SAFETY: mallinfo2 takes no arguments and returns a plain struct by value.
    let mi = unsafe { libc::mallinfo2() };
    Some(((mi.uordblks + mi.hblkhd) / 1024) as u64)
}
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn heap_kb() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
pub fn sample() -> MemSample {
    let mut s = MemSample::default();
    if let Ok(t) = std::fs::read_to_string("/proc/self/smaps_rollup") {
        parse_smaps_rollup(&t, &mut s);
    }
    s.threads = std::fs::read_to_string("/proc/self/status").ok().and_then(|t| parse_threads(&t));
    s.heap_kb = heap_kb();
    s.frames_fed = tinhat_streamer::lk_publish::video_frames_fed();
    s
}

#[cfg(windows)]
pub fn sample() -> MemSample {
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    extern "system" {
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn K32GetProcessMemoryInfo(p: *mut core::ffi::c_void, c: *mut ProcessMemoryCountersEx, cb: u32) -> i32;
    }
    let mut c = ProcessMemoryCountersEx { cb: std::mem::size_of::<ProcessMemoryCountersEx>() as u32, ..Default::default() };
    let mut s = MemSample::default();
    // SAFETY: a pseudo-handle for this process and a correctly sized, initialised out struct.
    if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) } != 0 {
        // the working set is the resident set; private bytes = committed private memory (no PSS / anonymous split)
        s.rss_kb = Some((c.working_set_size / 1024) as u64);
        s.private_kb = Some((c.private_usage / 1024) as u64);
    }
    s.frames_fed = tinhat_streamer::lk_publish::video_frames_fed();
    s
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn sample() -> MemSample {
    MemSample { frames_fed: tinhat_streamer::lk_publish::video_frames_fed(), ..Default::default() }
}

/// Start the `[mem]` thread: one line at start, then one every [`PERIOD`]. TINHAT_MEMLOG=0 turns it off.
pub fn spawn() {
    if std::env::var("TINHAT_MEMLOG").as_deref() == Ok("0") {
        return;
    }
    let period = std::env::var("TINHAT_MEMLOG_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s >= 1)
        .map(Duration::from_secs)
        .unwrap_or(PERIOD);
    let _ = std::thread::Builder::new().name("memlog".into()).spawn(move || {
        let mut prev: Option<(MemSample, Instant)> = None;
        loop {
            let now = sample();
            let at = Instant::now();
            let line = format_line(&now, prev.as_ref().map(|(p, t)| (p, at.duration_since(*t).as_secs_f64())));
            eprintln!("{line}");
            prev = Some((now, at));
            std::thread::sleep(period);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLLUP: &str = "55d0c0a8e000-7ffd5a1f2000 ---p 00000000 00:00 0                          [rollup]
Rss:             8123456 kB
Pss:             8001234 kB
Pss_Dirty:       7900000 kB
Pss_Anon:        7800000 kB
Pss_File:         200000 kB
Shared_Clean:     100000 kB
Anonymous:       7812345 kB
Swap:                 12 kB
SwapPss:              12 kB
";

    #[test]
    fn smaps_rollup_fields_are_read() {
        let mut s = MemSample::default();
        parse_smaps_rollup(ROLLUP, &mut s);
        assert_eq!((s.rss_kb, s.pss_kb, s.anon_kb, s.swap_kb), (Some(8123456), Some(8001234), Some(7812345), Some(12)));
    }

    #[test]
    fn garbage_leaves_nothing_measured() {
        let mut s = MemSample::default();
        parse_smaps_rollup("Rss: lots kB\nnonsense\n\nPss:\n", &mut s);
        assert_eq!(s, MemSample::default());
        assert_eq!(format_line(&s, None), "[mem] unavailable", "a failed read is not logged as zeros");
    }

    #[test]
    fn threads_from_status() {
        assert_eq!(parse_threads("Name:\ttinhat\nThreads:\t87\nSigQ:\t0/1\n"), Some(87));
        assert_eq!(parse_threads("Name:\ttinhat\n"), None);
    }

    #[test]
    fn first_line_has_no_growth() {
        let s = MemSample {
            rss_kb: Some(1024 * 900),
            pss_kb: Some(1024 * 850),
            anon_kb: Some(1024 * 600),
            swap_kb: Some(0),
            heap_kb: Some(1024 * 300),
            threads: Some(80),
            ..Default::default()
        };
        assert_eq!(format_line(&s, None), "[mem] rss=900MB pss=850MB anon=600MB swap=0MB heap=300MB threads=80");
    }

    #[test]
    fn growth_per_minute_and_per_frame() {
        // the reported share: ~30 KB per frame at 30 fps = 1800 frames and ~53 MB in a minute
        let p = MemSample { rss_kb: Some(1_000_000), anon_kb: Some(700_000), frames_fed: 10_000, ..Default::default() };
        let n = MemSample { rss_kb: Some(1_054_000), anon_kb: Some(754_000), frames_fed: 11_800, ..Default::default() };
        let line = format_line(&n, Some((&p, 60.0)));
        assert!(line.ends_with(" fed=1800 d_rss=+52.7MB/min d_anon=+52.7MB/min anon_per_frame=+30.0KB"), "{line}");
    }

    #[test]
    fn shrinking_and_idle_are_signed_and_frame_free() {
        let p = MemSample { rss_kb: Some(2048), anon_kb: Some(2048), frames_fed: 5, ..Default::default() };
        let n = MemSample { rss_kb: Some(1024), anon_kb: Some(1024), frames_fed: 5, ..Default::default() };
        let line = format_line(&n, Some((&p, 30.0)));
        assert!(line.ends_with(" fed=0 d_rss=-2.0MB/min d_anon=-2.0MB/min"), "{line}");
    }

    #[test]
    fn windows_figures_are_named_for_what_they_are() {
        let p = MemSample { rss_kb: Some(512 * 1024), private_kb: Some(400 * 1024), frames_fed: 0, ..Default::default() };
        let n = MemSample { rss_kb: Some(512 * 1024), private_kb: Some(430 * 1024), frames_fed: 1000, ..Default::default() };
        let line = format_line(&n, Some((&p, 60.0)));
        assert_eq!(line, "[mem] rss=512MB private=430MB fed=1000 d_rss=+0.0MB/min d_private=+30.0MB/min private_per_frame=+30.7KB");
    }

    #[test]
    fn a_figure_missing_on_one_side_has_no_growth() {
        let p = MemSample { rss_kb: None, anon_kb: Some(100), ..Default::default() };
        let n = MemSample { rss_kb: Some(200), anon_kb: Some(100), ..Default::default() };
        assert_eq!(format_line(&n, Some((&p, 60.0))), "[mem] rss=0MB anon=0MB fed=0 d_anon=+0.0MB/min");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_has_a_real_sample() {
        let s = sample();
        assert!(s.rss_kb > Some(0) && s.pss_kb > Some(0) && s.anon_kb > Some(0), "{s:?}");
        assert!(s.threads.unwrap_or(0) >= 1);
    }
}

#[cfg(test)]
mod start_order_tests {
    /// The clean-exit signal setup blocks SIGTERM/INT/HUP for the threads started after it. A thread started before it
    /// keeps the default action, so a logout or a kill ends the app at once: no leave-on-exit, the call membership stays.
    use super::signal_setup_report;

    #[test]
    fn the_signal_setup_report_flags_any_earlier_thread() {
        assert_eq!(signal_setup_report(Some(1)), ("[exit] signal setup: Threads=1".to_string(), true));
        let (two, ok) = signal_setup_report(Some(2));
        assert!(!ok);
        assert!(two.starts_with("ERROR ") && two.contains("Threads=2"), "{two}");
        assert!(!signal_setup_report(None).1);
    }

    #[test]
    fn the_memory_thread_starts_after_the_signal_setup() {
        let main = include_str!("main.rs");
        let signals = main.find("\n    install_clean_exit_signals();").expect("the signal setup call in main()");
        let memlog = main.find("\n    memlog::spawn();").expect("the memlog::spawn call in main()");
        assert!(signals < memlog, "memlog::spawn() must come after install_clean_exit_signals()");
    }
}
