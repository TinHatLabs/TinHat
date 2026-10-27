//! "Trust the phone's native device" — the desktop's half (docs: the iOS repo's BRIDGE.md "trust", Tin Hat Web's
//! docs/TRUST.md). The phone's Tin Hat app sets up its own device of the account (calls and notifications); on the
//! phone the page signs it through the app bridge. The desktop has no bridge: Settings → Devices & security → the
//! unsigned "Tin Hat · … (calls and notifications)" row offers "Trust…", shows that device's fingerprint, and runs the
//! same checks for that device only (steps 2–5 of the contract):
//!
//!   2 the server's copy      the device is listed and its ed25519 key is exactly the one shown; a miss waits up to
//!                            WAIT_MS for the device list to catch up, then "not-published"; different → "mismatch"
//!   3 already signed         is_verified_with_cross_signing() (= the web's crossSigningVerified) → "already"
//!   4 the self-signing key   no identity → "no-cross-signing"; not on this device → "keys-locked"
//!   5 sign                   Device::verify() (our own device: signed with our self-signing key) → "signed";
//!                            any error → "failed"
//!
//! The rule is the web's features/native/trust.ts trustDevice, case for case (the tests below are its CASES table,
//! same names, same answers), and the words are the web's TRUST_WORDS / trustResultText byte for byte. The web's
//! attempt bookkeeping (TrustAttempts) is bridge-only and has no desktop twin.
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustResult {
    Signed,
    Already,
    NotPublished,
    Mismatch,
    NoCrossSigning,
    KeysLocked,
    Failed,
}

impl TrustResult {
    /// The contract's word (the bridge's `result`; the desktop logs it).
    pub fn as_str(self) -> &'static str {
        match self {
            TrustResult::Signed => "signed",
            TrustResult::Already => "already",
            TrustResult::NotPublished => "not-published",
            TrustResult::Mismatch => "mismatch",
            TrustResult::NoCrossSigning => "no-cross-signing",
            TrustResult::KeysLocked => "keys-locked",
            TrustResult::Failed => "failed",
        }
    }
    pub const ALL: [TrustResult; 7] = [
        TrustResult::Signed,
        TrustResult::Already,
        TrustResult::NotPublished,
        TrustResult::Mismatch,
        TrustResult::NoCrossSigning,
        TrustResult::KeysLocked,
        TrustResult::Failed,
    ];
}

/// Step 2's wait for the device list after a miss.
pub const WAIT_MS: u64 = 5_000;

