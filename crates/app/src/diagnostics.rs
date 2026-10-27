//! Diagnostics — an OPT-IN, redacted, user-inspectable bug report.
//!
//! Design constraints (the owner does security for a living, and his friends should not have
//! to take his word for any of this):
//!   * OFF by default. Nothing is ever collected, written, or sent unless the user turns it
//!     on AND presses the button. There is no background collection and no auto-upload.
//!   * NOTHING LEAVES THE MACHINE on its own. The report is written to a FILE that the user
//!     can open and read in full before deciding to share it. Sharing = they attach it in
//!     chat, inside their own E2EE server. It never goes to a third party, and there is no
//!     network code in this module at all.
//!   * NO SYSTEM SCANNING. We do not enumerate processes, windows, or games — only this
//!     app's own log plus the audio/video stack versions needed to debug a call.
//!   * REDACTED. Access tokens, JWTs, media keys and recovery keys are stripped before the
//!     file is written (see [`redact`]), so a report can be pasted anywhere without leaking
//!     the user's session. Message text is never in the log to begin with.

use core::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(bool, enabled)] //      QML: diagnostics.enabled — opt-in, default OFF
        #[qproperty(QString, status)] //    QML: last action's result (path, or an error)
        #[qproperty(QString, report_path)] // QML: the file we just wrote (empty until then)
        type DiagnosticsController = super::DiagnosticsControllerRust;

        /// QML: diagnostics.setEnabled(on) — persist the opt-in.
        #[qinvokable]
        fn set_opt_in(self: Pin<&mut DiagnosticsController>, on: bool);
        /// QML: diagnostics.createReport() — write a REDACTED report next to the log and
        /// expose its path. No network access; the user chooses whether to share it.
        #[qinvokable]
        fn create_report(self: Pin<&mut DiagnosticsController>);
        /// QML: diagnostics.refresh() — mirror the persisted opt-in into `enabled`.
        #[qinvokable]
        fn refresh(self: Pin<&mut DiagnosticsController>);
    }

    impl cxx_qt::Threading for DiagnosticsController {}
}

#[derive(Default)]
pub struct DiagnosticsControllerRust {
    enabled: bool,
    status: QString,
    report_path: QString,
}

fn opt_in_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/diagnostics-opt-in")
}

fn opted_in() -> bool {
    std::fs::read_to_string(opt_in_path()).map(|s| s.trim() == "1").unwrap_or(false)
}

/// Strip anything that could impersonate the user or decrypt their media. Deliberately
/// aggressive: it is better to redact a harmless string than to leak a session.
pub(crate) fn redact(input: &str) -> String {
    // Security review 2026-10-26 (Low): two shapes the token rule below let through —
    //  * a secret-looking URL parameter (`sig=`, `token=`, `key=`, …): its value goes whatever it looks like (the
    //    Memories file links carry a lowercase-hex `sig`, and a short secret has no length to give it away);
    //  * a long run of hex digits (32+: a SHA-256 / HMAC / 128-bit token in lowercase hex has no uppercase letter).
    static PARAM_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static HEX_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let param_re = PARAM_RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)([?&;](?:sig|signature|token|access_token|refresh_token|key|apikey|api_key|secret|auth|code|password|pw|pass|hmac|jwt|login_token|loginToken)=)[^&\s#\x22'<>]+",
        )
        .unwrap()
    });
    let hex_re = HEX_RE.get_or_init(|| regex::Regex::new(r"\b[0-9a-fA-F]{32,}\b").unwrap());
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        let line = param_re.replace_all(line, "${1}<REDACTED>");
        let line = hex_re.replace_all(&line, "<REDACTED>").into_owned();
        let line = line.as_str();
        let mut l = line.to_string();
        // Long opaque blobs: JWTs (eyJ...), access tokens, base64 keys. The charset
        // INCLUDES `+` and `/`: STANDARD base64 (curve25519 keys, olm blobs) uses them,
        // and the old url-safe-only charset let every such blob through — a would-be
        // secret containing `+`/`/` shipped verbatim (2026-08-20 audit). `/` is only
        // accepted for tokens that don't look like paths/URLs (no `:`-bearing token
        // reaches here anyway, so https:// stays intact; filesystem paths are excluded
        // by the digit+uppercase requirement in almost all cases and are not secrets).
        for tok in line.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '=') {
            let t = tok
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '+' && c != '/');
            let looks_secret = t.len() >= 24
                && t.chars().all(|c| {
                    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '+' || c == '/'
                })
                && t.chars().any(|c| c.is_ascii_digit())
                && t.chars().any(|c| c.is_ascii_uppercase());
            if looks_secret || t.starts_with("eyJ") || t.starts_with("syt_") {
                l = l.replace(t, "<REDACTED>");
            }
        }
        let lower = l.to_lowercase();
        for key in ["access_token", "password", "secret", "jwt", "recovery", "media key", "sender_key"] {
            if lower.contains(key) {
                // Keep the fact that the line happened; drop everything after the marker.
                if let Some(i) = lower.find(key) {
                    l.truncate(i + key.len());
                    l.push_str(" <REDACTED>");
                }
            }
        }
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// Versions of the media stack — the things that actually explain a broken call. Read from
/// package/CLI version output only; no process or window enumeration.
fn environment() -> String {
    let run = |cmd: &str, args: &[&str]| -> String {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").trim().to_string())
            .unwrap_or_default()
    };
    let mut s = String::new();
    s.push_str(&format!("app            : {} ({})\n", env!("CARGO_PKG_VERSION"), option_env!("TINHAT_BUILD_HASH").unwrap_or("dev")));
    s.push_str(&format!("kernel         : {}\n", run("uname", &["-r"])));
    s.push_str(&format!("gstreamer      : {}\n", run("gst-launch-1.0", &["--version"])));
    s.push_str(&format!("pipewire       : {}\n", run("pipewire", &["--version"])));
    s.push_str(&format!("nvidia driver  : {}\n", run("nvidia-smi", &["--query-gpu=driver_version", "--format=csv,noheader"])));
    s.push_str(&format!("gpu            : {}\n", run("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"])));
    s.push_str(&format!(
        "session type   : {}\n",
        std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "?".into())
    ));
    s
}

