//! Call link watch (reconnect-wedge, 12.1.13): when a call that is no longer really connected must end
//! VISIBLY — the "Call ended — connection lost" banner with Rejoin, or the auto-rejoin that runs before it.
//!
//! Two field failures on 12.1.11 (a Linux laptop on flaky Wi-Fi, then a pulled cable) showed the call
//! screen claiming "In call · sharing screen" while nothing was being sent:
//!   * a resume wedged in its first attempt (the SDK's signal close waited on a dead socket) and a share
//!     pressed meanwhile hung in publish_track, the OS showing the share live, no error;
//!   * the engine gave up after its 10 attempts and the app never heard: 42 minutes later it still said
//!     "In call · sharing screen" and kept encoding.
//! The vendored SDK now bounds every reconnect step (rtc_engine RESUME_ATTEMPT_TIMEOUT …). This is the
//! app's own guard on top, independent of the SDK's event delivery: the session's watchdog feeds it the
//! room's connection state once a second, every publish feeds it its timeout, and the room's own terminal
//! drop latches it. The rule is shared with Tin Hat Web (src/features/calls/callHealth.ts), case by case
//! in testvectors/call-health/vectors.json.

/// A call reconnecting this long without a break is over (the SDK's own schedule, 10 attempts 5 s
/// apart, fits inside; the Wi-Fi freezes seen in the field, 14–51 s, still resume).
pub const RECONNECT_GIVE_UP_MS: u64 = 60_000;
/// The engine says Disconnected but no terminal drop reached the app this long after: say it ourselves.
pub const DISCONNECTED_GRACE_MS: u64 = 3_000;
/// A share / camera publish that hasn't finished by then never will: the call isn't connected.
pub const PUBLISH_TIMEOUT_MS: u64 = 20_000;

/// Status line when Share is pressed during a reconnect (desktop + web, same words).
pub const SHARE_WHILE_RECONNECTING: &str = "Can't share while the call is reconnecting";
/// Status line when a share publish timed out (desktop + web, same words); the call then ends visibly.
pub const SHARE_NOT_CONNECTED: &str = "Share failed: the call isn't connected";
/// The error a timed-out publish returns (the share path maps it to [`SHARE_NOT_CONNECTED`]).
pub const NOT_CONNECTED_ERR: &str = "the call isn't connected";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Connected,
    Reconnecting,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostWhy {
    ReconnectTooLong,
    EngineGaveUp,
    PublishTimedOut,
}

impl LostWhy {
    pub fn as_str(self) -> &'static str {
        match self {
            LostWhy::ReconnectTooLong => "reconnect-too-long",
            LostWhy::EngineGaveUp => "engine-gave-up",
            LostWhy::PublishTimedOut => "publish-timed-out",
        }
    }
}

/// The watch's state; times in ms on any monotonic clock.
#[derive(Debug, Default, Clone)]
pub struct LinkWatch {
    reconnecting_since: Option<u64>,
    disconnected_since: Option<u64>,
    done: bool,
}

impl LinkWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a verdict was given or the terminal drop came (nothing more will be said).
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// One observation of the room's connection state at `now`. Some = the call is lost (given once).
    pub fn observe(&mut self, now: u64, link: Link) -> Option<LostWhy> {
        if self.done {
            return None;
        }
        match link {
            Link::Connected => {
                self.reconnecting_since = None;
                self.disconnected_since = None;
                None
            }
            Link::Reconnecting => {
                self.disconnected_since = None;
                let since = *self.reconnecting_since.get_or_insert(now);
                self.verdict(now.saturating_sub(since) >= RECONNECT_GIVE_UP_MS, LostWhy::ReconnectTooLong)
            }
            Link::Disconnected => {
                self.reconnecting_since = None;
                let since = *self.disconnected_since.get_or_insert(now);
                self.verdict(now.saturating_sub(since) >= DISCONNECTED_GRACE_MS, LostWhy::EngineGaveUp)
            }
        }
    }

    /// A publish timed out: the call is lost at once (unless already over).
    pub fn publish_timed_out(&mut self) -> Option<LostWhy> {
        if self.done {
            return None;
        }
        self.verdict(true, LostWhy::PublishTimedOut)
    }

    /// The room's own terminal drop reached the app. True = this is the first word on it (send the terminal
    /// state); false = the watch already said so (drop the duplicate).
    pub fn terminal(&mut self) -> bool {
        !std::mem::replace(&mut self.done, true)
    }

    fn verdict(&mut self, lost: bool, why: LostWhy) -> Option<LostWhy> {
        if lost {
            self.done = true;
            Some(why)
        } else {
            None
        }
    }
}

