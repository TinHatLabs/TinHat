//! What the self-updater believes (security review 2026-10-26, H3) — PURE: no network, no Qt. update.rs does the
//! fetching and calls these rules; they are tested here on fixed manifests and signatures (`testvectors/update/`).
//!
//! Before 12.0.3 the only anchor of an update was the sha256 inside `latest.json`, and both came from the same GitHub
//! release: whoever could write a release could ship code to every install. Now a feed (`latest.json` on Linux,
//! `latest-windows.json` on Windows, update.rs) counts only when
//!   1. `<feed>.minisig` is a minisign signature (Ed25519 over the BLAKE2b-512 of exactly the bytes fetched) by a key
//!      this build trusts ([`trusted_keys`]: the Tin Hat release key, the key Tin Hat Server's updater trusts too);
//!   2. its trusted comment reads `tinhat-desktop <feed file> <version>` ([`trusted_comment`]): the signer said which
//!      platform's feed and which version, so a Windows feed can't stand in for the Linux one, nor an old one for another;
//!   3. the download it names sits where that release's files sit ([`download_allowed`]): under the feed's GitHub
//!      release tag `v<version>`, or — for a feed somewhere else (a mirror, or the `TINHAT_UPDATE_URL` test override) —
//!      in the feed's own folder on the feed's own host; https only (a test override may be plain http);
//!   4. every redirect is https with no user name, at most 5 of them ([`redirect_allowed`]): for a GitHub feed to any
//!      host (GitHub has moved its download host before; the signature and the sha256 pin the content, not the host),
//!      for any other feed only on the feed's own origin;
//!   5. it is newer than the running build (update.rs `is_upgrade`: no downgrade, no re-install).
//! The payload's sha256 (inside the signed manifest) is still checked before a byte is written, as before.
//!
//! The feed's own address stays pinned (update.rs `UPDATE_FEED_URL`): **never rename or transfer the `TinHatLabs/TinHat`
//! repository without a bridge release** (one that still reads the old path and moves installs to the new one) — a
//! verifying build only takes downloads from the `v<version>` release of the repository its feed names, so a renamed
//! repository strands every install from 12.0.3 on behind a manual reinstall.
//!
//! Keys: [`RELEASE_KEY`] is compiled in, and [`BACKUP_KEY`] is the (empty) slot of an offline backup key. A LAB build may add ONE development key when it is built: the
//! `lab-update-key` cargo feature together with `TINHAT_UPDATE_DEV_KEY=<base64 public key>` (build.rs refuses either
//! alone, and the feature when `TINHAT_RELEASE_BUILD=1`). Such a binary carries `LAB_TRUST_MARKER` and says so in its
//! log at every check; the publish step refuses a binary with the marker or with any minisign public key other than the
//! release key. Nothing at run time can add a key.

use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};
use url::Url;

/// The Tin Hat release key (minisign key id 2B53E5F91E971B14) — the same public key Tin Hat Server's updater trusts
/// (`keys/tinhat-release.pub` there). Its secret half never leaves the release machine.
pub const RELEASE_KEY: &str = "RWQUG5ce+eVTK+HZgYoCViu7WY1nwnbLR8s54/zRQR0DLiACjVuh5Mkv";

/// A development key a LAB build was given when it was built (never a release; see the module doc): only with the
/// `lab-update-key` feature, whose build.rs check also requires the key and refuses `TINHAT_RELEASE_BUILD=1`. Without
/// the feature the variable isn't even read (security review follow-up: `option_env!` alone applied to every build).
#[cfg(feature = "lab-update-key")]
const DEV_KEY: Option<&str> = Some(env!("TINHAT_UPDATE_DEV_KEY"));
#[cfg(not(feature = "lab-update-key"))]
const DEV_KEY: Option<&str> = None;

/// The fixed text every lab binary carries and no other does: the publish step refuses any binary that contains it,
/// whatever development key it trusts (it needn't know the key).
#[cfg(feature = "lab-update-key")]
#[used]
pub static LAB_TRUST_MARKER: &str = "TINHAT-LAB-BUILD: this build trusts a development update key - never publish it";

