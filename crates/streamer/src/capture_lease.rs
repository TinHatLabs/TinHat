//! Pad Watch Layer 2 (share claude/tinhat-pad/WATCH-LEASE/DESIGN.md §2): the capture lease. Every capture / publish
//! operation takes a lease before it starts, hands each resource it builds to the lease the moment it exists
//! ([`Lease::hold`]), ACTIVATES it (PLAYING, store, republish) only through [`Lease::activate`] — under the same lock a
//! revoke takes, so a revoke is either before (nothing starts) or after (the resource's stop runs) — and commits right
//! before it stores the result. A Watch press revokes every lease synchronously (the generation moves under the lock;
//! the stops run on a blocking thread, counted by [`pending`] until done: a watch join starts only at 0). The pure core
//! is pinned by the case table captureleases.cases.json, byte-identical with the web's capturelease.ts.
use std::sync::Mutex;
#[cfg(not(test))]
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mic,
    Meter,
    Camera,
    Screen,
    ShareAudio,
    Music,
    SameRoom,
    ConnTest,
    SettingsMeter,
    Aec,
    /// round 10: a speaker-OUTPUT tap (the speaker meter, the hear sniffer, the echo-canceller reference restart): it
    /// hears what the computer plays, other apps included — leased and gated like every capture. Follows the capture
    /// bar only (not the meter bar: the call's own echo reference must run in a normal call).
    OutputTap,
    /// voice notes (docs/VOICE-NOTES.md §4): the voice-message recorder — outside calls only; refused while watching
    /// (the capture bar) like the meters, and by the meters' own bar.
    VoiceNote,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "mic" => Kind::Mic,
            "meter" => Kind::Meter,
            "camera" => Kind::Camera,
            "screen" => Kind::Screen,
            "share-audio" => Kind::ShareAudio,
            "music" => Kind::Music,
            "sameroom" => Kind::SameRoom,
            "conntest" => Kind::ConnTest,
            "settings-meter" => Kind::SettingsMeter,
            "aec" => Kind::Aec,
            "output-tap" => Kind::OutputTap,
            "voice-note" => Kind::VoiceNote,
            _ => return None,
        })
    }
    fn is_meter(self) -> bool {
        matches!(self, Kind::Meter | Kind::SettingsMeter | Kind::VoiceNote)
    }
}

type Stop = Box<dyn FnOnce() + Send>;

struct Entry {
    gen: u64,
    kind: Kind,
    session: Option<u64>,
    stops: Vec<Stop>,
    committed: bool,
    /// round 6: a platform start of this lease is running (begun under the lock); a revoke meanwhile leaves the stop of
    /// what it starts to the worker (it learns at `end_start_hold`)
    starting: bool,
    /// round 7: the source gates of what this lease started (open = its samples may flow downstream). A revoke closes
    /// them synchronously, under the lock, before anything else runs.
    gates: Vec<Gate>,
}

/// Round 7: a capture's source gate. While closed, every buffer leaving the capture's source element is dropped — no
/// sample reaches an encoder, an appsink feeding a LiveKit source, or the network. Opened only when its start is
/// accepted under the lease lock; closed by any revoke of the lease (and by the capture's own stop).
pub type Gate = std::sync::Arc<std::sync::atomic::AtomicBool>;
pub fn new_gate() -> Gate {
    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
}

/// The pure state (one per process in the app: [`leases`]; the tests make their own).
pub struct Leases {
    gen: u64,
    barred: bool,
    meter_barred: bool,
    session: Option<u64>,
    next: u64,
    live: std::collections::BTreeMap<u64, Entry>,
    /// stops of revoked leases still running (the blocking thread); `pending` counts them
    stopping: usize,
    /// round 8: every source gate that is open now, committed captures included (the meter, a call's capture handed to
    /// its owner): a revoke closes them all at once (the invariant's "gated at its source at once")
    open_gates: Vec<std::sync::Weak<std::sync::atomic::AtomicBool>>,
    /// test seam: false = the pre-lease behaviour (only the start is checked), for the escape scenarios' red runs
    pub enforce: bool,
}

impl Default for Leases {
    fn default() -> Self {
        Leases { gen: 0, barred: false, meter_barred: false, session: None, next: 1, live: Default::default(), stopping: 0, open_gates: vec![], enforce: true }
    }
}