/// The watch's clock: ms since the first call to it (monotonic; one epoch for the watchdog and the room's events).
pub fn now_ms() -> u64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// Share pressed: refuse during a reconnect (the publish would only wait on it). None = go ahead.
pub fn share_refusal(reconnecting: bool) -> Option<&'static str> {
    reconnecting.then_some(SHARE_WHILE_RECONNECTING)
}

/// A share error as the status line shows it after "Share failed: ". A publish that timed out because the call isn't
/// connected keeps its own words ([`SHARE_NOT_CONNECTED`]); anything else keeps the old "share: …" text.
pub fn share_error_text(e: &anyhow::Error) -> String {
    if e.chain().any(|c| c.to_string() == NOT_CONNECTED_ERR) {
        NOT_CONNECTED_ERR.to_string()
    } else {
        format!("share: {e}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> serde_json::Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testvectors/call-health/vectors.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// The rule, case by case from the vectors Tin Hat Web runs too (testvectors/call-health).
    #[test]
    fn call_health_matches_the_shared_vectors() {
        let v = vectors();
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 12);
        for c in cases {
            let mut w = LinkWatch::new();
            let mut got: Option<(u64, &str)> = None;
            for s in c["steps"].as_array().unwrap() {
                let t = s["t"].as_u64().unwrap();
                let r = if s["publishTimedOut"].as_bool() == Some(true) {
                    w.publish_timed_out()
                } else if s["terminal"].as_bool() == Some(true) {
                    w.terminal();
                    None
                } else {
                    let link = match s["link"].as_str().unwrap() {
                        "connected" => Link::Connected,
                        "reconnecting" => Link::Reconnecting,
                        "disconnected" => Link::Disconnected,
                        o => panic!("{o}"),
                    };
                    w.observe(t, link)
                };
                if let (Some(why), None) = (r, got) {
                    got = Some((t, why.as_str()));
                } else {
                    assert!(r.is_none() || got.is_none(), "{}: a second verdict", c["name"]);
                }
            }
            let want = if c["lost"].is_null() { None } else { Some((c["lost"]["t"].as_u64().unwrap(), c["lost"]["why"].as_str().unwrap())) };
            assert_eq!(got, want, "{}", c["name"]);
        }
    }

    /// The constants and words are the ones the web uses (both read from the same file).
    #[test]
    fn constants_and_words_match_the_vectors() {
        let v = vectors();
        assert_eq!(v["constants"]["RECONNECT_GIVE_UP_MS"].as_u64(), Some(RECONNECT_GIVE_UP_MS));
        assert_eq!(v["constants"]["DISCONNECTED_GRACE_MS"].as_u64(), Some(DISCONNECTED_GRACE_MS));
        assert_eq!(v["constants"]["PUBLISH_TIMEOUT_MS"].as_u64(), Some(PUBLISH_TIMEOUT_MS));
        assert_eq!(v["words"]["shareWhileReconnecting"].as_str(), Some(SHARE_WHILE_RECONNECTING));
        assert_eq!(v["words"]["shareNotConnected"].as_str(), Some(SHARE_NOT_CONNECTED));
        assert_eq!(SHARE_NOT_CONNECTED, format!("Share failed: {NOT_CONNECTED_ERR}"));
    }

    #[test]
    fn share_error_words() {
        let e = anyhow::anyhow!(NOT_CONNECTED_ERR).context("publish screenshare failed");
        assert_eq!(format!("Share failed: {}", share_error_text(&e)), SHARE_NOT_CONNECTED);
        let e = anyhow::anyhow!("portal closed");
        assert_eq!(share_error_text(&e), "share: portal closed");
    }

    #[test]
    fn share_refused_only_while_reconnecting() {
        assert_eq!(share_refusal(true), Some(SHARE_WHILE_RECONNECTING));
        assert_eq!(share_refusal(false), None);
    }

    /// The terminal latch answers true once (send), then false (the duplicate is dropped).
    #[test]
    fn terminal_latches_once() {
        let mut w = LinkWatch::new();
        assert!(w.terminal());
        assert!(!w.terminal());
        let mut w = LinkWatch::new();
        assert_eq!(w.publish_timed_out(), Some(LostWhy::PublishTimedOut));
        assert!(!w.terminal(), "the watch already ended the call: the room's own drop is a duplicate");
    }
}