/// The largest feed we read (today's are ~500 bytes) and the largest signature file (~300 bytes).
pub const MANIFEST_MAX: usize = 64 * 1024;
pub const SIG_MAX: usize = 4 * 1024;

/// The slot of an offline BACKUP release key (launch-gate's review #3: if the release key is ever lost, installs could
/// otherwise never update again without a manual reinstall). EMPTY on purpose — the maintainer's decision, 2026-10-26: no backup key for
/// now, the app ships trusting the one release key. When one is made (kept offline; its public half also goes into Tin
/// Hat Server's `keys/` and the publish step's list of allowed keys), its base64 public key goes here as `Some("RW…")`.
pub const BACKUP_KEY: Option<&str> = None;

/// The keys this build accepts a feed from: the release key, the backup key when the slot holds one, plus a lab
/// build's development key.
pub fn trusted_keys() -> Vec<&'static str> {
    keys_from(RELEASE_KEY, BACKUP_KEY, dev_key())
}

fn keys_from<'a>(release: &'a str, backup: Option<&'a str>, dev: Option<&'a str>) -> Vec<&'a str> {
    let mut keys = vec![release];
    keys.extend(backup.map(str::trim).filter(|k| !k.is_empty()));
    keys.extend(dev);
    keys
}

/// The development key this build was given, if any (a lab build; the log says so at every check).
pub fn dev_key() -> Option<&'static str> {
    DEV_KEY.map(str::trim).filter(|k| !k.is_empty())
}

/// The lab marker of this build ("" in every other build) — printed with the development-key warning, so it is in the
/// binary for the publish step to find.
pub fn lab_marker() -> &'static str {
    #[cfg(feature = "lab-update-key")]
    {
        LAB_TRUST_MARKER
    }
    #[cfg(not(feature = "lab-update-key"))]
    {
        ""
    }
}

/// The id minisign prints for a public key ("minisign public key 2B53E5F91E971B14"), or None for a bad key.
pub fn key_id(b64: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?;
    if raw.len() != 42 || &raw[..2] != b"Ed" {
        return None;
    }
    Some(raw[2..10].iter().rev().map(|b| format!("{b:02X}")).collect())
}

/// The feed file of a platform: what the trusted comment names.
pub fn feed_role(windows: bool) -> &'static str {
    if windows {
        "latest-windows.json"
    } else {
        "latest.json"
    }
}

/// The trusted comment the publish step signs a feed with: `tinhat-desktop <feed file> <version>`.
pub fn trusted_comment(role: &str, version: &str) -> String {
    format!("tinhat-desktop {role} {version}")
}

/// Why a feed or a download was refused. [`Refusal::message`] is what the update banner says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No `.minisig` next to the feed (or an empty one).
    Unsigned,
    /// A signature that doesn't verify with any trusted key (wrong key, altered feed, an old-format signature).
    BadSignature,
    /// Signed, but for another feed file or another version than the feed says.
    SignedForSomethingElse(String),
    /// Signed, but the content isn't a usable feed.
    Malformed(String),
    /// Signed, but the download it names isn't where Tin Hat releases are.
    Download(String),
}

impl Refusal {
    /// What the banner says, for this build's channel (update.rs `channel()`).
    pub fn message(&self) -> String {
        self.message_in(crate::update::channel())
    }
    /// The banner's sentence for an install on `channel` ("flatpak" | "pacman-final" | ""): the way out it names is the
    /// one that install actually has (launch-gate's review: a Flatpak is updated by Discover / the software centre,
    /// never by downloading Tin Hat again).
    pub fn message_in(&self, channel: &str) -> String {
        let hint = match channel {
            "flatpak" => "if this keeps happening, update Tin Hat from Discover or your software centre",
            _ => "if this keeps happening, download Tin Hat again from tinhat.dev",
        };
        match self {
            Refusal::Unsigned => format!("the update information isn't signed by Tin Hat, so it was ignored ({hint})"),
            Refusal::BadSignature => format!("the update information failed its signature check, so it was ignored ({hint})"),
            Refusal::SignedForSomethingElse(why) => format!("the update information doesn't match its signature ({why}), so it was ignored"),
            Refusal::Malformed(why) => format!("the update information is unusable ({why})"),
            Refusal::Download(why) => format!("{why} — refusing it"),
        }
    }
}