impl Leases {
    pub fn set_barred(&mut self, on: bool) {
        self.barred = on;
    }
    /// the meter's own bar (the app: watching, or the call bar)
    pub fn set_meter_barred(&mut self, on: bool) {
        self.meter_barred = on;
    }
    pub fn set_session(&mut self, s: Option<u64>) {
        self.session = s;
    }
    pub fn session(&self) -> Option<u64> {
        self.session
    }
    fn kind_barred(&self, k: Kind) -> bool {
        self.barred || (k.is_meter() && self.meter_barred)
    }
    fn admits(&self, e: &Entry) -> bool {
        if !self.enforce {
            return true;
        }
        !e.committed && e.gen == self.gen && !self.kind_barred(e.kind) && (e.session.is_none() || e.session == self.session)
    }
    /// A lease id, or None when refused (barred, or a session that isn't the current one).
    pub fn take(&mut self, kind: Kind, session: Option<u64>) -> Option<u64> {
        if self.kind_barred(kind) || (session.is_some() && session != self.session) {
            return None;
        }
        let id = self.next;
        self.next += 1;
        self.live.insert(id, Entry { gen: self.gen, kind, session, stops: vec![], committed: false, starting: false, gates: vec![] });
        Some(id)
    }
    pub fn valid(&self, id: u64) -> bool {
        self.live.get(&id).is_some_and(|e| self.admits(e))
    }
    /// Hold a resource: Ok(()) = it is the lease's; Err(stop) = invalid, the caller runs the stop NOW (outside the lock).
    pub fn hold(&mut self, id: u64, stop: Stop) -> Result<(), Stop> {
        let ok = self.valid(id);
        match self.live.get_mut(&id) {
            Some(e) if ok => {
                e.stops.push(stop);
                Ok(())
            }
            _ => Err(stop),
        }
    }
    /// round 6: admission of a platform start — checked AND marked starting in one step (under the caller's lock). false
    /// = revoked / barred / another session: the start must not begin.
    pub fn begin_start(&mut self, id: u64) -> bool {
        let ok = self.valid(id);
        if let Some(e) = self.live.get_mut(&id).filter(|_| ok) {
            e.starting = true;
        }
        ok
    }
    /// round 6: the platform start returned — still valid? (it is no longer starting either way)
    pub fn end_start(&mut self, id: u64) -> bool {
        if let Some(e) = self.live.get_mut(&id) {
            e.starting = false;
        }
        self.valid(id)
    }
    /// round 6: end the start and hold what it started, in one step: Err(stop) = revoked while starting — the WORKER runs
    /// the stop (nothing was stopped by the revoking thread).
    pub fn end_start_hold(&mut self, id: u64, stop: Stop) -> Result<(), Stop> {
        self.end_start_hold_gated(id, stop, None)
    }
    /// round 7: as `end_start_hold`, and on success OPEN `gate` (in the same step) and keep it with the lease so a
    /// later revoke closes it at once.
    pub fn end_start_hold_gated(&mut self, id: u64, stop: Stop, gate: Option<Gate>) -> Result<(), Stop> {
        if !self.end_start(id) {
            return Err(stop);
        }
        self.hold(id, stop)?;
        if let (Some(g), Some(e)) = (gate, self.live.get_mut(&id)) {
            g.store(true, std::sync::atomic::Ordering::SeqCst);
            self.open_gates.retain(|w| w.strong_count() > 0);
            self.open_gates.push(std::sync::Arc::downgrade(&g));
            e.gates.push(g);
        }
        Ok(())
    }
    /// true = committed (the resources are the caller's); false = the stops to run now.
    pub fn commit(&mut self, id: u64) -> Result<(), Vec<Stop>> {
        let ok = self.valid(id);
        match self.live.get_mut(&id) {
            Some(e) if ok => {
                e.committed = true;
                e.stops.clear();
                Ok(())
            }
            Some(e) => Err(std::mem::take(&mut e.stops)),
            None => Err(vec![]),
        }
    }
    /// The operation is over: an uncommitted lease gives back what it still holds (to stop).
    pub fn done(&mut self, id: u64) -> Vec<Stop> {
        match self.live.remove(&id) {
            Some(e) if !e.committed => e.stops,
            _ => vec![],
        }
    }
    /// Revoke every lease: (how many were live, the stops to run). The generation moves first.
    pub fn revoke(&mut self) -> (usize, Vec<Stop>) {
        if !self.enforce {
            return (0, vec![]);
        }
        self.gen += 1;
        let mut n = 0;
        let mut stops = vec![];
        for e in self.live.values_mut() {
            if e.committed {
                continue;
            }
            n += 1;
            // round 7: the source gates close NOW (under the lock): from here no sample of these captures flows on, even
            // before their NULLing (on its own thread) has run
            for g in e.gates.drain(..) {
                g.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            stops.append(&mut e.stops);
        }
        // round 8: and every other open gate — a capture already handed to its owner keeps its device until the owner
        // stops it, but from this instant none of its samples leaves its source
        for w in self.open_gates.drain(..) {
            if let Some(g) = w.upgrade() {
                g.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        (n, stops)
    }
    pub fn pending(&self) -> usize {
        self.live.len() + self.stopping
    }
}

fn run_stops(stops: Vec<Stop>) {
    for s in stops {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(s));
    }
}

// ── the process-wide instance ────────────────────────────────────────────────────────────────────────────────────

#[cfg(not(test))]
fn cell() -> &'static Mutex<Leases> {
    static L: OnceLock<Mutex<Leases>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Leases::default()))
}
/// (tests run in parallel threads: each gets its own instance)
#[cfg(test)]
fn cell() -> &'static Mutex<Leases> {
    thread_local! {
        static L: &'static Mutex<Leases> = Box::leak(Box::new(Mutex::new(Leases::default())));
    }
    L.with(|l| *l)
}
fn lock() -> std::sync::MutexGuard<'static, Leases> {
    cell().lock().unwrap_or_else(|p| p.into_inner())
}