/// The user's call-relevant settings — these decide half of all "it's broken" reports.
fn settings_summary() -> String {
    format!(
        "echo cancellation : {}\nauto gain control : {}\nnoise suppression : {}\nRNNoise denoise   : {}\nshare quality     : {}\ncamera quality    : {}\nincoming video    : {}\nvoice overlay     : {}\n",
        tinhat_streamer::aec_enabled(),
        tinhat_streamer::mic_agc(),
        tinhat_streamer::mic_noise_suppression(),
        tinhat_streamer::mic_denoise(),
        tinhat_streamer::share_quality_label(),
        tinhat_streamer::camera::cam_quality_label(),
        tinhat_streamer::call::recv_quality(),
        crate::settings::overlay_summary(),
    )
}

impl qobject::DiagnosticsController {
    pub fn set_opt_in(mut self: Pin<&mut Self>, on: bool) {
        let p = opt_in_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&p, if on { "1" } else { "0" });
        self.as_mut().set_enabled(on);
        self.as_mut().set_status(QString::from(if on {
            "Diagnostics ON. Nothing is sent automatically — press “Create report” to write a file you can read and share."
        } else {
            "Diagnostics OFF. Nothing is collected."
        }));
    }

    pub fn refresh(mut self: Pin<&mut Self>) {
        let on = opted_in();
        self.as_mut().set_enabled(on);
    }

    pub fn create_report(mut self: Pin<&mut Self>) {
        if !opted_in() {
            self.as_mut()
                .set_status(QString::from("Turn diagnostics on first — nothing is collected while it's off."));
            return;
        }
        let log = crate::platform::live_log_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        // Only the tail matters, and a smaller file is easier for the user to actually read.
        let tail: String = {
            let lines: Vec<&str> = log.lines().collect();
            let start = lines.len().saturating_sub(3000);
            lines[start..].join("\n")
        };
        let body = format!(
            "Tin Hat diagnostic report\n\
             =============================\n\
             This file was created because YOU pressed the button. It has not been sent\n\
             anywhere. Read it, then share it however you like (e.g. attach it in chat).\n\
             Tokens, keys and recovery phrases are redacted. Message content is never logged.\n\n\
             -- environment ------------------------------------------------\n{}\n\
             -- call settings ----------------------------------------------\n{}\n\
             -- app log (last 3000 lines, redacted) ------------------------\n{}\n",
            environment(),
            settings_summary(),
            redact(&tail),
        );
        let dir = crate::platform::home_dir();
        let path = dir.join("tinhat-diagnostics.txt");   // 10.1 (TH-PKG-16)
        match std::fs::write(&path, body) {
            Ok(_) => {
                // QML builds "file://" + reportPath — keep the property in the
                // prefixable form (plain path on unix, /C:/-style on Windows).
                let p = crate::platform::qml_file_prefix_path(&path);
                self.as_mut().set_report_path(QString::from(&p));
                self.as_mut().set_status(QString::from(&format!(
                    "Report written to {p} — open it, then send it to your server's owner in chat if you're happy with it."
                )));
                eprintln!("[diag] report written to {p}");
            }
            Err(e) => {
                self.as_mut().set_status(QString::from(&format!("Couldn't write the report: {e}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn lowercase_hex_secrets_and_secret_url_parameters_are_redacted() {
        let sig = "9f2c4e1a7b3d5f60817263544a5b6c7d8e9f0a1b2c3d4e5f60718293a4b5c6d7";
        let line = format!("[vods] file https://chat.example.net/api/vods/file?id=abc&exp=1790000000&sig={sig} ok");
        let r = redact(&line);
        assert!(!r.contains(sig), "{r}");
        assert!(r.contains("sig=<REDACTED>"), "{r}");
        assert!(r.contains("id=abc"), "an ordinary parameter stays: {r}");
        // a short secret behind a secret-looking name, any case
        assert!(!redact("GET /x?token=abc123&v=1").contains("abc123"));
        assert!(redact("GET /x?token=abc123&v=1").contains("v=1"));
        assert!(!redact("https://x/y?KEY=s3cr3t").contains("s3cr3t"));
        // a bare 32+ hex run (a sha256, an HMAC, a 128-bit token) anywhere in a line
        assert!(!redact(&format!("checksum mismatch (expected {sig}, got 00)")).contains(sig));
        assert!(!redact("guest key 0123456789abcdef0123456789abcdef").contains("0123456789abcdef0123456789abcdef"));
        // what stays readable: ordinary words, short hex (a build hash), a YouTube id, the host
        let ok = redact("[update] build 3e8e3ce5 · watch?v=dQw4w9WgXcQ on chat.example.net");
        assert!(ok.contains("3e8e3ce5") && ok.contains("dQw4w9WgXcQ") && ok.contains("chat.example.net"), "{ok}");
        // the earlier rules still hold
        assert!(!redact("Authorization: Bearer syt_abc_def").contains("syt_abc_def"));
        assert!(redact("access_token=xyz").ends_with("<REDACTED>\n"));
    }
}