/// Check `body` (the feed's bytes, exactly as fetched) against `sig` (its `.minisig` text) with `keys`, then the
/// trusted comment against `role` and the version inside. Returns the parsed feed. Nothing in the feed is read before
/// the signature checks out.
pub fn verify_manifest(body: &[u8], sig: &str, role: &str, keys: &[&str]) -> Result<serde_json::Value, Refusal> {
    if sig.trim().is_empty() {
        return Err(Refusal::Unsigned);
    }
    if body.len() > MANIFEST_MAX || sig.len() > SIG_MAX {
        return Err(Refusal::Malformed("too big".into()));
    }
    let signature = Signature::decode(sig).map_err(|_| Refusal::BadSignature)?;
    let signed = keys.iter().any(|k| {
        PublicKey::from_base64(k.trim())
            // allow_legacy = false: only the prehashed format minisign 0.10+ makes; an old-format signature is refused
            .map(|pk| pk.verify(body, &signature, false).is_ok())
            .unwrap_or(false)
    });
    if !signed {
        return Err(Refusal::BadSignature);
    }
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| Refusal::Malformed("not JSON".into()))?;
    let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    if !plain_version(&version) {
        return Err(Refusal::Malformed("no usable version".into()));
    }
    let want = trusted_comment(role, &version);
    let got = signature.trusted_comment().trim();
    if got != want {
        return Err(Refusal::SignedForSomethingElse(format!("signed as \"{got}\", expected \"{want}\"")));
    }
    Ok(v)
}

/// A version as release tags carry it: digits, letters, `.`, `-`, `+` (and not empty).
fn plain_version(v: &str) -> bool {
    !v.is_empty() && v.len() <= 32 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'+')
}

/// A release file name: letters, digits, `.`, `_`, `-`, `+`; not hidden, not `..`.
fn plain_file_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && !n.starts_with('.')
        && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' || b == b'+')
}

/// `(owner, repo)` when `feed` is a GitHub release feed: `https://github.com/<owner>/<repo>/releases/latest/download/<file>`
/// or `…/releases/download/<tag>/<file>`.
fn github_release(feed: &Url) -> Option<(String, String)> {
    if feed.scheme() != "https" || feed.host_str() != Some("github.com") || feed.port().is_some() {
        return None;
    }
    let segs: Vec<&str> = feed.path_segments()?.collect();
    let ok = match segs.as_slice() {
        [_, _, "releases", "latest", "download", f] => !f.is_empty(),
        [_, _, "releases", "download", t, f] => !t.is_empty() && !f.is_empty(),
        _ => false,
    };
    if !ok || segs[0].is_empty() || segs[1].is_empty() {
        return None;
    }
    Some((segs[0].to_string(), segs[1].to_string()))
}

/// May the payload be fetched from `download`? The feed (`feed`, as configured) said `version`; `overridden` = the
/// feed is the `TINHAT_UPDATE_URL` test override (the only case where plain http is allowed, on the feed's own host).
pub fn download_allowed(download: &str, feed: &str, version: &str, overridden: bool) -> Result<(), Refusal> {
    let refuse = |why: String| Err(Refusal::Download(why));
    let Ok(d) = Url::parse(download.trim()) else {
        return refuse(format!("the update names a download that isn't a web address ({download})"));
    };
    let Ok(f) = Url::parse(feed.trim()) else {
        return refuse("the update feed's own address is unusable".into());
    };
    let https = d.scheme() == "https";
    let test_http = overridden && d.scheme() == "http" && f.scheme() == "http";
    if !(https || test_http) {
        return refuse(format!("the update names a download that isn't https ({download})"));
    }
    if !d.username().is_empty() || d.password().is_some() || d.query().is_some() || d.fragment().is_some() {
        return refuse(format!("the update names an unusual download address ({download})"));
    }
    if !plain_version(version) {
        return refuse("the update has no usable version".into());
    }
    let name = d.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
    if !plain_file_name(name) {
        return refuse(format!("the update names an unusual file ({download})"));
    }
    match github_release(&f) {
        Some((owner, repo)) => {
            let want = format!("/{owner}/{repo}/releases/download/v{version}/{name}");
            if d.host_str() != Some("github.com") || d.port().is_some() || d.path() != want {
                return refuse(format!(
                    "the update names a download outside the v{version} release of {owner}/{repo} on GitHub ({download})"
                ));
            }
        }
        None => {
            let folder = &f.path()[..f.path().rfind('/').map(|i| i + 1).unwrap_or(0)];
            if d.origin() != f.origin() || d.path() != format!("{folder}{name}") {
                return refuse(format!("the update names a download outside its own folder ({download})"));
            }
        }
    }
    Ok(())
}