/// A device id: printable ASCII, no spaces, 1–255 characters (the web's deviceIdOk).
pub fn device_id_ok(v: &str) -> bool {
    !v.is_empty() && v.len() <= 255 && v.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// An ed25519 public key as unpadded base64: 43 characters, the last one a valid end (the web's ed25519Ok).
pub fn ed25519_ok(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 43
        && b[..42].iter().all(|c| c.is_ascii_alphanumeric() || *c == b'+' || *c == b'/')
        && b"AEIMQUYcgkosw048".contains(&b[42])
}

/// The crypto facts the rule needs (MatrixLookup = matrix-sdk; the tests stub them).
#[allow(async_fn_in_trait)]
pub trait TrustLookup {
    fn own_device_id(&self) -> String;
    /// the server's copy of my device: Some(fingerprint) ("" = no key), None = not listed
    async fn device_key(&self, device_id: &str) -> Result<Option<String>, String>;
    /// returns once `device_id` is listed (after asking the server for my keys again), at the latest after `ms`
    async fn next_device_update(&self, device_id: &str, ms: u64);
    async fn cross_signed(&self, device_id: &str) -> Result<bool, String>;
    /// (the account has a cross-signing identity, the private self-signing key is on this device)
    async fn signing(&self) -> Result<(bool, bool), String>;
    /// signs `device_id`; the adapter checks right at the signature that the device it signs has exactly `ed25519`
    /// and that the session is still the one that started (Err otherwise)
    async fn sign(&self, device_id: &str, ed25519: &str) -> Result<(), String>;
}

/// Steps 2–5 for one device and the key the person was shown. Never signs anything but `device_id`, and that only
/// when the server's key for it is exactly `ed25519`.
pub async fn trust_device<L: TrustLookup>(look: &L, device_id: &str, ed25519: &str, wait_ms: u64) -> TrustResult {
    match run(look, device_id, ed25519, wait_ms).await {
        Ok(r) => r,
        Err(_) => TrustResult::Failed,
    }
}

async fn run<L: TrustLookup>(look: &L, device_id: &str, ed25519: &str, wait_ms: u64) -> Result<TrustResult, String> {
    if !device_id_ok(device_id) || !ed25519_ok(ed25519) || device_id == look.own_device_id() {
        return Ok(TrustResult::Mismatch);
    }
    let mut key = look.device_key(device_id).await?;
    if key.is_none() {
        look.next_device_update(device_id, wait_ms).await;
        key = look.device_key(device_id).await?;
    }
    let Some(key) = key else { return Ok(TrustResult::NotPublished) };
    if key != ed25519 {
        return Ok(TrustResult::Mismatch);
    }
    if look.cross_signed(device_id).await? {
        return Ok(TrustResult::Already);
    }
    let (identity, ssk_here) = look.signing().await?;
    if !identity {
        return Ok(TrustResult::NoCrossSigning);
    }
    if !ssk_here {
        return Ok(TrustResult::KeysLocked);
    }
    // the key once more right before signing (the device list may have changed meanwhile)
    if look.device_key(device_id).await?.as_deref() != Some(ed25519) {
        return Ok(TrustResult::Mismatch);
    }
    look.sign(device_id, ed25519).await?;
    Ok(TrustResult::Signed)
}

// ---- the Devices fallback's rows, fingerprint and words (= the web's trust.ts) ----------------------------------------

/// "Tin Hat · <model> (calls and notifications)" — how the iOS / Android apps name their device. The name only picks
/// the row; the person checks the fingerprint and the rule checks the key.
pub fn is_native_phone_device(name: &str) -> bool {
    const HEAD: &str = "Tin Hat · ";
    const TAIL: &str = " (calls and notifications)";
    match name.strip_prefix(HEAD).and_then(|r| r.strip_suffix(TAIL)) {
        Some(model) => {
            let n = model.chars().count();
            (1..=80).contains(&n) && !model.contains('\n') && !model.contains('\r')
        }
        None => false,
    }
}

/// The Devices fallback is OFF in 12.0.4 (launch-gate, 2026-10-27, assembly-035 review): the row is picked by the
/// device's NAME (the server's text; any device of the account can set it) and the phone apps don't show their own
/// fingerprint, so someone with the password could name a device "Tin Hat · iPhone (calls and notifications)" and get it
/// cross-signed with one click, and with that the room keys and secrets a verified own device is given. Only the bridge
/// signs (the web page inside the phone app). Back on with (b) the phone showing its fingerprint + the sheet asking to
/// compare, or (c) SAS/QR against the native device. Moves together with Tin Hat Web's trust.ts DEVICES_FALLBACK.
pub const DEVICES_FALLBACK: bool = false;

/// Does a row of Settings → Devices get "Trust…"? Never while [`DEVICES_FALLBACK`] is off, whatever the name.
pub fn fallback_offers_trust(name: &str) -> bool {
    DEVICES_FALLBACK && is_native_phone_device(name)
}

/// Groups of four, like every Matrix client's "session key".
pub fn fingerprint_text(ed25519: &str) -> String {
    ed25519.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

pub const ACTION: &str = "Trust…";
pub const TITLE: &str = "Trust this phone?";
pub const BUTTON: &str = "Trust";
pub const CANCEL: &str = "Cancel";
pub const WORKING: &str = "Trusting…";
pub const VERIFY: &str = "Verify this device…";

pub fn body(name: &str) -> String {
    format!(
        "{name} is the device the Tin Hat app on your phone set up for calls and notifications. Trust it only if you set it up yourself. Its fingerprint:"
    )
}

/// The dialog's sentence for a result ("" = done: close it).
pub fn result_text(r: TrustResult, name: &str) -> String {
    match r {
        TrustResult::Signed | TrustResult::Already => String::new(),
        TrustResult::NotPublished => format!("{name} hasn't published its keys yet. Open Tin Hat on the phone, then try again."),
        TrustResult::Mismatch => format!("The key your server has for {name} doesn't match. Nothing was trusted. Set the phone up again in the Tin Hat app."),
        TrustResult::NoCrossSigning => "Your account has no encryption set up yet, so nothing can vouch for this phone. Verify this device first.".into(),
        TrustResult::KeysLocked => "Verify this device first: only a verified device can vouch for your phone.".into(),
        TrustResult::Failed => "Couldn't trust it. Try again in a moment.".into(),
    }
}

/// The toast after a done result.
pub fn done_text(r: TrustResult, name: &str) -> String {
    if r == TrustResult::Already {
        format!("{name} is already trusted")
    } else {
        format!("Trusted {name}")
    }
}

// ---- matrix-sdk ----------------------------------------------------------------------------------------------------

/// The real lookups: my own account's devices in the crypto store (kept current by sync's device-list updates; a miss
/// asks the server for my keys again). Bound to the session that made it: `sign` refuses once the app's signed-in
/// client is another account or device.
pub struct MatrixLookup {
    pub client: matrix_sdk::Client,
    session: (String, String),
}

impl MatrixLookup {
    pub fn new(client: matrix_sdk::Client) -> Self {
        let session = Self::ids(&client);
        MatrixLookup { client, session }
    }
    fn ids(c: &matrix_sdk::Client) -> (String, String) {
        (c.user_id().map(|u| u.to_string()).unwrap_or_default(), c.device_id().map(|d| d.to_string()).unwrap_or_default())
    }
    fn still_ours(&self) -> bool {
        !self.session.0.is_empty() && crate::session::client().is_some_and(|c| Self::ids(&c) == self.session)
    }
    fn me(&self) -> Result<matrix_sdk::ruma::OwnedUserId, String> {
        self.client.user_id().map(|u| u.to_owned()).ok_or_else(|| "signed out".to_string())
    }
    async fn device(&self, device_id: &str) -> Result<Option<matrix_sdk::encryption::identities::Device>, String> {
        let me = self.me()?;
        let id: &matrix_sdk::ruma::DeviceId = device_id.into();
        self.client.encryption().get_device(&me, id).await.map_err(|e| e.to_string())
    }
}

impl TrustLookup for MatrixLookup {
    fn own_device_id(&self) -> String {
        self.client.device_id().map(|d| d.to_string()).unwrap_or_default()
    }
    async fn device_key(&self, device_id: &str) -> Result<Option<String>, String> {
        Ok(self.device(device_id).await?.map(|d| d.ed25519_key().map(|k| k.to_base64()).unwrap_or_default()))
    }
    async fn next_device_update(&self, device_id: &str, ms: u64) {
        // a key query for my own user now (sync's device-list change would do it too), then watch the store
        if let Ok(me) = self.me() {
            let _ = tokio::time::timeout(Duration::from_millis(ms), self.client.encryption().request_user_identity(&me)).await;
        }
        let end = tokio::time::Instant::now() + Duration::from_millis(ms);
        while tokio::time::Instant::now() < end {
            if matches!(self.device(device_id).await, Ok(Some(_))) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    async fn cross_signed(&self, device_id: &str) -> Result<bool, String> {
        Ok(self.device(device_id).await?.is_some_and(|d| d.is_verified_with_cross_signing()))
    }
    async fn signing(&self) -> Result<(bool, bool), String> {
        let me = self.me()?;
        let enc = self.client.encryption();
        let identity = enc.get_user_identity(&me).await.map_err(|e| e.to_string())?.is_some();
        let ssk = enc.cross_signing_status().await.is_some_and(|s| s.has_self_signing);
        Ok((identity, ssk))
    }
    async fn sign(&self, device_id: &str, ed25519: &str) -> Result<(), String> {
        if !self.still_ours() {
            return Err("the session changed".into());
        }
        // the very Device object that gets signed carries exactly the key that was checked
        let d = self.device(device_id).await?.ok_or_else(|| "device gone".to_string())?;
        if d.ed25519_key().map(|k| k.to_base64()).as_deref() != Some(ed25519) {
            return Err("the device's key changed".into());
        }
        d.verify().await.map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    const KEY: &str = "l0Fz1bVnXJ8wSg1E4Qm2b9P6v3cN5yA7kR0tD2uHqWE";
    const OTHER: &str = "AAAAbVnXJ8wSg1E4Qm2b9P6v3cN5yA7kR0tD2uHqWE"; // 42 chars: not a key
    const KEY2: &str = "Zm9vYmFyYmF6cXV4cXV1eGNvcmdlZ3JhdWx0Z2FycGs";
    const PHONE: &str = "PHONEDEV1";
    const ME_DEV: &str = "WEBDEV";

    #[derive(Default)]
    struct Facts {
        keys: Vec<Option<&'static str>>,
        signed: bool,
        no_identity: bool,
        no_ssk: bool,
        sign_throws: bool,
        look_throws: bool,
    }
    struct Rig {
        f: Facts,
        looks: Cell<usize>,
        waited: Cell<u64>,
        signed_ids: RefCell<Vec<String>>,
    }
    impl Rig {
        fn new(mut f: Facts) -> Self {
            if f.keys.is_empty() {
                f.keys = vec![Some(KEY)];
            }
            Rig { f, looks: Cell::new(0), waited: Cell::new(0), signed_ids: RefCell::new(vec![]) }
        }
    }
    impl TrustLookup for Rig {
        fn own_device_id(&self) -> String {
            ME_DEV.into()
        }
        async fn device_key(&self, id: &str) -> Result<Option<String>, String> {
            if self.f.look_throws {
                return Err("offline".into());
            }
            if id == ME_DEV {
                return Ok(Some(KEY.into()));
            }
            if id != PHONE {
                return Ok(None);
            }
            let i = self.looks.get().min(self.f.keys.len() - 1);
            self.looks.set(self.looks.get() + 1);
            Ok(self.f.keys[i].map(str::to_string))
        }
        async fn next_device_update(&self, id: &str, ms: u64) {
            assert_eq!(id, PHONE);
            self.waited.set(ms);
        }
        async fn cross_signed(&self, _: &str) -> Result<bool, String> {
            Ok(self.f.signed)
        }
        async fn signing(&self) -> Result<(bool, bool), String> {
            Ok((!self.f.no_identity, !self.f.no_ssk))
        }
        async fn sign(&self, id: &str, key: &str) -> Result<(), String> {
            if self.f.sign_throws {
                return Err("upload refused".into());
            }
            assert_eq!(key, KEY, "the adapter gets the checked key");
            self.signed_ids.borrow_mut().push(id.into());
            Ok(())
        }
    }
    fn block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }
    use TrustResult::*;

    /// the web's trust.test.ts CASES, same order, same names
    #[test]
    fn the_rule_answers_every_case_like_the_web() {
        let cases: Vec<(&str, Facts, &str, &str, TrustResult, bool)> = vec![
            ("signs the named device when the server's key matches", Facts::default(), PHONE, KEY, Signed, true),
            ("already signed: says so, signs nothing", Facts { signed: true, ..Default::default() }, PHONE, KEY, Already, false),
            ("not listed, then listed after the next update: signs", Facts { keys: vec![None, Some(KEY)], ..Default::default() }, PHONE, KEY, Signed, true),
            ("not listed after the wait: not-published", Facts { keys: vec![None], ..Default::default() }, PHONE, KEY, NotPublished, false),
            ("listed without a key: mismatch", Facts { keys: vec![Some("")], ..Default::default() }, PHONE, KEY, Mismatch, false),
            ("a different key on the server: mismatch, never signed", Facts { keys: vec![Some(KEY2)], ..Default::default() }, PHONE, KEY, Mismatch, false),
            ("the key changes right before signing: mismatch, never signed", Facts { keys: vec![Some(KEY), Some(KEY2)], ..Default::default() }, PHONE, KEY, Mismatch, false),
            ("no cross-signing on the account", Facts { no_identity: true, ..Default::default() }, PHONE, KEY, NoCrossSigning, false),
            ("the self-signing key isn't on this device: keys-locked", Facts { no_ssk: true, ..Default::default() }, PHONE, KEY, KeysLocked, false),
            ("signing fails: failed", Facts { sign_throws: true, ..Default::default() }, PHONE, KEY, Failed, false),
            ("a lookup throws: failed", Facts { look_throws: true, ..Default::default() }, PHONE, KEY, Failed, false),
            ("the page's own device is never signed this way", Facts::default(), ME_DEV, KEY, Mismatch, false),
            ("a malformed key is refused before any lookup", Facts::default(), PHONE, OTHER, Mismatch, false),
            ("a malformed device id is refused", Facts::default(), "has space", KEY, Mismatch, false),
        ];
        assert_eq!(cases.len(), 14);
        for (name, facts, id, key, want, signs) in cases {
            let rig = Rig::new(facts);
            let r = block(trust_device(&rig, id, key, 5000));
            assert_eq!(r, want, "{name}");
            let expect: Vec<String> = if signs { vec![id.to_string()] } else { vec![] };
            assert_eq!(*rig.signed_ids.borrow(), expect, "{name}");
        }
    }

    #[test]
    fn a_miss_waits_for_the_device_list_at_most_five_seconds() {
        let rig = Rig::new(Facts { keys: vec![None], ..Default::default() });
        block(trust_device(&rig, PHONE, KEY, WAIT_MS));
        assert_eq!(rig.waited.get(), 5000);
        let hit = Rig::new(Facts::default());
        block(trust_device(&hit, PHONE, KEY, WAIT_MS));
        assert_eq!(hit.waited.get(), 0, "no wait when the device is listed");
    }

    #[test]
    fn keys_and_ids_have_the_webs_shapes() {
        assert!(ed25519_ok(KEY) && ed25519_ok(KEY2));
        assert!(!ed25519_ok(OTHER));
        assert!(!ed25519_ok(&format!("{KEY}=")));
        assert!(!ed25519_ok(&format!("{}B", &KEY[..42])));
        assert!(device_id_ok(PHONE));
        assert!(!device_id_ok(""));
        assert!(!device_id_ok("a b"));
        assert!(!device_id_ok(&"x".repeat(256)));
    }

    #[test]
    fn the_devices_fallback_keeps_the_webs_rows_fingerprint_and_words() {
        assert!(is_native_phone_device("Tin Hat · iPhone (calls and notifications)"));
        assert!(is_native_phone_device("Tin Hat · Pixel 9 Pro (calls and notifications)"));
        assert!(!is_native_phone_device("Tin Hat Web (Firefox)"));
        assert!(!is_native_phone_device("Tin Hat · iPhone"));
        assert!(!is_native_phone_device("xTin Hat · iPhone (calls and notifications)"));
        assert_eq!(fingerprint_text(KEY), "l0Fz 1bVn XJ8w Sg1E 4Qm2 b9P6 v3cN 5yA7 kR0t D2uH qWE");
        assert_eq!(TITLE, "Trust this phone?");
        assert_eq!(ACTION, "Trust…");
        assert_eq!(result_text(Signed, "P"), "");
        assert_eq!(result_text(Already, "P"), "");
        assert_eq!(result_text(NotPublished, "P"), "P hasn't published its keys yet. Open Tin Hat on the phone, then try again.");
        assert_eq!(result_text(KeysLocked, "P"), "Verify this device first: only a verified device can vouch for your phone.");
        assert_eq!(done_text(Signed, "P"), "Trusted P");
        assert_eq!(done_text(Already, "P"), "P is already trusted");
        let words: Vec<&str> = TrustResult::ALL.iter().map(|r| r.as_str()).collect();
        assert_eq!(words, ["signed", "already", "not-published", "mismatch", "no-cross-signing", "keys-locked", "failed"]);
    }

    /// 12.0.4: a phone-named device gets no "Trust…" in Settings → Devices (web twin: trust.test.ts "the Devices
    /// fallback is off"); the rule itself (the bridge's path, the CASES above) still signs.
    #[test]
    fn the_devices_fallback_is_off() {
        assert!(!DEVICES_FALLBACK);
        for name in ["Tin Hat · iPhone (calls and notifications)", "Tin Hat · Pixel 9 Pro (calls and notifications)", "Tin Hat Web (Firefox)", ""] {
            assert!(!fallback_offers_trust(name), "{name}");
        }
        let settings = include_str!("settings.rs");
        assert!(settings.contains("fallback_offers_trust(&name)"), "settings.rs picks the Trust rows through fallback_offers_trust");
        assert!(!settings.contains("is_native_phone_device(&name)"), "settings.rs must not pick Trust rows by the name alone");
    }

    /// The QML dialog shows these strings through the controller; the window file must not carry its own copies.
    #[test]
    fn the_dialog_reads_its_words_from_rust() {
        let qml = include_str!("../qml/TrustDeviceDialog.qml");
        assert!(qml.contains("trustWords"), "TrustDeviceDialog.qml takes its words from settings.trustWords()");
        assert!(!qml.contains("Trust this phone?"));
    }
}