/// The app's lease handle: ends itself (done) when dropped.
pub struct Lease {
    id: u64,
    pub kind: Kind,
}

/// A lease for `kind` (`session` = the call session's id, None = no call: a meter, the connection test).
pub fn take(kind: Kind, session: Option<u64>) -> Option<Lease> {
    lock().take(kind, session).map(|id| Lease { id, kind })
}

impl Lease {
    pub fn valid(&self) -> bool {
        lock().valid(self.id)
    }
    /// Hold a resource (its stop). false = revoked already: the stop has run, stand down.
    pub fn hold(&self, stop: impl FnOnce() + Send + 'static) -> bool {
        let r = lock().hold(self.id, Box::new(stop));
        match r {
            Ok(()) => true,
            Err(s) => {
                run_stops(vec![s]);
                false
            }
        }
    }
    /// Activate held resources (set PLAYING, store a handle, republish) atomically with respect to a revoke: `f` runs
    /// under the lease lock only while the lease is valid. Returns false (and runs nothing) when revoked. Keep `f` short:
    /// start a state change, insert into a map — never wait on the network here.
    pub fn activate<R>(&self, f: impl FnOnce() -> R) -> Option<R> {
        let l = lock();
        if !l.valid(self.id) {
            return None;
        }
        let r = f();
        drop(l);
        Some(r)
    }
    /// Commit atomically with a revoke: `f` (store the handle) runs under the lease lock only while the lease is valid,
    /// and the lease is committed in the same breath. None = revoked: what it held is stopped, `f` never ran.
    pub fn commit_with<R>(&self, f: impl FnOnce() -> R) -> Option<R> {
        let mut l = lock();
        if l.valid(self.id) {
            let r = f();
            let _ = l.commit(self.id);
            return Some(r);
        }
        let stops = l.commit(self.id).err().unwrap_or_default();
        drop(l);
        run_stops(stops);
        None
    }
    /// The commit point: true = the resources are the caller's from now on; false = revoked, what it held is stopped.
    /// Round 9: commit AND open `gate` in one step under the lease lock (a capture whose start happens outside
    /// `start_under`, e.g. the camera self-view started on the GUI thread): its frames pass only from here on, and any
    /// later revoke closes the gate like every other (it joins `open_gates`). false = revoked: the gate stays closed and
    /// what the lease held is stopped.
    pub fn commit_opening(&self, gate: &Gate) -> bool {
        let mut l = lock();
        if l.valid(self.id) {
            let _ = l.commit(self.id);
            gate.store(true, std::sync::atomic::Ordering::SeqCst);
            l.open_gates.retain(|w| w.strong_count() > 0);
            l.open_gates.push(std::sync::Arc::downgrade(gate));
            return true;
        }
        gate.store(false, std::sync::atomic::Ordering::SeqCst);
        let stops = l.commit(self.id).err().unwrap_or_default();
        drop(l);
        run_stops(stops);
        false
    }
    pub fn commit(&self) -> bool {
        let r = lock().commit(self.id);
        match r {
            Ok(()) => true,
            Err(stops) => {
                run_stops(stops);
                false
            }
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let stops = lock().done(self.id);
        run_stops(stops);
    }
}

/// Revoke every lease (a Watch press, a queued one too; sign-out). The generation moves NOW; the stops (NULLing a
/// pipeline can block) run on their own thread and count as pending until they finish. Returns how many were live.
pub fn revoke(why: &str) -> usize {
    let (n, stops) = {
        let mut l = lock();
        let (n, stops) = l.revoke();
        l.stopping += usize::from(!stops.is_empty());
        (n, stops)
    };
    if n > 0 {
        eprintln!("[lease] {why}: {n} capture operation(s) revoked");
    }
    if !stops.is_empty() {
        #[cfg(not(test))]
        std::thread::spawn(move || {
            run_stops(stops);
            lock().stopping -= 1;
        });
        #[cfg(test)]
        {
            run_stops(stops);
            lock().stopping -= 1;
        }
    }
    n
}

/// Start a platform capture under `lease` (the shape every capture start uses, call.rs `leased_play`). Round 6 (Astra r5
/// P1): admission and the "starting" mark happen together UNDER the lease lock, immediately before `start` — nothing
/// runs between them — so a Watch press any time before that point means the start never begins. A press while the
/// platform call runs doesn't stop anything itself: when `start` returns, this worker sees the revoke (under the lock,
/// together with holding the result) and runs `stop` itself. false = not running.
pub fn start_under(lease: &Lease, stop: impl Fn() + Send + Sync + 'static, start: impl FnOnce() -> bool) -> bool {
    start_under_gated(lease, None, stop, start)
}

/// Round 7 (shape 2, accepted by launch-gate): as `start_under`, with the capture's source `gate`: closed while the
/// platform start runs and opened only when the start is accepted (under the lease lock, with holding its stop). So a
/// capture that starts late — the worker admitted, then a Watch press before it calls `start` — is physically open for
/// at most its bounded start + stop, and not one of its samples leaves its source element meanwhile.
pub fn start_under_gated(lease: &Lease, gate: Option<Gate>, stop: impl Fn() + Send + Sync + 'static, start: impl FnOnce() -> bool) -> bool {
    #[cfg(test)]
    test_pause();
    if let Some(g) = &gate {
        g.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    if !lock().begin_start(lease.id) {
        return false;
    }
    #[cfg(test)]
    test_pause_in_gap();
    let ok = start();
    let stop = std::sync::Arc::new(stop);
    let held = stop.clone();
    let g2 = gate.clone();
    let r = lock().end_start_hold_gated(
        lease.id,
        Box::new(move || {
            if let Some(g) = &g2 {
                g.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            held()
        }),
        if ok { gate.clone() } else { None },
    );
    match r {
        Ok(()) if ok => true,
        Ok(()) => false, // the start failed: the caller NULLs it (the held stop runs again at done — harmless)
        Err(_) => {
            if let Some(g) = &gate {
                g.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            stop(); // revoked while starting: the worker that owns the capture stops it
            false
        }
    }
}

/// tests: a pause IN the admission → platform-start gap (Astra round 6), running a press-like action in it
#[cfg(test)]
thread_local! {
    pub(crate) static GAP: std::cell::RefCell<Option<Box<dyn Fn()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
fn test_pause_in_gap() {
    GAP.with(|p| {
        if let Some(f) = p.borrow().as_ref() {
            f()
        }
    });
}

/// tests: a pause injected at the point between "everything before the start is done" and the start (where round 5 held
/// the stop and then started), running a press-like action in it
#[cfg(test)]
thread_local! {
    static PAUSE: std::cell::RefCell<Option<Box<dyn Fn()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
fn test_pause() {
    PAUSE.with(|p| {
        if let Some(f) = p.borrow().as_ref() {
            f()
        }
    });
}

/// tests: round 5's start_under, kept for the red run (hold, then the start — the pause lands between them)
#[cfg(test)]
fn start_under_r5(lease: &Lease, stop: impl Fn() + Send + Sync + 'static, start: impl FnOnce() -> bool) -> bool {
    let stop = std::sync::Arc::new(stop);
    let held = stop.clone();
    if !lease.hold(move || held()) {
        return false;
    }
    test_pause();
    if !start() {
        return false;
    }
    if !lease.valid() {
        stop();
        return false;
    }
    true
}

#[cfg(test)]
fn set_enforce(on: bool) {
    lock().enforce = on;
}
pub fn pending() -> usize {
    lock().pending()
}
pub fn set_barred(on: bool) {
    lock().set_barred(on);
}
pub fn set_meter_barred(on: bool) {
    lock().set_meter_barred(on);
}
/// A call session became the current one (CallSession::connect) / ended (close, phase 1).
pub fn set_session(s: u64) {
    lock().set_session(Some(s));
}
pub fn end_session(s: u64) {
    let mut l = lock();
    if l.session() == Some(s) {
        l.set_session(None);
    }
}
pub fn session() -> Option<u64> {
    lock().session()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    const CASES: &str = include_str!("captureleases.cases.json");
    /// the web's capturelease.test.ts LEASE_CASES_SHA256 (same bytes on both sides)
    const CASES_SHA256: &str = "79b13cb096f57bffc92275b126c1434f736cc922b3679d13eba47ca74baad058";

    /// SHA-256 (FIPS 180-4), test-only: the streamer crate has no hash dependency and needs none for this pin.
    fn sha256_hex(data: &[u8]) -> String {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01,
            0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
            0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
            0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
            0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08,
            0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
            0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
        ];
        let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
        let mut m = data.to_vec();
        let bits = (data.len() as u64).wrapping_mul(8);
        m.push(0x80);
        while m.len() % 64 != 56 {
            m.push(0);
        }
        m.extend_from_slice(&bits.to_be_bytes());
        for chunk in m.chunks(64) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
            }
            let mut v = h;
            for i in 0..64 {
                let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
                let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
                let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
                let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
                let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
                let t2 = s0.wrapping_add(maj);
                v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
            }
            for i in 0..8 {
                h[i] = h[i].wrapping_add(v[i]);
            }
        }
        h.iter().map(|x| format!("{x:08x}")).collect()
    }

    #[test]
    fn the_case_table_is_the_shared_one() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "the hash itself");
        assert_eq!(sha256_hex(CASES.as_bytes()), CASES_SHA256);
    }

    #[test]
    fn every_shared_case() {
        let v: serde_json::Value = serde_json::from_str(CASES).unwrap();
        for c in v["cases"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let mut l = Leases::default();
            let mut ids: std::collections::HashMap<String, u64> = Default::default();
            let mut sess: std::collections::HashMap<String, u64> = Default::default();
            let sid = |k: &serde_json::Value, sess: &mut std::collections::HashMap<String, u64>| -> Option<u64> {
                k.as_str().map(|k| {
                    let n = sess.len() as u64 + 1;
                    *sess.entry(k.to_string()).or_insert(n)
                })
            };
            let stopped = Arc::new(Mutex::new(Vec::<String>::new()));
            for (i, s) in c["steps"].as_array().unwrap().iter().enumerate() {
                let at = format!("{name}: step {} {}", i + 1, s["do"]);
                stopped.lock().unwrap().clear();
                let run = run_stops;
                match s["do"].as_str().unwrap() {
                    "owner" => {
                        let v = sid(&s["cur"], &mut sess);
                        l.set_session(v)
                    }
                    "bar" => l.set_barred(s["on"].as_bool().unwrap()),
                    "take" => {
                        let o = sid(&s["owner"], &mut sess);
                        let r = l.take(Kind::parse(s["kind"].as_str().unwrap()).unwrap(), o);
                        assert_eq!(r.is_some(), s["ok"].as_bool().unwrap(), "{at}");
                        if let Some(id) = r {
                            ids.insert(s["as"].as_str().unwrap().to_string(), id);
                        }
                    }
                    "hold" => {
                        let res = s["res"].as_str().unwrap().to_string();
                        let st = stopped.clone();
                        let stop: Stop = Box::new(move || {
                            st.lock().unwrap().push(res.clone());
                            if res.starts_with('!') {
                                panic!("boom");
                            }
                        });
                        let r = l.hold(ids[s["lease"].as_str().unwrap()], stop);
                        assert_eq!(r.is_ok(), s["ok"].as_bool().unwrap(), "{at}");
                        if let Err(stop) = r {
                            run(vec![stop]);
                        }
                    }
                    "valid" => assert_eq!(l.valid(ids[s["lease"].as_str().unwrap()]), s["ok"].as_bool().unwrap(), "{at}"),
                    "commit" => {
                        let r = l.commit(ids[s["lease"].as_str().unwrap()]);
                        assert_eq!(r.is_ok(), s["ok"].as_bool().unwrap(), "{at}");
                        if let Err(stops) = r {
                            run(stops);
                        }
                    }
                    "revoke" => {
                        let (n, stops) = l.revoke();
                        run(stops);
                        if let Some(want) = s.get("n") {
                            assert_eq!(n as u64, want.as_u64().unwrap(), "{at}");
                        }
                    }
                    "done" => {
                        let stops = l.done(ids[s["lease"].as_str().unwrap()]);
                        run(stops);
                    }
                    "pending" => assert_eq!(l.pending() as u64, s["n"].as_u64().unwrap(), "{at}"),
                    "begin_start" => assert_eq!(l.begin_start(ids[s["lease"].as_str().unwrap()]), s["ok"].as_bool().unwrap(), "{at}"),
                    "end_start" => assert_eq!(l.end_start(ids[s["lease"].as_str().unwrap()]), s["ok"].as_bool().unwrap(), "{at}"),
                    other => panic!("unknown op {other}"),
                }
                if let Some(want) = s.get("stopped") {
                    let want: Vec<String> = want.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
                    assert_eq!(*stopped.lock().unwrap(), want, "{at}: stopped");
                }
            }
        }
    }

    // ── the desktop's Astra round-3 escape classes, through the lease API in the order call.rs uses it ────────────
    // Each runs with leases off (RED: the pre-lease behaviour, only the start was checked — the escape reproduces) and
    // on (GREEN). Mirrored on the web where the class is shared (capturelease-escapes.test.ts).

    /// class 1: the meter's worker passed its first check, a Watch press lands while the pipeline is being set PLAYING
    fn meter_start(enforce: bool) -> (bool, bool) {
        set_enforce(enforce);
        set_barred(false);
        let lease = take(Kind::Meter, None).expect("admitted");
        let playing = Arc::new(Mutex::new(false));
        let (p1, p2) = (playing.clone(), playing.clone());
        let started = start_under(&lease, move || *p1.lock().unwrap() = false, move || {
            *p2.lock().unwrap() = true; // PLAYING ...
            set_barred(true); // ... and the Watch press in the middle of it
            revoke("watch");
            true
        });
        let committed = started && lease.commit();
        let live = *playing.lock().unwrap();
        set_enforce(true);
        (committed, live)
    }
    #[test]
    fn class1_red_without_leases_the_meter_keeps_capturing() {
        assert_eq!(meter_start(false), (true, true));
    }
    #[test]
    fn class1_green_the_meter_is_stopped_and_never_stored() {
        assert_eq!(meter_start(true), (false, false));
    }

    /// class 3: an aux capture in call session 7 finishes after the call's teardown drained the slots
    fn aux_after_teardown(enforce: bool) -> (bool, bool) {
        set_enforce(enforce);
        set_barred(false);
        set_session(7);
        let lease = take(Kind::ShareAudio, Some(7)).expect("admitted");
        let capturing = Arc::new(Mutex::new(false));
        let (c1, c2) = (capturing.clone(), capturing.clone());
        assert!(start_under(&lease, move || *c1.lock().unwrap() = false, move || {
            *c2.lock().unwrap() = true;
            true
        }));
        end_session(7); // close(): phase 1 — the slots are drained now
        let stored = Arc::new(Mutex::new(false));
        let s2 = stored.clone();
        let r = lease.commit_with(move || *s2.lock().unwrap() = true);
        let out = (r.is_some() && *stored.lock().unwrap(), *capturing.lock().unwrap());
        set_enforce(true);
        out
    }
    #[test]
    fn class3_red_without_leases_the_capture_registers_after_the_teardown() {
        assert_eq!(aux_after_teardown(false), (true, true));
    }
    #[test]
    fn class3_green_the_capture_is_nulled_and_never_registered() {
        assert_eq!(aux_after_teardown(true), (false, false));
    }

    /// class 5: a screen-quality restart of call A is awaiting the old track's unpublish (holding the call mutex) when
    /// Leave + Watch B are pressed (the Watch is QUEUED behind A's teardown) — does it re-publish into A?
    fn restart_with_queued_watch(enforce: bool) -> u32 {
        set_enforce(enforce);
        set_barred(false);
        set_session(11);
        let lease = take(Kind::Screen, Some(11)).expect("admitted");
        // ... 2) unpublish the old track (await) — meanwhile, on the Qt thread: Leave, then Watch B (queued)
        set_barred(true); // the press closes the bar for a queued watch too
        revoke("watch");
        let mut republished = 0;
        // 3) the restart's check before re-publishing
        if lease.valid() {
            republished += 1;
            // 4) the capture restart under the lease, and the commit
            if start_under(&lease, || {}, || true) && lease.commit() {
                republished += 1;
            }
        }
        set_enforce(true);
        republished
    }
    #[test]
    fn class5_red_without_leases_the_old_call_gets_a_new_share() {
        assert_eq!(restart_with_queued_watch(false), 2);
    }
    #[test]
    fn class5_green_nothing_is_republished_into_the_old_call() {
        assert_eq!(restart_with_queued_watch(true), 0);
    }

    #[test]
    fn a_revoke_counts_as_pending_until_its_stops_ran_and_the_lease_ended() {
        set_barred(false);
        let lease = take(Kind::Meter, None).unwrap();
        assert!(lease.hold(|| {}));
        assert_eq!(pending(), 1);
        revoke("watch");
        assert_eq!(pending(), 1, "the operation hasn't ended yet");
        drop(lease);
        assert_eq!(pending(), 0);
    }


    /// round 6 (Astra r5 P1): a pause between "everything before the start is done" and the platform start, with the
    /// Watch press IN the pause. Opened = how many platform starts ran after the press.
    fn pause_then_press(r5: bool) -> (u32, bool) {
        set_enforce(true);
        set_barred(false);
        let lease = take(Kind::Meter, None).expect("admitted");
        PAUSE.with(|p| {
            *p.borrow_mut() = Some(Box::new(|| {
                set_barred(true); // the Watch press, in the pause
                revoke("watch");
            }))
        });
        let opened = Arc::new(Mutex::new(0u32));
        let o = opened.clone();
        let start = move || {
            *o.lock().unwrap() += 1; // the platform start (pipewiresrc PLAYING / getUserMedia)
            true
        };
        let running = if r5 { start_under_r5(&lease, || {}, start) } else { start_under(&lease, || {}, start) };
        PAUSE.with(|p| *p.borrow_mut() = None);
        set_barred(false);
        let n = *opened.lock().unwrap();
        (n, running)
    }
    #[test]
    fn round6_red_round5_start_under_opens_the_capture_after_a_press_in_the_pause() {
        assert_eq!(pause_then_press(true).0, 1, "round 5: the start ran after the press");
    }
    #[test]
    fn round6_green_no_platform_start_after_a_press_in_the_pause() {
        assert_eq!(pause_then_press(false), (0, false));
    }
    #[test]
    fn round6_a_press_during_the_platform_start_is_stopped_by_the_worker_not_the_revoker() {
        set_barred(false);
        let lease = take(Kind::Meter, None).unwrap();
        let stops = Arc::new(Mutex::new(Vec::<&str>::new()));
        let s1 = stops.clone();
        let s2 = stops.clone();
        let running = start_under(&lease, move || s1.lock().unwrap().push("worker"), move || {
            set_barred(true);
            revoke("watch"); // the press while the platform call runs: nothing is stopped by the revoke
            s2.lock().unwrap().push("start returned");
            true
        });
        set_barred(false);
        assert!(!running);
        assert_eq!(*stops.lock().unwrap(), vec!["start returned", "worker"], "the worker stops it, after the start");
    }


    #[test]
    fn round8_a_revoke_closes_the_gate_of_a_capture_already_handed_to_its_owner() {
        set_barred(false);
        let lease = take(Kind::Meter, None).unwrap();
        let gate = new_gate();
        assert!(start_under_gated(&lease, Some(gate.clone()), || {}, || true));
        assert!(lease.commit(), "handed to its owner");
        drop(lease);
        assert!(gate.load(std::sync::atomic::Ordering::SeqCst), "open while it runs");
        revoke("watch");
        assert!(!gate.load(std::sync::atomic::Ordering::SeqCst), "closed at the press, before its owner stops it");
    }

}