/// May a fetch of the feed or the payload follow a redirect to `to`? Never with a user name/password. For a GitHub
/// release feed: any https address (launch-gate's review #2: GitHub has moved its download host before — S3, then
/// objects.githubusercontent.com, then release-assets.… — and a fixed list would strand every install behind a manual
/// reinstall the next time; the host adds nothing to the trust, the signature and the sha256 pin the bytes). The feed's
/// own address stays pinned (update.rs), and update.rs follows at most 5 redirects. Any other feed (a mirror, the test
/// override): its own origin only, https unless it is the http test feed.
pub fn redirect_allowed(to: &Url, feed: &Url, overridden: bool) -> bool {
    if !to.username().is_empty() || to.password().is_some() {
        return false;
    }
    let https = to.scheme() == "https";
    if to.origin() == feed.origin() && (https || overridden) {
        return true;
    }
    https && to.host_str().is_some_and(|h| !h.is_empty()) && github_release(feed).is_some()
}

/// A payload sha256 as the feed must carry it: 64 hex digits.
pub fn plain_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    //! The fixtures in testvectors/update/ were made with two throwaway minisign keys (`test.pub`, `other.pub`) whose
    //! secret halves were destroyed after signing: the manifests are publish-desktop-github's shapes for a 12.0.3.
    use super::*;

    const TEST_KEY_FILE: &str = include_str!("../testvectors/update/test.pub");
    const OTHER_KEY_FILE: &str = include_str!("../testvectors/update/other.pub");
    const LATEST: &[u8] = include_bytes!("../testvectors/update/latest.json");
    const LATEST_SIG: &str = include_str!("../testvectors/update/latest.json.minisig");
    const LATEST_SIG_OTHER_KEY: &str = include_str!("../testvectors/update/latest.json.other-key.minisig");
    const LATEST_SIG_LEGACY: &str = include_str!("../testvectors/update/latest.json.legacy.minisig");
    const LATEST_SIG_WIN_COMMENT: &str = include_str!("../testvectors/update/latest.json.windows-comment.minisig");
    const WINDOWS: &[u8] = include_bytes!("../testvectors/update/latest-windows.json");
    const WINDOWS_SIG: &str = include_str!("../testvectors/update/latest-windows.json.minisig");
    const FOREIGN: &[u8] = include_bytes!("../testvectors/update/latest-foreign.json");
    const FOREIGN_SIG: &str = include_str!("../testvectors/update/latest-foreign.json.minisig");
    const OLDTAG: &[u8] = include_bytes!("../testvectors/update/latest-oldtag.json");
    const OLDTAG_SIG: &str = include_str!("../testvectors/update/latest-oldtag.json.minisig");
    const OLD: &[u8] = include_bytes!("../testvectors/update/latest-old.json");
    const OLD_SIG: &str = include_str!("../testvectors/update/latest-old.json.minisig");
    const FEED: &str = "https://github.com/TinHatLabs/TinHat/releases/latest/download/latest.json";
    const FEED_WIN: &str = "https://github.com/TinHatLabs/TinHat/releases/latest/download/latest-windows.json";

    fn key(file: &str) -> &str {
        file.lines().nth(1).unwrap().trim()
    }
    fn test_key() -> &'static str {
        key(TEST_KEY_FILE)
    }
    fn s(v: &serde_json::Value, k: &str) -> String {
        v[k].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn the_release_key_is_tin_hats_and_a_release_build_trusts_only_it() {
        assert_eq!(key_id(RELEASE_KEY).as_deref(), Some("2B53E5F91E971B14"));
        assert!(PublicKey::from_base64(RELEASE_KEY).is_ok());
        // the fixtures' keys are what their files say they are (minisign's own header line)
        assert!(TEST_KEY_FILE.starts_with(&format!("untrusted comment: minisign public key {}", key_id(test_key()).unwrap())));
        // a build without the lab feature (every release) trusts exactly the release key and carries no marker
        #[cfg(not(feature = "lab-update-key"))]
        {
            assert_eq!(dev_key(), None);
            assert_eq!(trusted_keys(), vec![RELEASE_KEY]);
            assert_eq!(lab_marker(), "");
        }
        #[cfg(feature = "lab-update-key")]
        {
            assert_eq!(trusted_keys().len(), 2);
            assert_eq!(trusted_keys()[0], RELEASE_KEY);
            assert!(lab_marker().starts_with("TINHAT-LAB-BUILD"));
        }
        assert_eq!(key_id("not a key"), None);
    }

    #[test]
    fn a_good_signature_gives_the_feed() {
        let v = verify_manifest(LATEST, LATEST_SIG, "latest.json", &[test_key()]).unwrap();
        assert_eq!(s(&v, "version"), "12.0.3");
        assert!(s(&v, "url_bin").ends_with("/v12.0.3/tinhat-12.0.3-x86_64.bin"));
        let w = verify_manifest(WINDOWS, WINDOWS_SIG, "latest-windows.json", &[test_key()]).unwrap();
        assert_eq!(s(&w, "version"), "12.0.3");
        // any one trusted key is enough (a lab build: release key + its development key)
        assert!(verify_manifest(LATEST, LATEST_SIG, "latest.json", &[RELEASE_KEY, test_key()]).is_ok());
    }

    #[test]
    fn a_missing_signature_is_refused() {
        assert_eq!(verify_manifest(LATEST, "", "latest.json", &[test_key()]), Err(Refusal::Unsigned));
        assert_eq!(verify_manifest(LATEST, " \n", "latest.json", &[test_key()]), Err(Refusal::Unsigned));
        assert!(Refusal::Unsigned.message().contains("isn't signed"));
    }

    /// Launch-gate's review: the way out the banner names is the one the install has — a Flatpak is updated through
    /// Discover / the software centre, never by downloading Tin Hat again.
    #[test]
    fn the_refusal_names_the_way_out_this_install_has() {
        for r in [Refusal::Unsigned, Refusal::BadSignature] {
            let fp = r.message_in("flatpak");
            assert!(fp.contains("Discover or your software centre") && !fp.contains("tinhat.dev"), "{fp}");
            for ch in ["", "pacman-final"] {
                let m = r.message_in(ch);
                assert!(m.contains("download Tin Hat again from tinhat.dev") && !m.contains("Discover"), "{m}");
            }
        }
        assert_eq!(Refusal::Download("x".into()).message_in("flatpak"), "x — refusing it");
    }

    /// Launch-gate's review #3 + the maintainer's decision (2026-10-26): the backup-key slot exists and is EMPTY (the build trusts the one
    /// release key); a key put in the slot is trusted like the release key.
    #[test]
    fn the_backup_key_slot_is_empty_and_a_key_in_it_verifies() {
        assert_eq!(BACKUP_KEY, None, "decided 2026-10-26: no backup key for now");
        assert!(!trusted_keys().contains(&test_key()));
        // the slot filled with the fixtures' key: a feed signed by it verifies…
        let with_backup = keys_from(RELEASE_KEY, Some(test_key()), None);
        assert_eq!(with_backup, vec![RELEASE_KEY, test_key()]);
        let v = verify_manifest(LATEST, LATEST_SIG, "latest.json", &with_backup).unwrap();
        assert_eq!(s(&v, "version"), "12.0.3");
        // …and empty (None, or a blank key) it adds nothing: the same feed is refused
        for empty in [None, Some(""), Some("  ")] {
            let keys = keys_from(RELEASE_KEY, empty, None);
            assert_eq!(keys, vec![RELEASE_KEY]);
            assert_eq!(verify_manifest(LATEST, LATEST_SIG, "latest.json", &keys), Err(Refusal::BadSignature));
        }
        // a lab build's development key still comes last
        assert_eq!(keys_from(RELEASE_KEY, Some(test_key()), Some("D")), vec![RELEASE_KEY, test_key(), "D"]);
    }

    #[test]
    fn a_bad_or_garbled_signature_is_refused() {
        // one character inside the Ed25519 signature changed (base64 char 40 = byte 30: past the algorithm and key id)
        let mut lines: Vec<String> = LATEST_SIG.lines().map(str::to_string).collect();
        let mut c: Vec<char> = lines[1].chars().collect();
        c[40] = if c[40] == 'A' { 'B' } else { 'A' };
        lines[1] = c.into_iter().collect();
        let bad = lines.join("\n");
        assert!(Signature::decode(&bad).is_ok(), "still a well-formed signature file");
        assert_eq!(verify_manifest(LATEST, &bad, "latest.json", &[test_key()]), Err(Refusal::BadSignature));
        assert_eq!(verify_manifest(LATEST, "untrusted comment: x\nnot base64\n", "latest.json", &[test_key()]), Err(Refusal::BadSignature));
        // the trusted comment edited (the global signature covers it)
        let edited = LATEST_SIG.replace("tinhat-desktop latest.json 12.0.3", "tinhat-desktop latest.json 12.9.9");
        assert_eq!(verify_manifest(LATEST, &edited, "latest.json", &[test_key()]), Err(Refusal::BadSignature));
        // the old (non-prehashed) signature format is not accepted
        assert_eq!(verify_manifest(LATEST, LATEST_SIG_LEGACY, "latest.json", &[test_key()]), Err(Refusal::BadSignature));
    }

    #[test]
    fn a_signature_by_another_key_is_refused() {
        // signed by a key this build doesn't trust
        assert_eq!(verify_manifest(LATEST, LATEST_SIG_OTHER_KEY, "latest.json", &[test_key()]), Err(Refusal::BadSignature));
        // the real release build: the fixtures' key isn't trusted at all
        assert_eq!(verify_manifest(LATEST, LATEST_SIG, "latest.json", &[RELEASE_KEY]), Err(Refusal::BadSignature));
        // ...and with the other key trusted, the test key's signature is the stranger
        assert_eq!(verify_manifest(LATEST, LATEST_SIG, "latest.json", &[key(OTHER_KEY_FILE)]), Err(Refusal::BadSignature));
        assert!(verify_manifest(LATEST, LATEST_SIG_OTHER_KEY, "latest.json", &[key(OTHER_KEY_FILE)]).is_ok());
    }

    #[test]
    fn a_tampered_feed_is_refused() {
        // any change to the bytes: the payload's checksum, the url, even whitespace
        let text = std::str::from_utf8(LATEST).unwrap();
        for t in [
            text.replacen(&"a".repeat(64), &format!("b{}", "a".repeat(63)), 1),
            text.replace("github.com/TinHatLabs/TinHat", "github.com/TinHatLabs/TinHax"),
            format!("{text}\n"),
            text.replace("\"12.0.3\"", "\"12.0.4\""),
        ] {
            assert_ne!(t.as_bytes(), LATEST);
            assert_eq!(verify_manifest(t.as_bytes(), LATEST_SIG, "latest.json", &[test_key()]), Err(Refusal::BadSignature), "{t}");
        }
    }

    #[test]
    fn a_feed_signed_for_the_other_platform_or_version_is_refused() {
        // the Windows feed's bytes and signature offered as the Linux feed
        assert!(matches!(
            verify_manifest(WINDOWS, WINDOWS_SIG, "latest.json", &[test_key()]),
            Err(Refusal::SignedForSomethingElse(_))
        ));
        // the Linux feed, signed as the Windows one
        assert!(matches!(
            verify_manifest(LATEST, LATEST_SIG_WIN_COMMENT, "latest.json", &[test_key()]),
            Err(Refusal::SignedForSomethingElse(_))
        ));
        assert_eq!(trusted_comment(feed_role(false), "12.0.3"), "tinhat-desktop latest.json 12.0.3");
        assert_eq!(trusted_comment(feed_role(true), "12.0.3"), "tinhat-desktop latest-windows.json 12.0.3");
    }

    #[test]
    fn a_signed_feed_naming_a_foreign_host_or_another_release_is_refused() {
        // signed and well-formed, but its download lives elsewhere
        let v = verify_manifest(FOREIGN, FOREIGN_SIG, "latest.json", &[test_key()]).unwrap();
        let r = download_allowed(&s(&v, "url_bin"), FEED, &s(&v, "version"), false);
        assert!(matches!(r, Err(Refusal::Download(ref m)) if m.contains("outside the v12.0.3 release")), "{r:?}");
        // the binary of an older release under a newer feed
        let v = verify_manifest(OLDTAG, OLDTAG_SIG, "latest.json", &[test_key()]).unwrap();
        assert!(download_allowed(&s(&v, "url_bin"), FEED, "12.0.3", false).is_err());
        // the good ones pass
        let v = verify_manifest(LATEST, LATEST_SIG, "latest.json", &[test_key()]).unwrap();
        assert_eq!(download_allowed(&s(&v, "url_bin"), FEED, "12.0.3", false), Ok(()));
        let w = verify_manifest(WINDOWS, WINDOWS_SIG, "latest-windows.json", &[test_key()]).unwrap();
        assert_eq!(download_allowed(&s(&w, "url"), FEED_WIN, "12.0.3", false), Ok(()));
    }

    #[test]
    fn downloads_are_pinned_to_the_release_tag_or_the_feeds_own_folder() {
        let tag = "https://github.com/TinHatLabs/TinHat/releases/download/v12.0.3";
        let ok = |u: &str, f: &str, o: bool| download_allowed(u, f, "12.0.3", o).is_ok();
        assert!(ok(&format!("{tag}/tinhat-12.0.3-x86_64.bin"), FEED, false));
        // a feed read at the tag itself (not "latest") pins the same way
        assert!(ok(&format!("{tag}/x.bin"), &format!("{tag}/latest.json"), false));
        for bad in [
            "http://github.com/TinHatLabs/TinHat/releases/download/v12.0.3/x.bin".to_string(), // not https
            "https://github.com/TinHatLabs/Other/releases/download/v12.0.3/x.bin".to_string(), // another repository
            "https://github.com/evil/TinHat/releases/download/v12.0.3/x.bin".to_string(),      // another owner
            "https://github.com/TinHatLabs/TinHat/releases/download/v12.0.2/x.bin".to_string(), // another release
            "https://github.com:444/TinHatLabs/TinHat/releases/download/v12.0.3/x.bin".to_string(),
            "https://user@github.com/TinHatLabs/TinHat/releases/download/v12.0.3/x.bin".to_string(),
            format!("{tag}/x.bin?raw=1"),
            format!("{tag}/x.bin#frag"),
            format!("{tag}/.hidden"),
            format!("{tag}/sub/x.bin"),
            "https://release-assets.githubusercontent.com/x.bin".to_string(), // the CDN only by GitHub's own redirect
            "https://example.net/TinHatLabs/TinHat/releases/download/v12.0.3/x.bin".to_string(),
            "file:///tmp/x.bin".to_string(),
            "not a url".to_string(),
        ] {
            assert!(!ok(&bad, FEED, false), "{bad}");
        }
        // a weird version can't steer the path
        assert!(download_allowed(&format!("{tag}/x.bin"), FEED, "12.0.3/../x", false).is_err());
        // a mirror / test feed: its own folder on its own host
        let m = "https://updates.example.net/desktop/latest.json";
        assert!(ok("https://updates.example.net/desktop/tinhat-12.0.3-x86_64.bin", m, false));
        assert!(!ok("https://updates.example.net/other/tinhat-12.0.3-x86_64.bin", m, false));
        assert!(!ok("https://cdn.example.net/desktop/tinhat-12.0.3-x86_64.bin", m, false));
        assert!(!ok("http://updates.example.net/desktop/tinhat-12.0.3-x86_64.bin", m, false));
        // the local test override may be plain http, on its own host only
        let t = "http://127.0.0.1:8000/latest.json";
        assert!(ok("http://127.0.0.1:8000/tinhat-12.0.3-x86_64.bin", t, true));
        assert!(!ok("http://127.0.0.1:8000/tinhat-12.0.3-x86_64.bin", t, false));
        assert!(!ok("http://127.0.0.1:9000/tinhat-12.0.3-x86_64.bin", t, true));
        assert!(!ok("http://example.net/tinhat-12.0.3-x86_64.bin", t, true));
    }

    #[test]
    fn a_github_feed_follows_any_https_redirect_and_nothing_else() {
        let f = Url::parse(FEED).unwrap();
        let ok = |u: &str| redirect_allowed(&Url::parse(u).unwrap(), &f, false);
        assert!(ok("https://github.com/TinHatLabs/TinHat/releases/download/v12.0.3/latest.json"));
        assert!(ok("https://release-assets.githubusercontent.com/github-production-release-asset/1/2?sp=r&sig=x"));
        assert!(ok("https://objects.githubusercontent.com/github-production-release-asset-2e65be/1/2"));
        // launch-gate's review #2: a download host GitHub might move to next isn't refused (the signature and the
        // sha256 pin the content; the host never did)
        assert!(ok("https://github-releases.example-cdn.net/x"));
        assert!(ok("https://release-assets.githubusercontent.com:8443/x"));
        // never plain http, never a user name, never another scheme
        assert!(!ok("http://release-assets.githubusercontent.com/x"));
        assert!(!ok("https://user:pw@release-assets.githubusercontent.com/x"));
        assert!(!ok("https://user@example.net/x"));
        assert!(!ok("file:///etc/passwd"));
        assert!(!ok("ftp://example.net/x"));
        // a feed that isn't a GitHub release feed doesn't get the GitHub rule
        let not_release = Url::parse("https://github.com/TinHatLabs/TinHat/raw/main/latest.json").unwrap();
        assert!(!redirect_allowed(&Url::parse("https://example.net/x").unwrap(), &not_release, false));
        // a mirror feed: its own origin only
        let m = Url::parse("https://updates.example.net/desktop/latest.json").unwrap();
        assert!(redirect_allowed(&Url::parse("https://updates.example.net/desktop/v2/latest.json").unwrap(), &m, false));
        assert!(!redirect_allowed(&Url::parse("https://release-assets.githubusercontent.com/x").unwrap(), &m, false));
        // an http test feed may hop on its own origin only
        let t = Url::parse("http://127.0.0.1:8000/latest.json").unwrap();
        assert!(redirect_allowed(&Url::parse("http://127.0.0.1:8000/b.bin").unwrap(), &t, true));
        assert!(!redirect_allowed(&Url::parse("http://127.0.0.1:8000/b.bin").unwrap(), &t, false));
        assert!(!redirect_allowed(&Url::parse("http://example.net/b.bin").unwrap(), &t, true));
    }

    #[test]
    fn an_older_signed_feed_still_verifies_and_the_version_rule_refuses_it() {
        // a replayed older release is genuinely signed; update.rs' is_upgrade is what turns it away
        let v = verify_manifest(OLD, OLD_SIG, "latest.json", &[test_key()]).unwrap();
        assert_eq!(s(&v, "version"), "12.0.1");
        assert!(!crate::update::is_upgrade("12.0.1", "12.0.2"));
        assert!(!crate::update::is_upgrade("12.0.2", "12.0.2"));
        assert!(crate::update::is_upgrade("12.0.3", "12.0.2"));
    }

    #[test]
    fn checksums_must_be_sha256_hex() {
        assert!(plain_sha256(&"a".repeat(64)));
        assert!(plain_sha256(&"0123456789ABCDEFabcdef".repeat(3)[..64]));
        assert!(!plain_sha256(""));
        assert!(!plain_sha256(&"a".repeat(63)));
        assert!(!plain_sha256(&"g".repeat(64)));
    }
}
