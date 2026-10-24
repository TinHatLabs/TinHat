//! Game Reel ("What do we play tonight", 9.0) — the PURE fairness model: pool tiers, per-person
//! shares, regret credit, the verifiable winner draw, spin-record resolution and the explain
//! strings. No Qt, no Matrix, no clock: every function is a deterministic function of its
//! arguments, so the web client can be held to the same test vectors (DESIGN.md §12 gates) and
//! games.rs stays a thin I/O layer. Section numbers (§) refer to
//! the Game Reel DESIGN.md.
//!
//! Weights cross to the record as INTEGER micro-units (`to_micro`, 10.5.1): Synapse refuses JSON
//! floats in room-v10 state, so the 10.5.0 round6 floats made every spin fail on a live server.
//! Readers are scale-free (odds = w/Σw, `derive_winner` walks r·Σw), so every client — 10.5.0,
//! 10.5.1 and the web — sums the SAME doubles when it re-derives a winner from a spin record.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------------------------
// Prefs (§7.1)
// ---------------------------------------------------------------------------------------------

/// One person's `dev.tinhat.games.prefs` content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefs {
    pub would_play: Vec<String>,
    pub favorites: Vec<String>,
    pub definitely_not: Vec<String>,
    pub flexible: bool,
    pub updated_at: u64,
    /// 10.1 (TH-REEL-04): id → display name, as the editor learned them from search / import.
    /// Round-tripped so "My games" reads as names after a restart instead of `892970`.
    pub names: std::collections::BTreeMap<String, String>,
}

fn str_list(v: &Value, key: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(a) = v.get(key).and_then(Value::as_array) {
        for s in a.iter().filter_map(Value::as_str) {
            let s = s.trim();
            if !s.is_empty() && !out.iter().any(|x| x == s) {
                out.push(s.to_string());
            }
        }
    }
    out
}

impl Prefs {
    pub fn from_json(v: &Value) -> Self {
        Self {
            would_play: str_list(v, "wouldPlay"),
            favorites: str_list(v, "favorites"),
            definitely_not: str_list(v, "definitelyNot"),
            flexible: v.get("flexible").and_then(Value::as_bool).unwrap_or(false),
            updated_at: v.get("updatedAt").and_then(Value::as_u64).unwrap_or(0),
            // Either `{id: "Name"}` or the editor's older `{id: {name: "Name"}}` shape.
            names: v
                .get("names")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, val)| {
                            val.as_str()
                                .map(|s| s.to_string())
                                .or_else(|| val.get("name").and_then(Value::as_str).map(|s| s.to_string()))
                                .map(|n| (k.clone(), n))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Normalised for writing: a game is never both would-play and definitely-not (the refusal
    /// wins), favorites ⊆ would-play and at most `slots` of them (§6 `favorite.slots`).
    pub fn normalized(mut self, slots: u32) -> Self {
        let not: HashSet<String> = self.definitely_not.iter().cloned().collect();
        self.would_play.retain(|g| !not.contains(g));
        let wp: HashSet<String> = self.would_play.iter().cloned().collect();
        self.favorites.retain(|g| wp.contains(g));
        self.favorites.truncate(slots.max(1) as usize);
        self
    }

    pub fn to_json(&self) -> Value {
        json!({
            "v": 1,
            "wouldPlay": self.would_play,
            "favorites": self.favorites,
            "definitelyNot": self.definitely_not,
            "flexible": self.flexible,
            "updatedAt": self.updated_at,
            "names": self.names,
        })
    }

    /// Nothing to say: writing `{}` removes the person from every future pool (§10).
    pub fn is_empty(&self) -> bool {
        self.would_play.is_empty() && self.definitely_not.is_empty() && !self.flexible
    }

    /// The primary favorite (slot 1) — the one that carries regret credit.
    pub fn favorite(&self) -> Option<&str> {
        self.favorites.first().map(String::as_str)
    }
}

// ---------------------------------------------------------------------------------------------
// Settings (§6) — one flat JSON object with dotted keys, exactly the table in the design.
// ---------------------------------------------------------------------------------------------

/// `pool.min_owners` / `pool.stretch_rule`: "all" | "all-but-1" | N (≥ N owners).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerRule {
    All,
    AllButOne,
    AtLeast(u32),
}

impl OwnerRule {
    fn from_json(v: Option<&Value>, default: Self) -> Self {
        match v {
            Some(Value::String(s)) => {
                let s = s.trim().to_ascii_lowercase();
                match s.as_str() {
                    "all" | "all present" => Self::All,
                    "all-but-1" | "all-but-one" | "all but one" => Self::AllButOne,
                    _ => s
                        .trim_start_matches(|c: char| c == '≥' || c == '>' || c == '=' || c == ' ')
                        .parse::<u32>()
                        .map(|n| Self::AtLeast(n.clamp(1, 99)))
                        .unwrap_or(default),
                }
            }
            Some(Value::Number(n)) => n.as_u64().map(|n| Self::AtLeast(n.clamp(1, 99) as u32)).unwrap_or(default),
            _ => default,
        }
    }

    pub fn to_json(self) -> Value {
        match self {
            Self::All => json!("all"),
            Self::AllButOne => json!("all-but-1"),
            Self::AtLeast(n) => json!(n),
        }
    }

    /// Owners needed out of `n` counted people. Never below 1: a game nobody lists is never in.
    pub fn threshold(self, n: usize) -> usize {
        match self {
            Self::All => n.max(1),
            Self::AllButOne => n.saturating_sub(1).max(1),
            Self::AtLeast(k) => (k as usize).max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VetoMode {
    Hard,
    Soft,
    Off,
}

/// `veto.mode = soft`: a definitely-not crushes the weight instead of removing the game (§6).
pub const SOFT_VETO_FACTOR: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// 10.5.1 default (`"open"`): everyone in the call counts, RSVP or not, one person
    /// included; outside a call the planned roster + the viewer. See [`roster`].
    Open,
    /// yes/maybe RSVPs who are actually here (the 9.0–10.5.0 default; §2 "actual roster").
    RsvpPresent,
    RsvpOnly,
    CallOnly,
}

impl Presence {
    pub fn as_str(self) -> &'static str {
        match self {
            Presence::Open => "open",
            Presence::RsvpPresent => "rsvp+present",
            Presence::RsvpOnly => "rsvp-only",
            Presence::CallOnly => "call-only",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OddsVisibility {
    Full,
    BadgesOnly,
    Hidden,
}

/// 10.5.2 `credits.mode` (2026-09-23: a toggle between spending credits and free
/// spins): `Earn` (default, the behaviour so far) = a lost favourite earns regret credit and a winning
/// one spends it; `Free` = free spins — every spin record written while it is on carries
/// `"credit": false` (captured at write time, so flipping the mode never rewrites history) and
/// tonight's draw ignores banked credit (no "weighted up", no +N badges). The web's `CreditsMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreditsMode {
    Earn,
    Free,
}

impl CreditsMode {
    pub fn as_str(self) -> &'static str {
        match self {
            CreditsMode::Earn => "earn",
            CreditsMode::Free => "free",
        }
    }
}

/// Every tunable of §6 plus `rev`. Defaults are what ships.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub rev: u64,
    pub feature_enabled: bool,
    pub favorite_slots: u32,
    pub favorite_share: f64,
    pub favorite_second_share: f64,
    pub regret_step: f64,
    pub regret_cap: u32,
    pub regret_count_maybe: bool,
    /// `off` = None; `Some(n)` = the streak drops by one per n missed nights.
    pub regret_absence_decay: Option<u32>,
    pub regret_reset_on_favorite_change: bool,
    pub flexible_weight: f64,
    pub pool_min_owners: OwnerRule,
    pub pool_stretch_rule: OwnerRule,
    pub pool_spin_stretch: bool,
    pub pool_count_flexible_ownership: bool,
    pub veto_mode: VetoMode,
    pub veto_post_spin_per_person: u32,
    pub veto_respin_limit: u32,
    pub damping_last_winner: bool,
    pub damping_factor: f64,
    pub damping_sessions: u32,
    /// `off` = None; `Some(b)` = weight × (1 + b × fraction of present people listing the game).
    pub consensus_bonus: Option<f64>,
    pub reel_min_items: u32,
    pub reel_max_items: u32,
    pub reel_duration_ms: u32,
    pub reel_laps: u32,
    pub roster_presence: Presence,
    pub roster_grace_min: u32,
    pub odds_visibility: OddsVisibility,
    pub steam_import: bool,
    pub steam_multiplayer_filter: bool,
    pub steam_show_playtime: bool,
    pub catalog_custom_entries: bool,
    pub admin_min_pl: i64,
    pub credits_mode: CreditsMode,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            rev: 0,
            feature_enabled: true,
            favorite_slots: 1,
            favorite_share: 0.5,
            favorite_second_share: 0.25,
            regret_step: 0.5,
            regret_cap: 3,
            regret_count_maybe: true,
            regret_absence_decay: None,
            regret_reset_on_favorite_change: true,
            flexible_weight: 0.0,
            pool_min_owners: OwnerRule::All,
            pool_stretch_rule: OwnerRule::AllButOne,
            pool_spin_stretch: false,
            pool_count_flexible_ownership: true,
            veto_mode: VetoMode::Hard,
            veto_post_spin_per_person: 1,
            veto_respin_limit: 2,
            damping_last_winner: false,
            damping_factor: 0.5,
            damping_sessions: 1,
            consensus_bonus: None,
            reel_min_items: 2,
            reel_max_items: 16,
            reel_duration_ms: 6000,
            reel_laps: 5,
            roster_presence: Presence::Open,
            roster_grace_min: 0,
            odds_visibility: OddsVisibility::Full,
            steam_import: true,
            steam_multiplayer_filter: true,
            steam_show_playtime: false,
            catalog_custom_entries: true,
            admin_min_pl: 50,
            credits_mode: CreditsMode::Earn,
        }
    }
}

/// Typed, range-clamped reads of the flat settings object.
struct Reader<'a>(&'a Value);

impl<'a> Reader<'a> {
    fn b(&self, key: &str, default: bool) -> bool {
        match self.0.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
                "on" | "true" | "yes" | "1" => true,
                "off" | "false" | "no" | "0" => false,
                _ => default,
            },
            Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(default),
            _ => default,
        }
    }
    fn f(&self, key: &str, default: f64, lo: f64, hi: f64) -> f64 {
        self.0.get(key).and_then(Value::as_f64).filter(|f| f.is_finite()).unwrap_or(default).clamp(lo, hi)
    }
    fn u(&self, key: &str, default: u32, lo: u32, hi: u32) -> u32 {
        self.0
            .get(key)
            .and_then(Value::as_f64)
            .filter(|f| f.is_finite())
            .map(|f| f.round().max(0.0) as u32)
            .unwrap_or(default)
            .clamp(lo, hi)
    }
    fn s(&self, key: &str) -> Option<String> {
        self.0.get(key).and_then(Value::as_str).map(|s| s.trim().to_ascii_lowercase())
    }
    /// `off`/false/absent → None; a number → Some(n).
    fn opt_u(&self, key: &str, lo: u32, hi: u32) -> Option<u32> {
        match self.0.get(key) {
            Some(Value::Number(n)) => n.as_f64().map(|f| (f.round().max(0.0) as u32).clamp(lo, hi)).filter(|n| *n > 0),
            Some(Value::String(s)) => s.trim().parse::<u32>().ok().map(|n| n.clamp(lo, hi)).filter(|n| *n > 0),
            _ => None,
        }
    }
    fn opt_f(&self, key: &str, lo: f64, hi: f64) -> Option<f64> {
        match self.0.get(key) {
            Some(Value::Number(n)) => n.as_f64().filter(|f| f.is_finite()).map(|f| f.clamp(lo, hi)),
            Some(Value::String(s)) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()).map(|f| f.clamp(lo, hi)),
            _ => None,
        }
    }
}

impl Settings {
    /// Read the settings state event content; unknown keys are ignored and missing ones take
    /// the default, so an older client's event and a newer client's reader always agree (§11).
    pub fn from_json(v: &Value) -> Self {
        let d = Self::default();
        let r = Reader(v);
        Self {
            rev: v.get("rev").and_then(Value::as_u64).unwrap_or(0),
            feature_enabled: r.b("feature.enabled", d.feature_enabled),
            favorite_slots: r.u("favorite.slots", d.favorite_slots, 1, 2),
            favorite_share: r.f("favorite.share", d.favorite_share, 0.2, 0.9),
            favorite_second_share: r.f("favorite.second_share", d.favorite_second_share, 0.0, 0.5),
            regret_step: r.f("regret.step", d.regret_step, 0.0, 2.0),
            regret_cap: r.u("regret.cap", d.regret_cap, 0, 10),
            regret_count_maybe: r.b("regret.count_maybe", d.regret_count_maybe),
            regret_absence_decay: r.opt_u("regret.absence_decay", 1, 20),
            regret_reset_on_favorite_change: r.b("regret.reset_on_favorite_change", d.regret_reset_on_favorite_change),
            flexible_weight: r.f("flexible.weight", d.flexible_weight, 0.0, 0.5),
            pool_min_owners: OwnerRule::from_json(v.get("pool.min_owners"), d.pool_min_owners),
            pool_stretch_rule: OwnerRule::from_json(v.get("pool.stretch_rule"), d.pool_stretch_rule),
            pool_spin_stretch: r.b("pool.spin_stretch", d.pool_spin_stretch),
            pool_count_flexible_ownership: r.b("pool.count_flexible_ownership", d.pool_count_flexible_ownership),
            veto_mode: match r.s("veto.mode").as_deref() {
                Some("soft") => VetoMode::Soft,
                Some("off") => VetoMode::Off,
                Some("hard") => VetoMode::Hard,
                _ => d.veto_mode,
            },
            veto_post_spin_per_person: r.u("veto.post_spin_per_person", d.veto_post_spin_per_person, 0, 3),
            veto_respin_limit: r.u("veto.respin_limit", d.veto_respin_limit, 0, 5),
            damping_last_winner: r.b("damping.last_winner", d.damping_last_winner),
            damping_factor: r.f("damping.factor", d.damping_factor, 0.1, 1.0),
            damping_sessions: r.u("damping.sessions", d.damping_sessions, 1, 5),
            consensus_bonus: r.opt_f("consensus.bonus", 0.0, 1.0),
            reel_min_items: r.u("reel.min_items", d.reel_min_items, 1, 4),
            reel_max_items: r.u("reel.max_items", d.reel_max_items, 6, 30),
            reel_duration_ms: r.u("reel.duration_ms", d.reel_duration_ms, 2000, 12000),
            reel_laps: r.u("reel.laps", d.reel_laps, 2, 10),
            roster_presence: match r.s("roster.presence").as_deref() {
                Some("rsvp-only") => Presence::RsvpOnly,
                Some("call-only") => Presence::CallOnly,
                Some("rsvp+present") => Presence::RsvpPresent,
                Some("open") => Presence::Open,
                _ => d.roster_presence,
            },
            roster_grace_min: r.u("roster.grace_min", d.roster_grace_min, 0, 30),
            odds_visibility: match r.s("odds.visibility").as_deref() {
                Some("badges-only") => OddsVisibility::BadgesOnly,
                Some("hidden") => OddsVisibility::Hidden,
                Some("full") => OddsVisibility::Full,
                _ => d.odds_visibility,
            },
            steam_import: r.b("steam.import", d.steam_import),
            steam_multiplayer_filter: r.b("steam.multiplayer_filter", d.steam_multiplayer_filter),
            steam_show_playtime: r.b("steam.show_playtime", d.steam_show_playtime),
            catalog_custom_entries: r.b("catalog.custom_entries", d.catalog_custom_entries),
            admin_min_pl: v.get("admin.min_pl").and_then(Value::as_i64).unwrap_or(d.admin_min_pl).clamp(0, 100),
            credits_mode: match r.s("credits.mode").as_deref() {
                Some("free") => CreditsMode::Free,
                Some("earn") => CreditsMode::Earn,
                _ => d.credits_mode,
            },
        }
    }

    /// The flat object the settings event stores (every key, always — the admin panel shows
    /// default / current side by side and Matrix state history is the audit diff).
    pub fn to_json(&self) -> Value {
        json!({
            "rev": self.rev,
            "feature.enabled": self.feature_enabled,
            "favorite.slots": self.favorite_slots,
            "favorite.share": self.favorite_share,
            "favorite.second_share": self.favorite_second_share,
            "regret.step": self.regret_step,
            "regret.cap": self.regret_cap,
            "regret.count_maybe": self.regret_count_maybe,
            "regret.absence_decay": self.regret_absence_decay.map(Value::from).unwrap_or(Value::Bool(false)),
            "regret.reset_on_favorite_change": self.regret_reset_on_favorite_change,
            "flexible.weight": self.flexible_weight,
            "pool.min_owners": self.pool_min_owners.to_json(),
            "pool.stretch_rule": self.pool_stretch_rule.to_json(),
            "pool.spin_stretch": self.pool_spin_stretch,
            "pool.count_flexible_ownership": self.pool_count_flexible_ownership,
            "veto.mode": match self.veto_mode { VetoMode::Hard => "hard", VetoMode::Soft => "soft", VetoMode::Off => "off" },
            "veto.post_spin_per_person": self.veto_post_spin_per_person,
            "veto.respin_limit": self.veto_respin_limit,
            "damping.last_winner": self.damping_last_winner,
            "damping.factor": self.damping_factor,
            "damping.sessions": self.damping_sessions,
            "consensus.bonus": self.consensus_bonus.map(Value::from).unwrap_or(Value::Bool(false)),
            "reel.min_items": self.reel_min_items,
            "reel.max_items": self.reel_max_items,
            "reel.duration_ms": self.reel_duration_ms,
            "reel.laps": self.reel_laps,
            "roster.presence": self.roster_presence.as_str(),
            "roster.grace_min": self.roster_grace_min,
            "odds.visibility": match self.odds_visibility { OddsVisibility::Full => "full", OddsVisibility::BadgesOnly => "badges-only", OddsVisibility::Hidden => "hidden" },
            "steam.import": self.steam_import,
            "steam.multiplayer_filter": self.steam_multiplayer_filter,
            "steam.show_playtime": self.steam_show_playtime,
            "catalog.custom_entries": self.catalog_custom_entries,
            "admin.min_pl": self.admin_min_pl,
            "credits.mode": self.credits_mode.as_str(),
        })
    }

    /// These settings with `patch`'s keys laid over them (the admin panel saves a partial
    /// object; the simulator dry-runs one). `rev` is NOT bumped here.
    pub fn merged(&self, patch: &Value) -> Self {
        let mut j = self.to_json();
        if let (Some(obj), Some(p)) = (j.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                obj.insert(k.clone(), v.clone());
            }
        }
        Self::from_json(&j)
    }

    /// 10.5.1 — what the settings EVENT stores (the web's `settingsToWire`, TW DECISIONS D8):
    /// Synapse refuses any JSON float in room-v10 state, and serde writes every f64 as one
    /// (`0.0`, `0.5`), so 10.5.0's "Tune" saves always failed. Integral / bool / string keys are
    /// written as they are (an integral f64 as a JSON integer); a fractional key still at its
    /// default is OMITTED (every reader defaults it); `consensus.bonus` travels as a numeric
    /// STRING (the one fractional key both readers parse from a string); any other non-default
    /// fraction can't be stored → [`FLOAT_REFUSAL`].
    pub fn to_wire(&self) -> Result<Value, String> {
        let full = self.to_json();
        let defs = Self::default().to_json();
        let mut out = serde_json::Map::new();
        for (k, v) in full.as_object().into_iter().flatten() {
            if let Some(f) = v.as_f64().filter(|_| v.is_f64()) {
                if f.is_finite() && f.fract() == 0.0 {
                    out.insert(k.clone(), json!(f as i64));
                    continue;
                }
                if FRACTIONAL_KEYS.contains(&k.as_str()) && defs.get(k).and_then(Value::as_f64) == Some(f) {
                    continue;
                }
                if k == "consensus.bonus" {
                    out.insert(k.clone(), json!(f.to_string()));
                    continue;
                }
                return Err(FLOAT_REFUSAL.to_string());
            }
            out.insert(k.clone(), v.clone());
        }
        Ok(Value::Object(out))
    }
}

/// The tunables whose defaults are fractions (see [`Settings::to_wire`]).
pub const FRACTIONAL_KEYS: [&str; 6] = ["favorite.share", "favorite.second_share", "regret.step", "flexible.weight", "damping.factor", "consensus.bonus"];
/// Shown when a non-default fraction can't be stored (same words as the web's Tune panel).
pub const FLOAT_REFUSAL: &str = "Matrix can't store fractional numbers yet — keep the default";

// ---------------------------------------------------------------------------------------------
// Spin records (§7.2) + adjustments (§7.4)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Voided {
    pub by: String,
    pub reason: String,
    pub at: u64,
}

/// One `dev.tinhat.games.spin` state event. Wire fields per §7.2 plus three small extensions
/// (all ignorable by an older client): `seed` (the event id of the FIRST write — the second
/// write REPLACES the state event, so the seed must ride along), `vetoBy` (who used their
/// post-spin veto on what, for `veto.post_spin_per_person`), `late` (grace arrivals appended
/// to `roster` after the spin, `roster.grace_min`) and `maybes` (roster members who had
/// answered "maybe", for `regret.count_maybe`).
#[derive(Clone, Debug, PartialEq)]
pub struct SpinRecord {
    /// state_key `sp-<ms>-<hex>`
    pub id: String,
    /// event id of the state event as it is in the store NOW (changes on every rewrite)
    pub event_id: String,
    /// event id of the first write = the winner seed ("" = `event_id` is the first write)
    pub seed: String,
    /// origin_server_ts of the seed event — the concurrency clock (§7.2)
    pub ts: u64,
    pub sender: String,
    pub session_id: String,
    /// `initial` | `respin`
    pub kind: String,
    pub supersedes: Option<String>,
    pub vetoed: Vec<String>,
    pub veto_by: HashMap<String, String>,
    pub roster: Vec<String>,
    pub late: Vec<String>,
    pub maybes: Vec<String>,
    pub flexible: Vec<String>,
    pub favorites: HashMap<String, String>,
    pub pool: Vec<(String, f64)>,
    pub settings_rev: u64,
    /// what the sender wrote (absent until its second write, or forever if it disconnected)
    pub winner: Option<String>,
    pub voided: Option<Voided>,
    /// 10.5.2: false = this spin changes nobody's regret credit (wire `"credit": false`; absent =
    /// counts, so every older record counts). Written in write 1 by a spin in free mode; the reader
    /// keeps the spinner's own value unless an admin wrote the current copy ([`trusted_credit_flag`]),
    /// then applies an admin's "Don't count for credit" / "Count for credit" ([`credit_overrides`]).
    /// The result itself stands — unlike a void it still wins, verifies and damps. `Default` = true
    /// (counts), like an absent key.
    pub credit: bool,
    // ---- derived by the reader, never written ----
    /// what THIS client derives from the seed — the only winner it trusts
    pub computed_winner: Option<String>,
    /// The seed event WAS fetched and disagreed with this record (pool, roster, session or a
    /// winner the sender made up). Distinct from `!verified`, which is also true when the seed
    /// simply has not been fetched — a transient state that must not rewrite credit history.
    /// A tampered record is shown struck through and drives nothing.
    pub tampered: bool,
    /// the seed event was fetched and its pool/roster match this record, and `winner` (if
    /// written) equals `computed_winner`
    pub verified: bool,
}

impl Default for SpinRecord {
    /// Everything empty; `kind` "" and `credit` TRUE (a record counts unless it says otherwise).
    fn default() -> Self {
        Self {
            id: String::new(),
            event_id: String::new(),
            seed: String::new(),
            ts: 0,
            sender: String::new(),
            session_id: String::new(),
            kind: String::new(),
            supersedes: None,
            vetoed: Vec::new(),
            veto_by: HashMap::new(),
            roster: Vec::new(),
            late: Vec::new(),
            maybes: Vec::new(),
            flexible: Vec::new(),
            favorites: HashMap::new(),
            pool: Vec::new(),
            settings_rev: 0,
            winner: None,
            voided: None,
            credit: true,
            computed_winner: None,
            tampered: false,
            verified: false,
        }
    }
}

impl SpinRecord {
    pub fn from_json(state_key: &str, event_id: &str, ts: u64, sender: &str, c: &Value) -> Self {
        let map_str = |key: &str| -> HashMap<String, String> {
            c.get(key)
                .and_then(Value::as_object)
                .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                .unwrap_or_default()
        };
        Self {
            id: state_key.to_string(),
            event_id: event_id.to_string(),
            seed: c.get("seed").and_then(Value::as_str).unwrap_or("").to_string(),
            ts,
            sender: sender.to_string(),
            session_id: c.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string(),
            kind: c.get("kind").and_then(Value::as_str).unwrap_or("initial").to_string(),
            supersedes: c.get("supersedes").and_then(Value::as_str).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            vetoed: str_list(c, "vetoed"),
            veto_by: map_str("vetoBy"),
            roster: str_list(c, "roster"),
            late: str_list(c, "late"),
            maybes: str_list(c, "maybes"),
            flexible: str_list(c, "flexible"),
            favorites: map_str("favorites"),
            pool: c
                .get("pool")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|g| Some((g.get("id")?.as_str()?.to_string(), g.get("weight").and_then(Value::as_f64).unwrap_or(0.0))))
                        .collect()
                })
                .unwrap_or_default(),
            settings_rev: c.get("settingsRev").and_then(Value::as_u64).unwrap_or(0),
            winner: c.get("winner").and_then(Value::as_str).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            voided: c.get("voided").filter(|v| v.is_object()).map(|v| Voided {
                by: v.get("by").and_then(Value::as_str).unwrap_or("").to_string(),
                reason: v.get("reason").and_then(Value::as_str).unwrap_or("").to_string(),
                at: v.get("at").and_then(Value::as_u64).unwrap_or(0),
            }),
            // only a literal `false` disables (the web's `c.credit !== false`)
            credit: c.get("credit") != Some(&Value::Bool(false)),
            computed_winner: None,
            verified: false,
            tampered: false,
        }
    }

    /// The wire content (derived fields left out).
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "v": 1,
            "sessionId": self.session_id,
            "kind": self.kind,
            "supersedes": self.supersedes,
            "vetoed": self.vetoed,
            "vetoBy": self.veto_by,
            "roster": self.roster,
            "late": self.late,
            "maybes": self.maybes,
            "flexible": self.flexible,
            "favorites": self.favorites,
            // 10.5.1: `do_spin` builds the pool in integer micro-units ([`to_micro`]) and they go
            // out as JSON INTEGERS — serde would write 750000.0, a float Synapse refuses in room-v10
            // state (the 10.5.0 bug: every native spin failed). A fraction (only a record read from
            // a pre-10.5.1 float spin) keeps its exact value; it is never re-scaled here.
            "pool": self.pool.iter().map(|(id, w)| json!({ "id": id, "weight": weight_json(*w) })).collect::<Vec<_>>(),
            "settingsRev": self.settings_rev,
        });
        if !self.seed.is_empty() {
            v["seed"] = json!(self.seed);
        }
        if let Some(w) = &self.winner {
            v["winner"] = json!(w);
        }
        if let Some(x) = &self.voided {
            v["voided"] = json!({ "by": x.by, "reason": x.reason, "at": x.at });
        }
        // 10.5.2: only the exception travels, so a counting record is byte-identical to a 10.5.1 one
        if !self.credit {
            v["credit"] = json!(false);
        }
        v
    }

    /// The event id the winner is derived from.
    pub fn seed_id(&self) -> &str {
        if self.seed.is_empty() {
            &self.event_id
        } else {
            &self.seed
        }
    }

    /// The winner this client believes: its own derivation, or (before derivation) the
    /// sender's claim.
    pub fn effective_winner(&self) -> Option<&str> {
        self.computed_winner.as_deref().or(self.winner.as_deref())
    }

    /// The roster as it was at the spin (grace arrivals excluded) — what the seed event holds.
    pub fn roster_at_spin(&self) -> Vec<String> {
        self.roster.iter().filter(|p| !self.late.contains(p)).cloned().collect()
    }

    pub fn in_pool(&self, id: &str) -> bool {
        self.pool.iter().any(|(g, _)| g == id)
    }
}

/// `dev.tinhat.games.adjust` content: `{delta, reason, asOf}` and, from reset-all,
/// `{set: 0, asOf}` (§7.4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Adjustment {
    pub delta: i64,
    /// Present = the derived count restarts from this value at `as_of` (spins at or before
    /// `as_of` no longer count).
    pub set: Option<i64>,
    pub as_of: u64,
    pub reason: String,
    /// 10.5.2: origin_server_ts of the adjust event (set by [`merge_adjustments`]; 0 = unknown) —
    /// the server clock the spins' `ts` use too, so "which came first" never depends on a client's
    /// clock.
    pub written: u64,
}

impl Adjustment {
    pub fn from_json(v: &Value) -> Self {
        Self {
            delta: v.get("delta").and_then(Value::as_i64).unwrap_or(0),
            set: v.get("set").and_then(Value::as_i64),
            as_of: v.get("asOf").and_then(Value::as_u64).unwrap_or(0),
            reason: v.get("reason").and_then(Value::as_str).unwrap_or("").to_string(),
            written: 0,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 10.5.2 admin credit adjustments that actually land (2026-09-23: admins couldn't reliably
// undo the spin cost) — the web's model/games.ts block of the same name, same vectors
// ---------------------------------------------------------------------------------------------
//
// Up to 10.5.1 an adjustment lived at state_key = the TARGET's user id. Matrix (room v10 auth rules)
// refuses a state_key starting with "@" unless it is the sender's own id, so an admin could only ever
// adjust / reset their OWN credit ("Reset all credits" stopped at the first other person). From 10.5.2
// admins write `dev.tinhat.games.adjust` at state_key "admin:<target mxid>" ([`admin_adjust_key`]),
// which any sender with the event's power level may write. Readers ([`merge_adjustments`]):
//   - honour an event only if its SENDER is an admin NOW (power level ≥ settings admin.min_pl in the
//     container) — the event's own power level may be lowered, so this is what stops a member from
//     crediting themselves; the same rule applies to the legacy self-key (state_key == sender);
//   - per person, the adjustment written LAST (origin_server_ts; a tie goes to the admin key) is the
//     one that counts — it replaces the other one completely. Writers always start from that merged
//     value ([`adjust_content`]), so a reset line (`set` + `asOf`) is carried forward, never lost.
// Credit ([`credit_detail`]): min(losses since the reset line, cap) + (set + delta) floored at 0, as
// before, except that the offset `set + delta` LAPSES once the streak it was given in has ended — a
// night after the adjustment on which the person's favourite won, or their favourite changed (with
// regret.reset_on_favorite_change) — so an admin's "+1" is spent by the next win instead of sticking
// forever. A reset (set 0, delta 0) carries no offset, so this changes nothing for it.

/// State-key prefix of an admin's adjustment of someone else's credit (10.5.2).
pub const ADMIN_ADJUST_PREFIX: &str = "admin:";

pub fn admin_adjust_key(mxid: &str) -> String {
    format!("{ADMIN_ADJUST_PREFIX}{mxid}")
}

/// (target mxid, is the admin key) of an adjust state key; None = not an adjustment of a person.
pub fn adjust_target(state_key: &str) -> Option<(String, bool)> {
    match state_key.strip_prefix(ADMIN_ADJUST_PREFIX) {
        Some(t) => t.starts_with('@').then(|| (t.to_string(), true)),
        None => state_key.starts_with('@').then(|| (state_key.to_string(), false)),
    }
}

/// One `dev.tinhat.games.adjust` state event as the store holds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdjustEvent {
    pub state_key: String,
    pub sender: String,
    /// origin_server_ts
    pub ts: u64,
    pub content: Value,
}

/// The adjustment that counts for each person (see the block comment above).
pub fn merge_adjustments(events: &[AdjustEvent], is_admin: impl Fn(&str) -> bool) -> HashMap<String, Adjustment> {
    let mut best: HashMap<String, (Adjustment, bool)> = HashMap::new();
    for e in events {
        if !e.content.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
            continue;
        }
        let Some((target, admin_key)) = adjust_target(&e.state_key) else { continue };
        if !admin_key && e.sender != e.state_key {
            continue; // Matrix already refuses it; belt and braces
        }
        if !is_admin(&e.sender) {
            continue;
        }
        let adj = Adjustment { written: e.ts, ..Adjustment::from_json(&e.content) };
        let newer = match best.get(&target) {
            None => true,
            Some((cur, cur_admin)) => adj.written > cur.written || (adj.written == cur.written && admin_key && !cur_admin),
        };
        if newer {
            best.insert(target, (adj, admin_key));
        }
    }
    best.into_iter().map(|(k, (a, _))| (k, a)).collect()
}

/// The content of an admin's ±N for one person (written at [`admin_adjust_key`]): the merged
/// adjustment `cur` is carried forward while its offset is `live` ([`credit_detail`]'s
/// `offset_live`); a lapsed offset is not — the streak it belonged to is over, so the new offset
/// starts from 0 — but a reset line (`asOf`, with `set` 0 once lapsed) is always kept, so losses from
/// before a reset can never count again (e.g. after regret.reset_on_favorite_change is switched off).
pub fn adjust_content(cur: Option<&Adjustment>, live: bool, delta: i64, reason: &str, by: &str, now: u64) -> Value {
    let reset = cur.filter(|a| a.set.is_some());
    let mut content = json!({
        "delta": cur.filter(|_| live).map(|a| a.delta).unwrap_or(0) + delta,
        "asOf": reset.map(|a| a.as_of).unwrap_or(now),
        "reason": reason.trim(),
        "by": by,
        "at": now,
    });
    if let Some(a) = reset {
        content["set"] = json!(if live { a.set.unwrap_or(0) } else { 0 });
    }
    content
}

/// An admin's reset of one person's credit ("Zero" in Tune, and every line of "Reset all credits").
pub fn zero_content(reason: &str, by: &str, now: u64) -> Value {
    json!({ "set": 0, "delta": 0, "asOf": now, "reason": reason, "by": by, "at": now })
}

// ---------------------------------------------------------------------------------------------
// 10.5.2 "Don't count for credit" on one spin
// ---------------------------------------------------------------------------------------------

/// The credit flag a reader trusts from a spin RECORD. Spin records are writable by every member
/// (their power level is lowered so anyone can spin), so a rewrite by a non-admin may not change the
/// flag the spinner's first write (the seed event, when fetched) carries — a free-mode spin stays free,
/// a counted one stays counted. (An admin's rewrite — a void, a reset — is taken as is.)
pub fn trusted_credit_flag(current: bool, seed_flag: Option<bool>, writer_is_admin: bool) -> bool {
    match seed_flag {
        Some(f) if !writer_is_admin => f,
        _ => current,
    }
}

/// An admin's "Don't count for credit" / "Count for credit" lives OUTSIDE the member-writable spin
/// record: `dev.tinhat.games.adjust` state_key "credit:<spin id>" `{credit: bool, by, at}` — the adjust
/// type keeps the room's admin power level, and readers honour it only while its sender is an admin, so
/// no member rewrite of the record (not even an honest late-arrival append) can undo it. The record
/// itself is never touched, so it still verifies. It overrides the record's own flag (a free spin can be
/// made to count). The web's `CREDIT_OVERRIDE_PREFIX` / `creditOverrides`.
pub const CREDIT_OVERRIDE_PREFIX: &str = "credit:";

pub fn credit_override_key(spin_id: &str) -> String {
    format!("{CREDIT_OVERRIDE_PREFIX}{spin_id}")
}

pub fn credit_override_content(counts: bool, by: &str, now: u64) -> Value {
    json!({ "credit": counts, "by": by, "at": now })
}

/// spin id → counts, from admins' override events (an emptied event or a non-boolean `credit` = none).
pub fn credit_overrides(events: &[AdjustEvent], is_admin: impl Fn(&str) -> bool) -> HashMap<String, bool> {
    let mut out = HashMap::new();
    for e in events {
        let Some(id) = e.state_key.strip_prefix(CREDIT_OVERRIDE_PREFIX).filter(|id| !id.is_empty()) else { continue };
        let Some(counts) = e.content.get("credit").and_then(Value::as_bool) else { continue };
        if is_admin(&e.sender) {
            out.insert(id.to_string(), counts);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Roster (§2, §6 `roster.presence`) + tonight's session — 10.5.1 open roster
// ---------------------------------------------------------------------------------------------

/// Who is in tonight's reel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Roster {
    /// The planned roster: yes RSVPs (RSVP order), then maybes. The header's dimmed avatars.
    pub expected: Vec<String>,
    /// The ACTUAL roster — whose lists, vetoes, weights and credit count tonight.
    pub actual: Vec<String>,
    /// actual people who answered "maybe"
    pub maybes: Vec<String>,
    /// actual people whose prefs say Flexible
    pub flexible: Vec<String>,
}

/// §2 / §6 `roster.presence`. `rsvps` = (user, status) for the session in store order, `present`
/// = the call the viewer is in (the viewer first; empty outside a call), `me` = the viewer.
///
/// 10.5.1 (2026-09-23: the reel works with any number of people in the call, and everyone
/// who joins adds their preferences and games): the default `open`
/// counts EVERYONE in the call — RSVP or not, one person included — so a newcomer's lists and
/// favourites join the pool the moment they join, and someone who leaves drops out before the
/// spin (the SFU-truth call roster is live). Outside a call the reel previews the planned roster
/// plus the viewer (DESIGN §6: present = "in call, joining soon, or online in the app"), so it is
/// never empty for whoever is looking. Before 10.5.1 the default was `rsvp+present`: only people
/// who had RSVP'd AND were in the call counted — one friend without an RSVP emptied the reel.
/// The older modes stay for spaces that pick them explicitly.
pub fn roster(rsvps: &[(String, String)], present: &[String], me: &str, prefs: &HashMap<String, Prefs>, presence: Presence) -> Roster {
    let mut yes: Vec<String> = Vec::new();
    let mut maybe: Vec<String> = Vec::new();
    for (u, st) in rsvps {
        match st.as_str() {
            "yes" if !yes.contains(u) => yes.push(u.clone()),
            "maybe" if !maybe.contains(u) => maybe.push(u.clone()),
            _ => {}
        }
    }
    let mut expected: Vec<String> = yes;
    for m in &maybe {
        if !expected.contains(m) {
            expected.push(m.clone());
        }
    }
    let mut live: Vec<String> = Vec::new();
    for p in present {
        if p.starts_with('@') && !live.contains(p) {
            live.push(p.clone());
        }
    }
    let actual: Vec<String> = match presence {
        Presence::Open if !live.is_empty() => live,
        Presence::Open => {
            let mut a = expected.clone();
            if me.starts_with('@') && !a.iter().any(|p| p == me) {
                a.push(me.to_string());
            }
            a
        }
        Presence::RsvpPresent => expected.iter().filter(|p| live.contains(p)).cloned().collect(),
        Presence::RsvpOnly => expected.clone(),
        Presence::CallOnly => live,
    };
    let maybes = actual.iter().filter(|p| maybe.contains(p)).cloned().collect();
    let flexible = actual.iter().filter(|p| prefs.get(*p).map(|x| x.flexible).unwrap_or(false)).cloned().collect();
    Roster { expected, actual, maybes, flexible }
}

/// Prefix of a calendar-less "tonight" session id (see [`adhoc_session_id`]).
pub const ADHOC_PREFIX: &str = "night-";
/// The game-night day rolls over at 14:00 UTC: a fixed day boundary (morning across the Americas), so
/// everyone in a call derives the same id.
pub const NIGHT_ROLLOVER_UTC_H: u64 = 14;
const DAY_MS: u64 = 86_400_000;

/// 10.5.1: the session a spin belongs to when no calendar session is picked (no calendar event
/// tonight is no longer "Pick a session first."): `night-YYYY-MM-DD`, the UTC date of
/// `now − 14 h`. Every client computes the same id, so one standing spin per night per space;
/// there is no calendar event to write the winner back to.
pub fn adhoc_session_id(now_ms: u64) -> String {
    let day = (now_ms.saturating_sub(NIGHT_ROLLOVER_UTC_H * 3_600_000) / DAY_MS) as i64;
    // Howard Hinnant's civil_from_days (days since 1970-01-01 → proleptic Gregorian y-m-d).
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{ADHOC_PREFIX}{y:04}-{m:02}-{d:02}")
}

pub fn is_adhoc_session(id: &str) -> bool {
    id.starts_with(ADHOC_PREFIX)
}

/// When tonight's game-night day ends (the next rollover) — a calendar session only auto-picks
/// for the reel when it starts before this.
pub fn night_end_ms(now_ms: u64) -> u64 {
    let off = NIGHT_ROLLOVER_UTC_H * 3_600_000;
    (now_ms.saturating_sub(off) / DAY_MS + 1) * DAY_MS + off
}

/// The session the reel works on: the picked calendar session when it exists (or an ad-hoc id
/// already resolved), else tonight's ad-hoc session.
pub fn effective_session(picked: &str, known_event: bool, now_ms: u64) -> String {
    if !picked.is_empty() && (known_event || is_adhoc_session(picked)) {
        picked.to_string()
    } else {
        adhoc_session_id(now_ms)
    }
}

// ---------------------------------------------------------------------------------------------
// Pool (§5.2)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Main,
    Stretch,
    Removed,
}

/// Tonight's candidate games, tiered. Ids inside each tier are sorted for determinism; the
/// controller orders the reel by weight.
#[derive(Clone, Debug, Default)]
pub struct Pool {
    pub main: Vec<String>,
    pub stretch: Vec<String>,
    /// (game, reason string per §9: "Jo: definitely not." / "Not enough owners.")
    pub removed: Vec<(String, String)>,
    /// game → every present person listing it (flexible included) — the "N of you" count.
    pub owners: HashMap<String, Vec<String>>,
    /// game → counted present people NOT listing it — the stretch tier's "don't own it".
    pub missing: HashMap<String, Vec<String>>,
    /// game → present people with it in definitely-not.
    pub vetoers: HashMap<String, Vec<String>>,
    /// how many present people counted toward the ownership thresholds
    pub counted: usize,
}

impl Pool {
    pub fn tier(&self, id: &str) -> Option<Tier> {
        if self.main.iter().any(|g| g == id) {
            Some(Tier::Main)
        } else if self.stretch.iter().any(|g| g == id) {
            Some(Tier::Stretch)
        } else if self.removed.iter().any(|(g, _)| g == id) {
            Some(Tier::Removed)
        } else {
            None
        }
    }

    /// What the reel may draw from: the main tier, plus stretch when `pool.spin_stretch`.
    pub fn spinnable(&self, settings: &Settings) -> Vec<String> {
        let mut v = self.main.clone();
        if settings.pool_spin_stretch {
            v.extend(self.stretch.iter().cloned());
        }
        v
    }
}

/// Display name for a user id (falls back to the localpart).
pub fn display_name(names: &HashMap<String, String>, mxid: &str) -> String {
    names
        .get(mxid)
        .filter(|n| !n.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| mxid.trim_start_matches('@').split(':').next().unwrap_or(mxid).to_string())
}

/// "Kai" / "Kai and Sam" / "Kai, Sam and Jo".
pub fn join_names(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
    }
}

fn veto_reason(vetoers: &[String], names: &HashMap<String, String>) -> String {
    let n: Vec<String> = vetoers.iter().map(|p| display_name(names, p)).collect();
    format!("{}: definitely not.", join_names(&n))
}

/// §5.2: tier every game the present people list. `present` is the ACTUAL roster (the
/// controller has already applied `roster.presence`); `names` only feeds the reason strings.
pub fn build_pool(present: &[String], prefs: &HashMap<String, Prefs>, settings: &Settings, names: &HashMap<String, String>) -> Pool {
    let is_flex = |p: &String| prefs.get(p).map(|x| x.flexible).unwrap_or(false);
    // 10.5.1 open roster: someone who hasn't listed any games (no prefs, or only definitely-nots)
    // tells us nothing about what they own — they neither block a game nor count toward
    // "everyone owns" (a friend joining the call without lists used to empty the reel). Their
    // definitely-nots still veto below, and the moment they save their lists they count.
    let lists_games = |p: &String| prefs.get(p).map(|x| x.flexible || !x.would_play.is_empty()).unwrap_or(false);
    // Whose lists count toward "everyone owns": everyone here, or everyone but the flexible.
    let counted: Vec<String> = present.iter().filter(|p| lists_games(p)).filter(|p| settings.pool_count_flexible_ownership || !is_flex(p)).cloned().collect();
    let n = counted.len();
    let t_main = settings.pool_min_owners.threshold(n);
    let t_stretch = settings.pool_stretch_rule.threshold(n);

    let mut cands: Vec<String> = counted.iter().filter_map(|p| prefs.get(p)).flat_map(|x| x.would_play.iter().cloned()).collect();
    cands.sort();
    cands.dedup();

    let mut pool = Pool { counted: n, ..Default::default() };
    for g in cands {
        let lists = |p: &String| prefs.get(p).map(|x| x.would_play.iter().any(|w| w == &g)).unwrap_or(false);
        // §3.4: a Flexible person will play anything, so they own everything for the purpose of
        // the owner thresholds — a game is never excluded on their account. `owners_all` stays the
        // list of people who ACTUALLY listed it, because that is what the UI shows.
        let owns = |p: &String| lists(p) || is_flex(p);
        let owners_all: Vec<String> = present.iter().filter(|p| lists(p)).cloned().collect();
        let owners_counted = counted.iter().filter(|p| owns(p)).count();
        let missing: Vec<String> = counted.iter().filter(|p| !owns(p)).cloned().collect();
        let vetoers: Vec<String> = present
            .iter()
            .filter(|p| prefs.get(*p).map(|x| x.definitely_not.iter().any(|d| d == &g)).unwrap_or(false))
            .cloned()
            .collect();
        pool.owners.insert(g.clone(), owners_all);
        pool.missing.insert(g.clone(), missing);
        if !vetoers.is_empty() {
            pool.vetoers.insert(g.clone(), vetoers.clone());
        }
        if settings.veto_mode == VetoMode::Hard && !vetoers.is_empty() {
            // A present person's definitely-not removes it from EVERY tier (§2, §3.6).
            pool.removed.push((g, veto_reason(&vetoers, names)));
        } else if n > 0 && owners_counted >= t_main {
            pool.main.push(g);
        } else if n > 0 && owners_counted >= t_stretch {
            pool.stretch.push(g);
        } else {
            pool.removed.push((g, "Not enough owners.".to_string()));
        }
    }
    pool
}

// ---------------------------------------------------------------------------------------------
// Weights (§5.3–5.4)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameWeight {
    pub id: String,
    /// after damping / soft veto / consensus
    pub weight: f64,
    /// (person, share) — the per-person breakdown before the multipliers
    pub shares: Vec<(String, f64)>,
    pub damped: bool,
    pub soft_vetoed: bool,
    /// consensus multiplier applied (1.0 = none)
    pub consensus: f64,
    /// 10.5.1: nobody here carries weight, so this game got the even share ([`EVEN_SHARE`])
    pub even: bool,
}

/// 10.5.1: the base weight every spinnable game gets when NOBODY here carries any weight —
/// everyone is Flexible (`flexible.weight` is 0 by default), e.g. one flexible person alone.
/// Before, such a reel had zero total weight: an empty strip and "No game everyone here owns."
pub const EVEN_SHARE: f64 = 1.0;

/// §5.3: how ONE person's vote unit lands on the spinnable pool (`pool_ids`). A flexible person
/// spreads `flexible.weight` (default 0) instead. Credit multiplies slot-1's share only.
pub fn person_shares(prefs: &Prefs, pool_ids: &[String], credit: u32, settings: &Settings) -> Vec<(String, f64)> {
    if pool_ids.is_empty() {
        return Vec::new();
    }
    if prefs.flexible {
        if settings.flexible_weight <= 0.0 {
            return Vec::new();
        }
        let each = settings.flexible_weight / pool_ids.len() as f64;
        return pool_ids.iter().map(|g| (g.clone(), each)).collect();
    }
    let lp: Vec<&String> = pool_ids
        .iter()
        .filter(|g| prefs.would_play.contains(g) && !prefs.definitely_not.contains(g))
        .collect();
    if lp.is_empty() {
        return Vec::new();
    }
    // Favourites by slot position: slot 1 = favorite.share (× credit), slot 2 = second_share.
    let slots = settings.favorite_slots.clamp(1, 2) as usize;
    let mut favs: Vec<(&String, f64, bool)> = Vec::new();
    for (i, f) in prefs.favorites.iter().take(slots).enumerate() {
        if lp.contains(&f) && !favs.iter().any(|(x, _, _)| *x == f) {
            let base = if i == 0 { settings.favorite_share } else { settings.favorite_second_share };
            favs.push((f, base, i == 0));
        }
    }
    let others: Vec<&String> = lp.iter().filter(|g| !favs.iter().any(|(f, _, _)| *f == **g)).copied().collect();
    let mut base_total: f64 = favs.iter().map(|f| f.1).sum();
    if (others.is_empty() && base_total > 0.0) || base_total > 1.0 {
        // "If the favorite is their only pool game, the whole unit goes to it" — and two slots
        // configured past a whole unit are scaled back to one (§5.3).
        let scale = 1.0 / base_total;
        for f in &mut favs {
            f.1 *= scale;
        }
        base_total = 1.0;
    }
    let m = 1.0 + settings.regret_step * credit as f64;
    let mut out: Vec<(String, f64)> = Vec::new();
    for (id, base, primary) in favs {
        out.push((id.clone(), if primary { base * m } else { base }));
    }
    if !others.is_empty() {
        let each = (1.0 - base_total) / others.len() as f64;
        for g in others {
            out.push((g.clone(), each));
        }
    }
    out
}

/// §5.4: weight per spinnable game, in `pool.spinnable(settings)` order. `recent_winners` =
/// the winners of the last `damping.sessions` final spins (the controller supplies them;
/// ignored unless `damping.last_winner`).
pub fn weights(
    pool: &Pool,
    present: &[String],
    prefs: &HashMap<String, Prefs>,
    credits: &HashMap<String, u32>,
    recent_winners: &[String],
    settings: &Settings,
) -> Vec<GameWeight> {
    let ids = pool.spinnable(settings);
    weights_over(&ids, pool, present, prefs, credits, recent_winners, settings)
}

/// [`weights`] over an explicit id list (a respin excludes the vetoed games from the draw).
pub fn weights_over(
    ids: &[String],
    pool: &Pool,
    present: &[String],
    prefs: &HashMap<String, Prefs>,
    credits: &HashMap<String, u32>,
    recent_winners: &[String],
    settings: &Settings,
) -> Vec<GameWeight> {
    let mut acc: HashMap<String, GameWeight> = HashMap::new();
    for p in present {
        let Some(pr) = prefs.get(p) else { continue };
        let c = credits.get(p).copied().unwrap_or(0);
        for (g, s) in person_shares(pr, ids, c, settings) {
            let e = acc.entry(g.clone()).or_insert_with(|| GameWeight { id: g.clone(), consensus: 1.0, ..Default::default() });
            e.shares.push((p.clone(), s));
            e.weight += s;
        }
    }
    let n_present = present.len().max(1) as f64;
    // Nobody leaned toward anything (no POSITIVE share — a zero second-favourite share counts as
    // none): every game gets the same base share, then the usual multipliers (soft veto, damping,
    // consensus) — so a room of Flexible people can still spin.
    let even = acc.values().all(|g| !(g.weight > 0.0));
    ids.iter()
        .map(|id| {
            let mut gw = acc.remove(id).unwrap_or_else(|| GameWeight { id: id.clone(), consensus: 1.0, ..Default::default() });
            if even {
                gw.weight = EVEN_SHARE;
                gw.even = true;
            }
            if settings.veto_mode == VetoMode::Soft && pool.vetoers.get(id).map(|v| !v.is_empty()).unwrap_or(false) {
                gw.weight *= SOFT_VETO_FACTOR;
                gw.soft_vetoed = true;
            }
            if settings.damping_last_winner && recent_winners.iter().any(|w| w == id) {
                gw.weight *= settings.damping_factor;
                gw.damped = true;
            }
            if let Some(b) = settings.consensus_bonus {
                let owners = pool.owners.get(id).map(|o| o.len()).unwrap_or(0) as f64;
                gw.consensus = 1.0 + b * (owners / n_present);
                gw.weight *= gw.consensus;
            }
            gw
        })
        .collect()
}

/// Odds = w / Σw (all zero when nothing carries weight).
pub fn odds(ws: &[GameWeight]) -> Vec<f64> {
    let total: f64 = ws.iter().map(|w| w.weight).sum();
    if total <= 0.0 {
        return vec![0.0; ws.len()];
    }
    ws.iter().map(|w| w.weight / total).collect()
}

/// The precision weights are shown at (tiers, simulator). 10.5.0 also wrote spin records with
/// it — as floats, which Synapse refuses in room-v10 state; records now carry [`to_micro`].
pub fn round6(x: f64) -> f64 {
    (x * 1_000_000.0).round() / 1_000_000.0
}

/// 10.5.1 — a weight in integer micro-units (`round(w × 10⁶)`, the web's `toMicro`): the form a
/// spin record's pool is BUILT in before the first write, so the spinner derives the winner from
/// exactly the numbers every reader will read (readers are scale-free).
pub fn to_micro(w: f64) -> f64 {
    (w * 1_000_000.0).round()
}

/// An integral f64 as a JSON integer (Synapse's canonical JSON has no floats), else the f64.
fn weight_json(w: f64) -> Value {
    if w.is_finite() && w.fract() == 0.0 && w.abs() < 9_007_199_254_740_992.0 {
        json!(w as i64)
    } else {
        json!(w)
    }
}

// ---------------------------------------------------------------------------------------------
// Regret credit (§5.5) — derived from history, never stored
// ---------------------------------------------------------------------------------------------

/// §5.5, walked exactly in the design's order over the FINAL spin of each past session, newest
/// first (`spins` must already be finals only — non-voided, non-superseded — and must carry
/// `computed_winner`): absent → skip (or, with `regret.absence_decay`, one absence); favorite
/// not in that pool → skip; favorite differs from today's → stop (`reset_on_favorite_change`);
/// favorite won → stop; otherwise one lost night. Credit = min(nights, cap) + adjustment,
/// floored at 0. `regret.cap = 0` disables regret entirely.
pub fn derive_credit(user: &str, spins: &[SpinRecord], prefs_now: &Prefs, settings: &Settings, adjustment: Option<&Adjustment>) -> u32 {
    credit_detail(user, spins, prefs_now, settings, adjustment).credit
}

/// [`derive_credit`] plus whether the adjustment's offset still applies (10.5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreditDetail {
    pub credit: u32,
    /// the adjustment's offset (set + delta) still applies — false once the streak it was given in
    /// ended after it; true without an adjustment
    pub offset_live: bool,
}

/// §5.5 regret credit over the finals (newest first). 10.5.2: a final with `credit: false` is
/// skipped exactly like a voided one (no loss, no win, no absence — the night is invisible to
/// credit), and the adjustment's offset lapses when a streak end (win / favourite change / a night
/// without a favourite on record) newer than the adjustment stops the walk. See the block comment
/// at [`ADMIN_ADJUST_PREFIX`].
pub fn credit_detail(user: &str, spins: &[SpinRecord], prefs_now: &Prefs, settings: &Settings, adjustment: Option<&Adjustment>) -> CreditDetail {
    if settings.regret_cap == 0 {
        return CreditDetail { credit: 0, offset_live: true };
    }
    let Some(current_fav) = prefs_now.favorite() else { return CreditDetail { credit: 0, offset_live: true } };
    // A reset-all (`set`) draws a line: spins at or before `asOf` no longer count.
    let since = adjustment.and_then(|a| a.set.map(|_| a.as_of)).unwrap_or(0);
    #[derive(Clone, Copy)]
    enum Night {
        Loss,
        Absent,
    }
    let mut window: Vec<Night> = Vec::new(); // newest first
    // ts of the night that ended the streak (None = the walk ran out or hit the reset line)
    let mut stopped_at: Option<u64> = None;
    for s in spins {
        if s.voided.is_some() || !s.credit {
            continue;
        }
        if since > 0 && s.ts <= since {
            break;
        }
        if !s.roster.iter().any(|p| p == user) {
            if settings.regret_absence_decay.is_some() {
                window.push(Night::Absent);
            }
            continue; // absence does not break the streak
        }
        if s.flexible.iter().any(|p| p == user) {
            continue; // flexible people never gain credit (§3.4)
        }
        if !settings.regret_count_maybe && s.maybes.iter().any(|p| p == user) {
            continue;
        }
        let Some(fav) = s.favorites.get(user) else {
            // No favorite on record that night — nothing of theirs was up for selection.
            if settings.regret_reset_on_favorite_change {
                stopped_at = Some(s.ts);
                break;
            }
            continue;
        };
        if !s.in_pool(fav) {
            continue; // not up for selection
        }
        // a changed favourite resets the streak; a win ends it
        if (settings.regret_reset_on_favorite_change && fav.as_str() != current_fav) || s.effective_winner() == Some(fav.as_str()) {
            stopped_at = Some(s.ts);
            break;
        }
        window.push(Night::Loss);
    }
    // Replay oldest → newest so decay (one step per N absences) only eats nights that came
    // BEFORE the absences.
    let mut count: u32 = 0;
    let mut run: u32 = 0;
    for n in window.iter().rev() {
        match n {
            Night::Loss => count += 1,
            Night::Absent => {
                run += 1;
                if let Some(per) = settings.regret_absence_decay {
                    if run >= per.max(1) {
                        count = count.saturating_sub(1);
                        run = 0;
                    }
                }
            }
        }
    }
    let mut credit = count.min(settings.regret_cap) as i64;
    // the adjustment's time: when it was last written (server clock), else its asOf (tests / legacy)
    let adj_at = adjustment.map(|a| if a.written > 0 { a.written } else { a.as_of }).unwrap_or(0);
    let offset_live = adjustment.is_none() || stopped_at.map(|t| t <= adj_at).unwrap_or(true);
    if let Some(a) = adjustment.filter(|_| offset_live) {
        credit += a.set.unwrap_or(0) + a.delta;
    }
    // clamp before the cast (`as u32` would wrap) — the web saturates the same way
    CreditDetail { credit: credit.clamp(0, u32::MAX as i64) as u32, offset_live }
}

/// Tonight's credit per person on the roster (who has lists) — what the draw weighs a favourite up
/// by (games.rs `tonight`). `finals_past` = the finals of OTHER sessions, newest first. 10.5.2 free
/// spins (`credits.mode` free): 0 for everyone — banked credit is neither earned, spent nor used.
pub fn tonight_credits(roster: &[String], prefs: &HashMap<String, Prefs>, finals_past: &[SpinRecord], adjustments: &HashMap<String, Adjustment>, settings: &Settings) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    for p in roster {
        let Some(pr) = prefs.get(p) else { continue };
        let c = if settings.credits_mode == CreditsMode::Free { 0 } else { derive_credit(p, finals_past, pr, settings, adjustments.get(p)) };
        out.insert(p.clone(), c);
    }
    out
}

/// The winners of the last `damping.sessions` finals (newest first), for [`weights`].
pub fn recent_winners(finals_newest_first: &[SpinRecord], settings: &Settings) -> Vec<String> {
    finals_newest_first
        .iter()
        .filter(|s| s.voided.is_none())
        .filter_map(|s| s.effective_winner().map(|w| w.to_string()))
        .take(settings.damping_sessions.max(1) as usize)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Winner derivation (§7.2) — shared contract with the web client
// ---------------------------------------------------------------------------------------------

/// `r = first 8 bytes of sha256(seed_event_id)` read big-endian as u64, `/ 2^64` (u64 → f64 is
/// round-to-nearest-even in both Rust and JS `Number(BigInt)`); walk the pool IN RECORD ORDER
/// with cumulative weights, summing left to right, until `r × ΣW` is passed (strictly
/// greater). A zero-weight game can never win. None only for an empty/zero pool.
pub fn derive_winner(seed_event_id: &str, pool: &[(String, f64)]) -> Option<String> {
    let h = Sha256::digest(seed_event_id.as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&h[..8]);
    let r = u64::from_be_bytes(b) as f64 / 18_446_744_073_709_551_616.0;
    let mut total = 0.0f64;
    for (_, w) in pool {
        total += *w;
    }
    if !(total > 0.0) {
        return None;
    }
    let target = r * total;
    let mut cum = 0.0f64;
    for (id, w) in pool {
        cum += *w;
        if cum > target {
            return Some(id.clone());
        }
    }
    pool.iter().rev().find(|(_, w)| *w > 0.0).map(|(id, _)| id.clone())
}

// ---------------------------------------------------------------------------------------------
// Session resolution (§7.2 concurrency + veto chains)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolved {
    /// standing records in order: the initial spin, then each respin that replaced the last
    pub chain: Vec<String>,
    /// spin id → id of the record that beat it (two spins at once; earlier `ts` stands)
    pub superseded: HashMap<String, String>,
    /// the FINAL record: last of the chain that is not voided
    pub final_id: Option<String>,
}

/// Judge one session's records client-side (nothing is written): among records replacing the
/// same predecessor the earliest `origin_server_ts` stands (ties: the smaller event id), the
/// rest are superseded; a respin of a superseded record is superseded too. Final = the last
/// standing, non-voided record.
pub fn resolve_session(records: &[SpinRecord]) -> Resolved {
    let mut by_parent: BTreeMap<Option<String>, Vec<&SpinRecord>> = BTreeMap::new();
    for r in records {
        by_parent.entry(r.supersedes.clone()).or_default().push(r);
    }
    for v in by_parent.values_mut() {
        v.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.event_id.cmp(&b.event_id)));
    }
    let mut out = Resolved::default();
    let mut parent: Option<String> = None;
    let mut guard = 0;
    while let Some(group) = by_parent.get(&parent) {
        let Some(stands) = group.first() else { break };
        for other in group.iter().skip(1) {
            out.superseded.insert(other.id.clone(), stands.id.clone());
        }
        out.chain.push(stands.id.clone());
        parent = Some(stands.id.clone());
        guard += 1;
        if guard > 64 {
            break; // a cycle in `supersedes` — never legitimately reachable
        }
    }
    for r in records {
        if !out.chain.contains(&r.id) && !out.superseded.contains_key(&r.id) {
            // Its parent lost a race (or never existed): orphaned, treat as superseded.
            let by = r.supersedes.as_ref().and_then(|p| out.superseded.get(p)).cloned().unwrap_or_default();
            out.superseded.insert(r.id.clone(), by);
        }
    }
    out.final_id = out
        .chain
        .iter()
        .rev()
        .find(|id| records.iter().any(|r| &r.id == *id && r.voided.is_none()))
        .cloned();
    out
}

/// Re-spins used tonight = the chain's `kind == "respin"` records, voided or not (the line
/// games.rs `compute` has used since 9.0; the web's `respinsUsedIn`).
pub fn respins_used_in<R: std::borrow::Borrow<SpinRecord>>(chain: &[String], records: &[R]) -> u32 {
    chain.iter().filter(|id| records.iter().any(|r| r.borrow().id == **id && r.borrow().kind == "respin")).count() as u32
}

// ---------------------------------------------------------------------------------------------
// Admin "Reset spins" (10.5.2 — the web's model/games.ts `resetWrites`, same writes in the same
// order; 2026-09-23: an admin hit the re-spin limit while testing and had no way back)
// ---------------------------------------------------------------------------------------------

/// The kind an admin reset gives a re-spin record: anything but "respin" stops counting everywhere.
pub const RESET_KIND: &str = "reset";
pub const RESET_REASON: &str = "spins reset by admin";

/// A stored spin record as the reset sees it: state key, current event id, raw content.
#[derive(Clone, Debug, PartialEq)]
pub struct RawSpin {
    pub id: String,
    pub event_id: String,
    pub content: Value,
}

/// The rewrites that restart tonight, in order. `raw` = every stored record of tonight's session,
/// `chain` = its resolved chain. Two passes:
///   1. `voided` on every record that lacks one — records outside the chain (race losers /
///      orphans) first, then the chain root → tail. The re-spin count does not move while a result
///      still stands, so nobody gets a re-spin back early, and a watcher keeps seeing tonight's
///      latest result until the standing record's own write lands (voiding the tail first would
///      bring an earlier result back). The last write of this pass also re-kinds its record: from
///      then on nothing stands.
///   2. `kind: "respin"` → [`RESET_KIND`] on every remaining re-spin, so they stop counting.
/// Nothing else changes (pool / roster / session / winner / seed stay, so each record still
/// verifies against its seed; `seed` is filled from the current event id when the spinner never
/// wrote it, like a void). Losers are voided too because a rewrite gives a record a new server
/// timestamp, and a reader that could not fetch its seed orders race siblings by that timestamp —
/// an untouched loser could otherwise become the chain's root and stand again.
///
/// Why not just void the standing record (Tune's "Void tonight's spin"): it then falls back to
/// its predecessor, and every client — desktop 10.5.0 / 10.5.1 included — keeps counting the
/// chain's "respin" records, so even a fresh spin would land with 0 re-spins left. Readers need no
/// change for this: after the rewrites nothing stands (Spin is enabled), and the next spin —
/// chained onto the tail by `do_spin` (10.5.1+) and the web — counts no re-spins and has no
/// vetoes. A re-run after a partial failure only writes what is left. (`resolve_session` stops
/// walking a chain at 65 records, so a single night survives ~20 reset + spin + 2 re-spin rounds;
/// the ad-hoc session rolls over at the fixed day boundary, 14:00 UTC.)
pub fn reset_writes(chain: &[String], raw: &[RawSpin], me: &str, now: u64) -> Vec<(String, Value)> {
    let live: Vec<&RawSpin> = raw.iter().filter(|r| r.content.as_object().map(|o| !o.is_empty()).unwrap_or(false)).collect();
    let mut order: Vec<&RawSpin> = live.iter().copied().filter(|r| !chain.contains(&r.id)).collect();
    order.extend(chain.iter().filter_map(|id| live.iter().copied().find(|r| &r.id == id)));
    let mut cur: HashMap<String, serde_json::Map<String, Value>> = HashMap::new();
    for r in &order {
        cur.insert(r.id.clone(), r.content.as_object().cloned().unwrap_or_default());
    }
    let mut out: Vec<(String, Value)> = Vec::new();
    let mut put = |cur: &mut HashMap<String, serde_json::Map<String, Value>>, r: &RawSpin, patch: Vec<(&str, Value)>| {
        let mut next = cur.get(&r.id).cloned().unwrap_or_default();
        for (k, v) in patch {
            next.insert(k.to_string(), v);
        }
        if next.get("seed").and_then(Value::as_str).map(str::is_empty).unwrap_or(true) {
            next.insert("seed".into(), json!(r.event_id));
        }
        cur.insert(r.id.clone(), next.clone());
        out.push((r.id.clone(), Value::Object(next)));
    };
    let is_respin = |cur: &HashMap<String, serde_json::Map<String, Value>>, id: &str| cur.get(id).and_then(|c| c.get("kind")).and_then(Value::as_str) == Some("respin");
    let to_void: Vec<&RawSpin> = order.iter().copied().filter(|r| !cur.get(&r.id).and_then(|c| c.get("voided")).map(Value::is_object).unwrap_or(false)).collect();
    for (i, r) in to_void.iter().enumerate() {
        let mut patch = vec![("voided", json!({ "by": me, "reason": RESET_REASON, "at": now }))];
        if i + 1 == to_void.len() && is_respin(&cur, &r.id) {
            patch.push(("kind", json!(RESET_KIND)));
        }
        put(&mut cur, r, patch);
    }
    for r in &order {
        if is_respin(&cur, &r.id) {
            put(&mut cur, r, vec![("kind", json!(RESET_KIND))]);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Explainability (§9)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    /// `favorite` | `credit` | `lastWinner` | `onlyPick`
    pub kind: &'static str,
    pub text: String,
}

pub struct ExplainCtx<'a> {
    pub pool: &'a Pool,
    /// the actual roster
    pub present: &'a [String],
    pub prefs: &'a HashMap<String, Prefs>,
    pub credits: &'a HashMap<String, u32>,
    pub names: &'a HashMap<String, String>,
    pub settings: &'a Settings,
    /// what the reel draws from tonight (after vetoes)
    pub spinnable: &'a [String],
}

fn lost_nights(c: u32) -> String {
    if c == 1 {
        "Lost last time".to_string()
    } else {
        format!("Lost the last {c} nights")
    }
}

/// The §9 one-liners + strip badges for one game. Every number on the strip gets a reason.
pub fn explain(ctx: &ExplainCtx, id: &str, w: Option<&GameWeight>) -> (Vec<String>, Vec<Badge>) {
    let mut lines: Vec<String> = Vec::new();
    let mut badges: Vec<Badge> = Vec::new();
    let name = |p: &String| display_name(ctx.names, p);
    let tier = ctx.pool.tier(id);
    if tier == Some(Tier::Removed) {
        if let Some((_, reason)) = ctx.pool.removed.iter().find(|(g, _)| g == id) {
            lines.push(reason.clone());
        }
        return (lines, badges);
    }
    let owners = ctx.pool.owners.get(id).cloned().unwrap_or_default();
    lines.push(format!("{} of you would play this.", owners.len()));

    let non_flex = |p: &&String| ctx.prefs.get(*p).map(|x| !x.flexible).unwrap_or(false);
    // Slot-1 favourites among the present, with their credit.
    let favers: Vec<(String, u32)> = ctx
        .present
        .iter()
        .filter(non_flex)
        .filter(|p| ctx.prefs.get(*p).and_then(|x| x.favorite()) == Some(id))
        .map(|p| (p.clone(), ctx.credits.get(p).copied().unwrap_or(0)))
        .collect();
    match favers.len() {
        0 => {}
        1 => {
            let (p, c) = &favers[0];
            if *c > 0 {
                lines.push(format!("{}'s favorite. {}, so it's weighted up.", name(p), lost_nights(*c)));
            } else {
                lines.push(format!("{}'s favorite.", name(p)));
            }
            badges.push(Badge { kind: "favorite", text: format!("★ {}", name(p)) });
        }
        _ => {
            let ns: Vec<String> = favers.iter().map(|(p, _)| name(p)).collect();
            lines.push(format!("Favorite of {}.", join_names(&ns)));
            for (p, c) in &favers {
                if *c > 0 {
                    lines.push(format!("{} {}, so it's weighted up.", name(p), lost_nights(*c).to_lowercase()));
                }
            }
            badges.push(Badge { kind: "favorite", text: format!("★ favorite of {}", favers.len()) });
        }
    }
    for (p, c) in &favers {
        if *c > 0 {
            badges.push(Badge { kind: "credit", text: if favers.len() > 1 { format!("+{c} {}", name(p)) } else { format!("+{c}") } });
        }
    }
    if ctx.settings.favorite_slots >= 2 {
        for p in ctx.present.iter().filter(non_flex) {
            if ctx.prefs.get(p).and_then(|x| x.favorites.get(1)).map(String::as_str) == Some(id) {
                lines.push(format!("{}'s second favorite.", name(p)));
            }
        }
    }
    // Single-game list: their whole unit rides on this one (§5.3 "Max only listed this one").
    for p in ctx.present.iter().filter(non_flex) {
        let Some(x) = ctx.prefs.get(p) else { continue };
        let lp: Vec<&String> = ctx.spinnable.iter().filter(|g| x.would_play.contains(g) && !x.definitely_not.contains(g)).collect();
        if lp.len() == 1 && lp[0] == id {
            lines.push(format!("{} only listed this one.", name(p)));
            badges.push(Badge { kind: "onlyPick", text: format!("{}'s only pick", name(p)) });
        }
    }
    if tier == Some(Tier::Stretch) {
        let missing: Vec<String> = ctx.pool.missing.get(id).map(|m| m.iter().map(name).collect()).unwrap_or_default();
        if missing.len() == 1 {
            lines.push(format!("{} doesn't own it.", missing[0]));
        } else if !missing.is_empty() {
            lines.push(format!("{} don't own it.", join_names(&missing)));
        }
    }
    if let Some(v) = ctx.pool.vetoers.get(id).filter(|v| !v.is_empty()) {
        let ns: Vec<String> = v.iter().map(name).collect();
        match ctx.settings.veto_mode {
            VetoMode::Soft => lines.push(format!("{}: definitely not, so it's weighted down.", join_names(&ns))),
            VetoMode::Off => lines.push(format!("{} would rather not (vetoes are off tonight).", join_names(&ns))),
            VetoMode::Hard => {}
        }
    }
    if let Some(gw) = w {
        if gw.even {
            lines.push("Nobody here leans toward a game, so every game gets an even share.".to_string());
        }
        if gw.damped {
            lines.push("Won last time, so it's weighted down.".to_string());
            badges.push(Badge { kind: "lastWinner", text: "won last time".to_string() });
        }
        if gw.consensus > 1.0 {
            lines.push(format!("Listed by {} of {}, so it gets a consensus bonus.", owners.len(), ctx.present.len()));
        }
    }
    (lines, badges)
}

/// §9 "Superseded spin" line.
pub fn explain_superseded(winner_by_name: &str) -> String {
    format!("Two people spun at once. {winner_by_name}'s spin came first.")
}

/// §9 "Voided" line.
pub fn explain_voided(by_name: &str, reason: &str) -> String {
    if reason.trim().is_empty() {
        format!("Voided by {by_name}.")
    } else {
        format!("Voided by {by_name}: {}.", reason.trim().trim_end_matches('.'))
    }
}

// ---------------------------------------------------------------------------------------------
// Tests — the §12 gates. The winner vectors are the contract the web client must reproduce.
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const KAI: &str = "@kai:example.com";
    const SAM: &str = "@sam:example.com";
    const RILEY: &str = "@riley:example.com";
    const JO: &str = "@jo:example.com";
    const MAX: &str = "@max:example.com";
    const VALHEIM: &str = "steam:892970";
    const ZOMBOID: &str = "steam:108600";
    const PALWORLD: &str = "steam:1623730";
    const PHASMO: &str = "steam:739630";

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn prefs(would: &[&str], fav: Option<&str>, not: &[&str], flexible: bool) -> Prefs {
        Prefs { would_play: s(would), favorites: fav.map(|f| vec![f.to_string()]).unwrap_or_default(), definitely_not: s(not), flexible, updated_at: 0, names: Default::default() }
    }

    fn names() -> HashMap<String, String> {
        [(KAI, "Kai"), (SAM, "Sam"), (RILEY, "Riley"), (JO, "Jo"), (MAX, "Max")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// The §5.5 worked example's people.
    fn worked_prefs() -> HashMap<String, Prefs> {
        let mut m = HashMap::new();
        m.insert(KAI.to_string(), prefs(&[VALHEIM, ZOMBOID, PALWORLD], Some(VALHEIM), &[], false));
        m.insert(SAM.to_string(), prefs(&[ZOMBOID, PHASMO], Some(PHASMO), &[], false));
        m.insert(RILEY.to_string(), prefs(&[], None, &[], true));
        m.insert(JO.to_string(), prefs(&[VALHEIM, ZOMBOID, PALWORLD, PHASMO], None, &[], false));
        m.insert(MAX.to_string(), prefs(&[PALWORLD], Some(PALWORLD), &[], false));
        m
    }

    fn worked_pool() -> Pool {
        let mut p = Pool { main: s(&[PALWORLD, VALHEIM, ZOMBOID, PHASMO]), ..Default::default() };
        let pr = worked_prefs();
        for g in p.main.clone() {
            let owners: Vec<String> = [KAI, SAM, RILEY, JO, MAX].iter().filter(|u| pr[**u].would_play.iter().any(|w| w == &g)).map(|u| u.to_string()).collect();
            p.owners.insert(g, owners);
        }
        p
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn worked_example_weights_and_odds() {
        let st = Settings::default();
        let present = s(&[KAI, SAM, RILEY, JO, MAX]);
        let credits: HashMap<String, u32> = [(KAI.to_string(), 2u32)].into_iter().collect();
        let ws = weights(&worked_pool(), &present, &worked_prefs(), &credits, &[], &st);
        let w = |id: &str| ws.iter().find(|g| g.id == id).map(|g| g.weight).unwrap();
        assert!(approx(w(PALWORLD), 1.50), "Palworld {}", w(PALWORLD));
        assert!(approx(w(VALHEIM), 1.25), "Valheim {}", w(VALHEIM));
        assert!(approx(w(ZOMBOID), 1.00), "Zomboid {}", w(ZOMBOID));
        assert!(approx(w(PHASMO), 0.75), "Phasmophobia {}", w(PHASMO));
        let o = odds(&ws);
        let pct: Vec<u32> = o.iter().map(|x| (x * 100.0).round() as u32).collect();
        assert_eq!(pct, vec![33, 28, 22, 17]);
        assert!(approx(o.iter().sum::<f64>(), 1.0), "odds sum to 100%");
        // Every person's total = one unit + credit extra (favorite base share × step × credit);
        // flexible Riley contributes nothing.
        let unit_of = |p: &str| -> f64 { ws.iter().flat_map(|g| g.shares.iter()).filter(|(u, _)| u == p).map(|(_, x)| x).sum() };
        assert!(approx(unit_of(KAI), 1.0 + 0.5 * 0.5 * 2.0));
        assert!(approx(unit_of(SAM), 1.0));
        assert!(approx(unit_of(JO), 1.0));
        assert!(approx(unit_of(MAX), 1.0));
        assert!(approx(unit_of(RILEY), 0.0));
    }

    #[test]
    fn only_pool_game_carries_the_whole_unit_times_credit() {
        let st = Settings::default();
        let ps = person_shares(&prefs(&[PALWORLD], Some(PALWORLD), &[], false), &s(&[PALWORLD, VALHEIM]), 1, &st);
        assert_eq!(ps.len(), 1);
        assert!(approx(ps[0].1, 1.5)); // 1 unit × (1 + 0.5 × 1)
    }

    #[test]
    fn flexible_person_never_blocks_a_game() {
        // §3.4: Riley says "I'll play whatever" and lists nothing. Four people all list Valheim.
        // Valheim must stay in the MAIN tier — before this, Riley counted as a non-owner and the
        // main tier came back empty, so Spin was disabled for the whole night.
        let st = Settings::default();
        let mut pr = HashMap::new();
        for p in [KAI, SAM, JO, MAX] {
            pr.insert(p.to_string(), prefs(&[VALHEIM], None, &[], false));
        }
        pr.insert(RILEY.to_string(), prefs(&[], None, &[], true));
        let present = s(&[KAI, SAM, JO, MAX, RILEY]);
        let pool = build_pool(&present, &pr, &st, &names());
        assert_eq!(pool.main, s(&[VALHEIM]), "a flexible person must not empty the main tier");
        assert!(pool.stretch.is_empty());
        assert!(pool.missing.get(VALHEIM).map(|m| m.is_empty()).unwrap_or(true), "Riley is not 'missing' Valheim");
        // ...and they still are not listed as an owner of a game they never listed.
        assert_eq!(pool.owners[VALHEIM], s(&[KAI, SAM, JO, MAX]));
    }

    #[test]
    fn pool_tiers_and_reasons() {
        let st = Settings::default();
        let mut pr = HashMap::new();
        pr.insert(KAI.to_string(), prefs(&[VALHEIM, ZOMBOID, PALWORLD], Some(VALHEIM), &[], false));
        pr.insert(SAM.to_string(), prefs(&[VALHEIM, ZOMBOID], None, &[PALWORLD], false));
        pr.insert(JO.to_string(), prefs(&[VALHEIM, PHASMO], None, &[], false));
        let present = s(&[KAI, SAM, JO]);
        let pool = build_pool(&present, &pr, &st, &names());
        assert_eq!(pool.main, s(&[VALHEIM]));
        assert_eq!(pool.stretch, s(&[ZOMBOID]));
        let removed: HashMap<String, String> = pool.removed.iter().cloned().collect();
        assert_eq!(removed[PALWORLD], "Sam: definitely not.");
        assert_eq!(removed[PHASMO], "Not enough owners.");
        assert_eq!(pool.missing[ZOMBOID], s(&[JO]));
        // Soft veto keeps the game in its tier at ×0.1 (Jo listing Palworld puts it in stretch).
        let mut pr_soft = pr.clone();
        pr_soft.get_mut(JO).unwrap().would_play = s(&[VALHEIM, PHASMO, PALWORLD]);
        let soft = Settings { veto_mode: VetoMode::Soft, pool_spin_stretch: true, ..Settings::default() };
        let pool2 = build_pool(&present, &pr_soft, &soft, &names());
        assert_eq!(pool2.main, s(&[VALHEIM]));
        assert_eq!(pool2.stretch, s(&[ZOMBOID, PALWORLD]));
        let ws = weights(&pool2, &present, &pr_soft, &HashMap::new(), &[], &soft);
        let pal = ws.iter().find(|g| g.id == PALWORLD).unwrap();
        assert!(pal.soft_vetoed);
        assert!(approx(pal.weight, 0.075)); // Kai 0.25 + Jo 0.5 (Sam refuses it), × 0.1
        // Flexible people's lists count unless told otherwise.
        let mut pr2 = pr.clone();
        pr2.insert(RILEY.to_string(), prefs(&[VALHEIM], None, &[], true));
        let p4 = build_pool(&s(&[KAI, SAM, JO, RILEY]), &pr2, &st, &names());
        assert_eq!(p4.main, s(&[VALHEIM]));
        assert_eq!(p4.counted, 4);
        let no_flex = Settings { pool_count_flexible_ownership: false, ..Settings::default() };
        let p5 = build_pool(&s(&[KAI, SAM, JO, RILEY]), &pr2, &no_flex, &names());
        assert_eq!(p5.counted, 3);
        assert_eq!(p5.main, s(&[VALHEIM]));
        // min_owners ≥2 opens the main tier up.
        let two = Settings { pool_min_owners: OwnerRule::AtLeast(2), ..Settings::default() };
        let p6 = build_pool(&present, &pr, &two, &names());
        assert_eq!(p6.main, s(&[ZOMBOID, VALHEIM]));
    }

    #[test]
    fn damping_and_consensus() {
        let st = Settings { damping_last_winner: true, damping_factor: 0.5, ..Settings::default() };
        let present = s(&[KAI, SAM, RILEY, JO, MAX]);
        let ws = weights(&worked_pool(), &present, &worked_prefs(), &HashMap::new(), &s(&[PALWORLD]), &st);
        let pal = ws.iter().find(|g| g.id == PALWORLD).unwrap();
        assert!(pal.damped);
        assert!(approx(pal.weight, 0.5 * (0.25 + 0.25 + 1.0)));
        let st2 = Settings { consensus_bonus: Some(1.0), ..Settings::default() };
        let ws2 = weights(&worked_pool(), &present, &worked_prefs(), &HashMap::new(), &[], &st2);
        let zom = ws2.iter().find(|g| g.id == ZOMBOID).unwrap();
        assert!(approx(zom.consensus, 1.0 + 3.0 / 5.0)); // Kai, Sam, Jo list it out of 5 present
    }

    // ---- credit ----

    fn spin(session: &str, ts: u64, roster: &[&str], favs: &[(&str, &str)], pool: &[&str], winner: &str) -> SpinRecord {
        SpinRecord {
            id: format!("sp-{ts}-0000"),
            event_id: format!("$ev{ts}"),
            ts,
            session_id: session.to_string(),
            kind: "initial".into(),
            roster: s(roster),
            favorites: favs.iter().map(|(u, g)| (u.to_string(), g.to_string())).collect(),
            pool: pool.iter().map(|g| (g.to_string(), 1.0)).collect(),
            winner: Some(winner.to_string()),
            computed_winner: Some(winner.to_string()),
            verified: true,
            ..Default::default()
        }
    }

    fn kai_now() -> Prefs {
        prefs(&[VALHEIM, ZOMBOID], Some(VALHEIM), &[], false)
    }

    #[test]
    fn credit_streak_and_cap() {
        let st = Settings::default();
        let mut finals = vec![];
        for i in 0..5u64 {
            finals.push(spin(&format!("s{i}"), 1000 - i, &[KAI, SAM], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID));
        }
        assert_eq!(derive_credit(KAI, &finals[..2], &kai_now(), &st, None), 2);
        assert_eq!(derive_credit(KAI, &finals[..3], &kai_now(), &st, None), 3);
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &st, None), 3, "capped at 3");
        let cap5 = Settings { regret_cap: 5, ..Settings::default() };
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &cap5, None), 5);
        let off = Settings { regret_cap: 0, ..Settings::default() };
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &off, None), 0, "cap 0 disables regret");
        // Sam has no favorite → nothing to credit; someone not in the story → 0.
        assert_eq!(derive_credit(SAM, &finals, &prefs(&[ZOMBOID], None, &[], false), &st, None), 0);
        assert_eq!(derive_credit(JO, &finals, &prefs(&[VALHEIM], Some(VALHEIM), &[], false), &st, None), 0);
    }

    #[test]
    fn credit_resets_on_win() {
        let st = Settings::default();
        let finals = vec![
            spin("s4", 1004, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
            spin("s3", 1003, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
            spin("s2", 1002, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], VALHEIM),
            spin("s1", 1001, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
        ];
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &st, None), 2);
        // The night just won: the streak is over.
        assert_eq!(derive_credit(KAI, &finals[2..], &kai_now(), &st, None), 0);
        // A voided final never counts.
        let mut v = finals.clone();
        v[0].voided = Some(Voided { by: JO.into(), reason: "test".into(), at: 1 });
        assert_eq!(derive_credit(KAI, &v, &kai_now(), &st, None), 1);
    }

    #[test]
    fn credit_resets_on_favorite_change() {
        let st = Settings::default();
        let finals = vec![
            spin("s3", 1003, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
            spin("s2", 1002, &[KAI], &[(KAI, ZOMBOID)], &[VALHEIM, ZOMBOID], VALHEIM),
            spin("s1", 1001, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
        ];
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &st, None), 1);
        let keep = Settings { regret_reset_on_favorite_change: false, ..Settings::default() };
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &keep, None), 3, "with reset off the old favorite's losses still count");
    }

    #[test]
    fn credit_skips_absence_and_out_of_pool() {
        let st = Settings::default();
        let finals = vec![
            spin("s4", 1004, &[SAM], &[(SAM, PHASMO)], &[PHASMO, ZOMBOID], PHASMO), // Kai absent
            spin("s3", 1003, &[KAI], &[(KAI, VALHEIM)], &[ZOMBOID, PALWORLD], ZOMBOID), // favorite not up
            spin("s2", 1002, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
            spin("s1", 1001, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
        ];
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &st, None), 2);
        // absence_decay = 1 per absence: the one missed night after two losses eats one.
        let decay = Settings { regret_absence_decay: Some(1), ..Settings::default() };
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &decay, None), 1);
        // Flexible that night → no credit for it; a "maybe" who showed → configurable.
        let mut f2 = finals.clone();
        f2[2].flexible = s(&[KAI]);
        assert_eq!(derive_credit(KAI, &f2, &kai_now(), &st, None), 1);
        let mut f3 = finals.clone();
        f3[2].maybes = s(&[KAI]);
        assert_eq!(derive_credit(KAI, &f3, &kai_now(), &st, None), 2);
        let no_maybe = Settings { regret_count_maybe: false, ..Settings::default() };
        assert_eq!(derive_credit(KAI, &f3, &kai_now(), &no_maybe, None), 1);
    }

    #[test]
    fn credit_adjustments() {
        let st = Settings::default();
        let finals = vec![
            spin("s2", 1002, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
            spin("s1", 1001, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
        ];
        let plus = Adjustment { delta: 1, set: None, as_of: 0, reason: "".into(), written: 0 };
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &st, Some(&plus)), 3);
        let minus = Adjustment { delta: -5, set: None, as_of: 0, reason: "".into(), written: 0 };
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &st, Some(&minus)), 0, "floored at 0");
        // Reset-all "as of" after both nights → 0; between them → only the newer counts.
        let reset_now = Adjustment { delta: 0, set: Some(0), as_of: 2000, reason: "".into(), written: 0 };
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &st, Some(&reset_now)), 0);
        let reset_mid = Adjustment { delta: 0, set: Some(0), as_of: 1001, reason: "".into(), written: 0 };
        assert_eq!(derive_credit(KAI, &finals, &kai_now(), &st, Some(&reset_mid)), 1);
        // Adjustment sits OUTSIDE the cap.
        let big = Adjustment { delta: 2, set: None, as_of: 0, reason: "".into(), written: 0 };
        let five: Vec<SpinRecord> = (0..5u64).map(|i| spin(&format!("x{i}"), 900 - i, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID)).collect();
        assert_eq!(derive_credit(KAI, &five, &kai_now(), &st, Some(&big)), 5);
    }

    // ---- winner derivation: shared vectors (sha256, big-endian u64 / 2^64, strict >) ----

    fn worked_record_pool() -> Vec<(String, f64)> {
        vec![(PALWORLD.into(), 1.5), (VALHEIM.into(), 1.25), (ZOMBOID.into(), 1.0), (PHASMO.into(), 0.75)]
    }

    #[test]
    fn winner_vectors() {
        // r values: v3 = 0.04291298494925477, v5 = 0.7902206343106676, v23 = 0.5504738895377616
        let pool = worked_record_pool();
        assert_eq!(derive_winner("$spin-vector-3:example.com", &pool).as_deref(), Some(PALWORLD));
        assert_eq!(derive_winner("$spin-vector-5:example.com", &pool).as_deref(), Some(ZOMBOID));
        assert_eq!(derive_winner("$spin-vector-23:example.com", &pool).as_deref(), Some(VALHEIM));
        let flat: Vec<(String, f64)> = vec![("a".into(), 1.0), ("b".into(), 1.0), ("c".into(), 1.0)];
        assert_eq!(derive_winner("$spin-vector-3:example.com", &flat).as_deref(), Some("a"));
        assert_eq!(derive_winner("$spin-vector-5:example.com", &flat).as_deref(), Some("c"));
        assert_eq!(derive_winner("$spin-vector-23:example.com", &flat).as_deref(), Some("b"));
        // A zero-weight game can never win; an empty pool has no winner.
        let zero: Vec<(String, f64)> = vec![("a".into(), 0.0), ("b".into(), 1.0)];
        assert_eq!(derive_winner("$spin-vector-3:example.com", &zero).as_deref(), Some("b"));
        assert_eq!(derive_winner("$spin-vector-3:example.com", &[]), None);
        // Same seed, same pool, same answer — every client lands on the same cover.
        for _ in 0..3 {
            assert_eq!(derive_winner("$spin-vector-5:example.com", &pool).as_deref(), Some(ZOMBOID));
        }
    }

    // ---- concurrency: earlier origin_server_ts stands ----

    #[test]
    fn two_spins_at_once_earlier_stands() {
        let mut a = spin("s1", 5000, &[KAI, SAM], &[], &[VALHEIM, ZOMBOID], VALHEIM);
        a.id = "sp-a".into();
        a.sender = KAI.into();
        let mut b = spin("s1", 5200, &[KAI, SAM], &[], &[VALHEIM, ZOMBOID], ZOMBOID);
        b.id = "sp-b".into();
        b.sender = SAM.into();
        // Order of arrival must not matter.
        for recs in [vec![a.clone(), b.clone()], vec![b.clone(), a.clone()]] {
            let r = resolve_session(&recs);
            assert_eq!(r.chain, vec!["sp-a".to_string()]);
            assert_eq!(r.superseded.get("sp-b").map(String::as_str), Some("sp-a"));
            assert_eq!(r.final_id.as_deref(), Some("sp-a"));
        }
        // A respin of the loser is orphaned; a respin of the winner extends the chain and
        // becomes the final; voiding the final falls back to its predecessor.
        let mut c = spin("s1", 6000, &[KAI, SAM], &[], &[ZOMBOID], ZOMBOID);
        c.id = "sp-c".into();
        c.kind = "respin".into();
        c.supersedes = Some("sp-b".into());
        let mut d = spin("s1", 6100, &[KAI, SAM], &[], &[ZOMBOID], ZOMBOID);
        d.id = "sp-d".into();
        d.kind = "respin".into();
        d.supersedes = Some("sp-a".into());
        let r = resolve_session(&[a.clone(), b.clone(), c.clone(), d.clone()]);
        assert_eq!(r.chain, vec!["sp-a".to_string(), "sp-d".to_string()]);
        assert!(r.superseded.contains_key("sp-c"));
        assert_eq!(r.final_id.as_deref(), Some("sp-d"));
        let mut dv = d.clone();
        dv.voided = Some(Voided { by: JO.into(), reason: "oops".into(), at: 1 });
        let r2 = resolve_session(&[a.clone(), b, c, dv]);
        assert_eq!(r2.final_id.as_deref(), Some("sp-a"));
        // 10.5.1: an admin voids the only spin, then someone spins again. A new initial with no
        // `supersedes` is a sibling of the voided record and loses to it (nothing stands); chained
        // onto the tail — what do_spin now writes — it stands and costs no re-spin.
        let mut av = a;
        av.voided = Some(Voided { by: JO.into(), reason: "double spin".into(), at: 1 });
        let mut e = spin("s1", 7000, &[KAI, SAM], &[], &[VALHEIM, ZOMBOID], ZOMBOID);
        e.id = "sp-e".into();
        assert_eq!(resolve_session(&[av.clone(), e.clone()]).final_id, None, "the 10.5.0 write stays stuck");
        e.supersedes = Some("sp-a".into());
        let r3 = resolve_session(&[av, e]);
        assert_eq!(r3.chain, vec!["sp-a".to_string(), "sp-e".to_string()]);
        assert_eq!(r3.final_id.as_deref(), Some("sp-e"));
    }

    // ---- explain strings (§9) ----

    #[test]
    fn explain_strings() {
        let st = Settings::default();
        let present = s(&[KAI, SAM, RILEY, JO, MAX]);
        let pr = worked_prefs();
        let pool = worked_pool();
        let credits: HashMap<String, u32> = [(KAI.to_string(), 2u32)].into_iter().collect();
        let spinnable = pool.spinnable(&st);
        let ctx = ExplainCtx { pool: &pool, present: &present, prefs: &pr, credits: &credits, names: &names(), settings: &st, spinnable: &spinnable };
        let (val, vb) = explain(&ctx, VALHEIM, None);
        assert_eq!(val[0], "2 of you would play this.");
        assert_eq!(val[1], "Kai's favorite. Lost the last 2 nights, so it's weighted up.");
        assert!(vb.iter().any(|b| b.kind == "favorite" && b.text == "★ Kai"));
        assert!(vb.iter().any(|b| b.kind == "credit" && b.text == "+2"));
        let (pal, pb) = explain(&ctx, PALWORLD, None);
        assert!(pal.contains(&"Max's favorite.".to_string()));
        assert!(pal.contains(&"Max only listed this one.".to_string()));
        assert!(pb.iter().any(|b| b.kind == "onlyPick"));
        let (ph, _) = explain(&ctx, PHASMO, None);
        assert_eq!(ph[1], "Sam's favorite.");
        // Two favourites of one game.
        let mut pr2 = pr.clone();
        pr2.get_mut(JO).unwrap().favorites = vec![VALHEIM.into()];
        let ctx2 = ExplainCtx { prefs: &pr2, ..ctx };
        let (v2, b2) = explain(&ctx2, VALHEIM, None);
        assert_eq!(v2[1], "Favorite of Kai and Jo.");
        assert_eq!(v2[2], "Kai lost the last 2 nights, so it's weighted up.");
        assert!(b2.iter().any(|b| b.text == "★ favorite of 2"));
        // Stretch + removed reasons.
        let mut pool3 = pool.clone();
        pool3.main.retain(|g| g != VALHEIM);
        pool3.stretch.push(VALHEIM.into());
        pool3.missing.insert(VALHEIM.into(), s(&[SAM, JO]));
        pool3.removed.push((PHASMO.into(), "Jo: definitely not.".into()));
        pool3.main.retain(|g| g != PHASMO);
        let ctx3 = ExplainCtx { pool: &pool3, ..ctx };
        let (v3, _) = explain(&ctx3, VALHEIM, None);
        assert!(v3.contains(&"Sam and Jo don't own it.".to_string()));
        let (r3, _) = explain(&ctx3, PHASMO, None);
        assert_eq!(r3, vec!["Jo: definitely not.".to_string()]);
        let damped = GameWeight { id: ZOMBOID.into(), weight: 0.5, damped: true, consensus: 1.0, ..Default::default() };
        let (z, zb) = explain(&ctx, ZOMBOID, Some(&damped));
        assert!(z.contains(&"Won last time, so it's weighted down.".to_string()));
        assert!(zb.iter().any(|b| b.kind == "lastWinner"));
        assert_eq!(explain_superseded("Kai"), "Two people spun at once. Kai's spin came first.");
        assert_eq!(explain_voided("Blake", "double spin"), "Voided by Blake: double spin.");
        assert_eq!(join_names(&s(&["Kai", "Sam", "Jo"])), "Kai, Sam and Jo");
    }

    // ---- settings + records round-trip ----

    #[test]
    fn settings_round_trip_and_clamps() {
        let d = Settings::default();
        assert_eq!(Settings::from_json(&d.to_json()), d);
        assert_eq!(Settings::from_json(&json!({})), d);
        let v = json!({
            "rev": 7, "regret.cap": 99, "favorite.share": 0.05, "veto.mode": "soft",
            "pool.min_owners": 2, "pool.stretch_rule": "all-but-1", "consensus.bonus": 0.4,
            "regret.absence_decay": 3, "roster.presence": "call-only", "odds.visibility": "hidden",
            "admin.min_pl": 100, "unknown.key": true,
        });
        let st = Settings::from_json(&v);
        assert_eq!(st.rev, 7);
        assert_eq!(st.regret_cap, 10, "clamped to the §6 range");
        assert!(approx(st.favorite_share, 0.2));
        assert_eq!(st.veto_mode, VetoMode::Soft);
        assert_eq!(st.pool_min_owners, OwnerRule::AtLeast(2));
        assert_eq!(st.consensus_bonus, Some(0.4));
        assert_eq!(st.regret_absence_decay, Some(3));
        assert_eq!(st.roster_presence, Presence::CallOnly);
        assert_eq!(st.odds_visibility, OddsVisibility::Hidden);
        assert_eq!(st.admin_min_pl, 100);
        let merged = d.merged(&json!({ "regret.cap": 4 }));
        assert_eq!(merged.regret_cap, 4);
        assert_eq!(merged.favorite_share, d.favorite_share);
        assert_eq!(OwnerRule::All.threshold(4), 4);
        assert_eq!(OwnerRule::AllButOne.threshold(4), 3);
        assert_eq!(OwnerRule::AllButOne.threshold(1), 1);
        assert_eq!(OwnerRule::AtLeast(2).threshold(1), 2);
    }

    #[test]
    fn prefs_and_spin_record_round_trip() {
        let p = Prefs::from_json(&json!({ "wouldPlay": [VALHEIM, ZOMBOID, ZOMBOID, " "], "favorites": [PHASMO, VALHEIM], "definitelyNot": [ZOMBOID], "flexible": false, "updatedAt": 5 }));
        assert_eq!(p.would_play, s(&[VALHEIM, ZOMBOID]));
        let n = p.clone().normalized(1);
        assert_eq!(n.would_play, s(&[VALHEIM]), "definitely-not beats would-play");
        assert_eq!(n.favorites, s(&[VALHEIM]), "favorites ⊆ would-play, one slot");
        assert_eq!(Prefs::from_json(&n.to_json()), n);
        let mut r = spin("s1", 1000, &[KAI, SAM], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], VALHEIM);
        r.seed = "$seed".into();
        r.veto_by.insert(ZOMBOID.into(), JO.into());
        r.vetoed = s(&[ZOMBOID]);
        let back = SpinRecord::from_json(&r.id, "$new", 2000, KAI, &r.to_json());
        assert_eq!(back.seed, "$seed");
        assert_eq!(back.seed_id(), "$seed");
        assert_eq!(back.pool, r.pool);
        assert_eq!(back.roster, r.roster);
        assert_eq!(back.favorites, r.favorites);
        assert_eq!(back.veto_by, r.veto_by);
        assert_eq!(back.winner.as_deref(), Some(VALHEIM));
        assert!(back.voided.is_none());
        let bare = SpinRecord::from_json("sp-x", "$first", 1, KAI, &json!({ "sessionId": "s", "pool": [{ "id": VALHEIM, "weight": 1 }] }));
        assert_eq!(bare.seed_id(), "$first", "no seed = the record IS the first write");
    }

    // ---- 10.5.1: open roster + integer wire form (2026-09-23) ----

    const BLAKE: &str = "@logan:example.com";

    /// roster → pool → weights, chained the way games.rs `tonight` + `draw` do.
    fn tonight_of(rsvps: &[(&str, &str)], present: &[&str], me: &str, pr: &HashMap<String, Prefs>, st: &Settings) -> (Roster, Pool, Vec<GameWeight>) {
        let rs: Vec<(String, String)> = rsvps.iter().map(|(u, v)| (u.to_string(), v.to_string())).collect();
        let r = roster(&rs, &s(present), me, pr, st.roster_presence);
        let pool = build_pool(&r.actual, pr, st, &names());
        let ws = weights(&pool, &r.actual, pr, &HashMap::new(), &[], st);
        (r, pool, ws)
    }

    fn weight_of(ws: &[GameWeight], id: &str) -> f64 {
        ws.iter().find(|w| w.id == id).map(|w| w.weight).unwrap_or(0.0)
    }

    fn no_floats(v: &Value) -> bool {
        match v {
            Value::Number(n) => !n.is_f64(),
            Value::Array(a) => a.iter().all(no_floats),
            Value::Object(o) => o.values().all(no_floats),
            _ => true,
        }
    }

    #[test]
    fn open_roster_is_the_default_and_round_trips() {
        let d = Settings::default();
        assert_eq!(d.roster_presence, Presence::Open);
        assert_eq!(d.to_json()["roster.presence"], json!("open"));
        assert_eq!(Settings::from_json(&json!({ "roster.presence": "open" })).roster_presence, Presence::Open);
        assert_eq!(Settings::from_json(&json!({ "roster.presence": "rsvp+present" })).roster_presence, Presence::RsvpPresent);
        assert_eq!(Settings::from_json(&json!({ "roster.presence": "bogus" })).roster_presence, Presence::Open);
    }

    #[test]
    fn open_roster_one_person_alone() {
        // A real-world prefs shape (2026-09-23): Flexible, three games, no favourite.
        let mut pr = HashMap::new();
        pr.insert(BLAKE.to_string(), prefs(&[VALHEIM, ZOMBOID, PALWORLD], None, &[], true));
        let st = Settings::default();
        let alone_in_call: &[&str] = &[BLAKE];
        let no_call: &[&str] = &[];
        for present in [alone_in_call, no_call] {
            let (r, pool, ws) = tonight_of(&[], present, BLAKE, &pr, &st);
            assert_eq!(r.actual, s(&[BLAKE]), "one person is a roster (present {present:?})");
            assert_eq!(r.flexible, s(&[BLAKE]));
            assert_eq!(pool.main, s(&[ZOMBOID, PALWORLD, VALHEIM]));
            // 10.5.0: a flexible person carries no weight → Σw = 0 → empty reel. Now: even odds.
            assert!(ws.iter().all(|w| w.even && approx(w.weight, EVEN_SHARE)));
            assert!(odds(&ws).iter().all(|o| approx(*o, 1.0 / 3.0)));
        }
        // Not flexible, alone: their own lists and favourite drive the reel.
        pr.insert(BLAKE.to_string(), prefs(&[VALHEIM, ZOMBOID], Some(VALHEIM), &[], false));
        let (_, pool, ws) = tonight_of(&[], &[BLAKE], BLAKE, &pr, &st);
        assert_eq!(pool.main, s(&[ZOMBOID, VALHEIM]));
        assert!(approx(weight_of(&ws, VALHEIM), 0.5) && approx(weight_of(&ws, ZOMBOID), 0.5));
        assert!(ws.iter().all(|w| !w.even));
    }

    #[test]
    fn open_roster_people_joining_and_leaving_mid_session() {
        let mut pr = HashMap::new();
        pr.insert(KAI.to_string(), prefs(&[VALHEIM, ZOMBOID], None, &[], true));
        pr.insert(SAM.to_string(), prefs(&[ZOMBOID, PALWORLD], Some(PALWORLD), &[], false));
        pr.insert(JO.to_string(), prefs(&[ZOMBOID, PALWORLD, PHASMO], Some(ZOMBOID), &[], false));
        let st = Settings::default();
        // Kai (flexible) alone: his games, even odds.
        let (r, pool, ws) = tonight_of(&[], &[KAI], KAI, &pr, &st);
        assert_eq!(r.actual, s(&[KAI]));
        assert_eq!(pool.main, s(&[ZOMBOID, VALHEIM]));
        assert!(ws.iter().all(|w| w.even));
        // Sam joins the call — no RSVP needed: his games join the pool (Kai will play anything),
        // Valheim (Sam doesn't own it) drops to "most of us", and his favourite carries weight.
        let (r, pool, ws) = tonight_of(&[], &[KAI, SAM], KAI, &pr, &st);
        assert_eq!(r.actual, s(&[KAI, SAM]));
        assert_eq!(pool.main, s(&[ZOMBOID, PALWORLD]));
        assert_eq!(pool.stretch, s(&[VALHEIM]));
        assert!(approx(weight_of(&ws, PALWORLD), 0.5) && approx(weight_of(&ws, ZOMBOID), 0.5));
        assert!(ws.iter().all(|w| !w.even));
        // Jo joins too: her Phasmophobia is "most of us" (Sam doesn't own it), Valheim is Kai's
        // alone now, and her vote unit lands on the pool (favourite Zomboid 0.5 + Palworld 0.5).
        let (r, pool, ws) = tonight_of(&[], &[KAI, SAM, JO], KAI, &pr, &st);
        assert_eq!(r.actual, s(&[KAI, SAM, JO]));
        assert_eq!(pool.main, s(&[ZOMBOID, PALWORLD]));
        assert_eq!(pool.stretch, s(&[PHASMO]));
        assert_eq!(pool.tier(VALHEIM), Some(Tier::Removed));
        assert!(approx(weight_of(&ws, ZOMBOID), 1.0) && approx(weight_of(&ws, PALWORLD), 1.0));
        let jo_z: f64 = ws.iter().find(|w| w.id == ZOMBOID).unwrap().shares.iter().filter(|(p, _)| p == JO).map(|x| x.1).sum();
        assert!(approx(jo_z, 0.5), "Jo's favourite share is in the pool");
        // Sam leaves before the spin: his lists and weight drop out at once.
        let (r, pool, ws) = tonight_of(&[], &[KAI, JO], KAI, &pr, &st);
        assert_eq!(r.actual, s(&[KAI, JO]));
        assert!(ws.iter().all(|w| w.shares.iter().all(|(p, _)| p != SAM)));
        assert_eq!(pool.main, s(&[ZOMBOID, PALWORLD, PHASMO]));
        assert_eq!(pool.tier(VALHEIM), Some(Tier::Stretch));
    }

    #[test]
    fn open_roster_rsvps_and_people_without_lists() {
        let mut pr = HashMap::new();
        pr.insert(KAI.to_string(), prefs(&[VALHEIM, ZOMBOID], None, &[], false));
        pr.insert(MAX.to_string(), prefs(&[VALHEIM], None, &[], false));
        pr.insert(JO.to_string(), prefs(&[], None, &[ZOMBOID], false)); // a veto, no games listed
        let st = Settings::default();
        let rsvps = [(MAX, "yes"), (SAM, "maybe"), (RILEY, "no")];
        // In a call: everyone in it counts, RSVP or not; an RSVP'd no-show doesn't.
        let (r, pool, _) = tonight_of(&rsvps, &[KAI, JO, SAM], KAI, &pr, &st);
        assert_eq!(r.expected, s(&[MAX, SAM]));
        assert_eq!(r.actual, s(&[KAI, JO, SAM]));
        assert_eq!(r.maybes, s(&[SAM]));
        // Sam (no prefs) and Jo (only a definitely-not) don't block Kai's games — 10.5.0 made
        // them "own nothing" and emptied the pool — but Jo's veto still removes Zomboid.
        assert_eq!(pool.counted, 1);
        assert_eq!(pool.main, s(&[VALHEIM]));
        assert_eq!(pool.tier(ZOMBOID), Some(Tier::Removed));
        assert!(pool.missing[VALHEIM].is_empty());
        // Outside a call: the planned roster plus the viewer (never listed twice).
        let (r, pool, _) = tonight_of(&rsvps, &[], KAI, &pr, &st);
        assert_eq!(r.actual, s(&[MAX, SAM, KAI]));
        assert_eq!(pool.main, s(&[VALHEIM]));
        assert_eq!(pool.stretch, s(&[ZOMBOID]));
        let (r, _, _) = tonight_of(&rsvps, &[], MAX, &pr, &st);
        assert_eq!(r.actual, s(&[MAX, SAM]));
        // Nobody has listed anything yet: an empty pool, not a blocked one.
        let (r, pool, ws) = tonight_of(&[], &[SAM, RILEY], SAM, &HashMap::new(), &st);
        assert_eq!(r.actual, s(&[SAM, RILEY]));
        assert_eq!((pool.counted, pool.main.len(), ws.len()), (0, 0, 0));
    }

    #[test]
    fn older_presence_modes_are_kept() {
        let pr = HashMap::new();
        let rs: Vec<(String, String)> = vec![(KAI.into(), "yes".into()), (SAM.into(), "maybe".into())];
        assert_eq!(roster(&rs, &s(&[SAM, JO]), JO, &pr, Presence::RsvpPresent).actual, s(&[SAM]));
        assert_eq!(roster(&rs, &s(&[SAM, JO]), JO, &pr, Presence::RsvpOnly).actual, s(&[KAI, SAM]));
        assert_eq!(roster(&rs, &s(&[SAM, JO]), JO, &pr, Presence::CallOnly).actual, s(&[SAM, JO]));
        assert!(roster(&rs, &[], JO, &pr, Presence::CallOnly).actual.is_empty());
        assert!(roster(&rs, &[], JO, &pr, Presence::RsvpPresent).actual.is_empty());
    }

    #[test]
    fn even_share_explains_itself_and_yields_to_flexible_weight() {
        let mut pr = HashMap::new();
        pr.insert(KAI.to_string(), prefs(&[VALHEIM, ZOMBOID], None, &[], true));
        pr.insert(RILEY.to_string(), prefs(&[VALHEIM], None, &[], true));
        let st = Settings::default();
        let (r, pool, ws) = tonight_of(&[], &[KAI, RILEY], KAI, &pr, &st);
        let spinnable: Vec<String> = ws.iter().map(|w| w.id.clone()).collect();
        let ctx = ExplainCtx { pool: &pool, present: &r.actual, prefs: &pr, credits: &HashMap::new(), names: &names(), settings: &st, spinnable: &spinnable };
        let (lines, _) = explain(&ctx, VALHEIM, ws.iter().find(|w| w.id == VALHEIM));
        assert_eq!(lines[0], "2 of you would play this.");
        assert!(lines.contains(&"Nobody here leans toward a game, so every game gets an even share.".to_string()));
        // An admin who gives Flexible people a share gets their spread, not the fallback.
        let st2 = st.merged(&json!({ "flexible.weight": 0.2 }));
        let (_, _, ws2) = tonight_of(&[], &[KAI, RILEY], KAI, &pr, &st2);
        assert!(ws2.iter().all(|w| !w.even));
        assert!(approx(weight_of(&ws2, VALHEIM), 0.2) && approx(weight_of(&ws2, ZOMBOID), 0.2));
    }

    #[test]
    fn even_share_when_the_only_shares_are_zero() {
        // favorite.slots 2 with a 0 second share: Kai's only pool game is his slot-2 favourite →
        // a ZERO share. That is "nobody carries weight" too (Astra review 2026-09-23).
        let st = Settings { favorite_slots: 2, favorite_second_share: 0.0, ..Settings::default() };
        let mut pr = HashMap::new();
        let mut kai = prefs(&[VALHEIM, ZOMBOID], None, &[], false);
        kai.favorites = s(&[VALHEIM, ZOMBOID]);
        pr.insert(KAI.to_string(), kai);
        pr.insert(JO.to_string(), prefs(&[], None, &[VALHEIM], false));
        let (_, pool, ws) = tonight_of(&[], &[KAI, JO], KAI, &pr, &st);
        assert_eq!(pool.main, s(&[ZOMBOID]));
        assert_eq!(ws.len(), 1);
        assert!(ws[0].even && approx(ws[0].weight, EVEN_SHARE), "zero-share pool gets the even share ({})", ws[0].weight);
    }

    #[test]
    fn adhoc_session_ids_roll_over_at_the_fixed_day_boundary() {
        assert_eq!(adhoc_session_id(1_790_139_600_000), "night-2026-09-22", "05:00 UTC on the 23rd is still the 22nd's night");
        assert_eq!(adhoc_session_id(1_790_171_999_999), "night-2026-09-22", "13:59:59 UTC");
        assert_eq!(adhoc_session_id(1_790_172_000_000), "night-2026-09-23", "14:00 UTC rolls over");
        assert_eq!(adhoc_session_id(951_832_800_000), "night-2000-02-29");
        assert_eq!(adhoc_session_id(1_735_686_000_000), "night-2024-12-31");
        assert_eq!(adhoc_session_id(0), "night-1970-01-01");
        assert_eq!(night_end_ms(1_790_139_600_000), 1_790_172_000_000);
        assert_eq!(night_end_ms(1_790_172_000_000), 1_790_258_400_000);
        assert!(is_adhoc_session("night-2026-09-23") && !is_adhoc_session("ev-1790000000000-abcd"));
        assert_eq!(effective_session("", false, 1_790_139_600_000), "night-2026-09-22");
        assert_eq!(effective_session("ev-1", true, 1_790_139_600_000), "ev-1");
        assert_eq!(effective_session("ev-gone", false, 1_790_139_600_000), "night-2026-09-22");
        assert_eq!(effective_session("night-2026-09-21", false, 1_790_139_600_000), "night-2026-09-21", "already resolved");
    }

    #[test]
    fn spin_record_pool_travels_as_integer_micro_units() {
        let round6_pool = worked_record_pool();
        let micro: Vec<(String, f64)> = round6_pool.iter().map(|(g, w)| (g.clone(), to_micro(*w))).collect();
        assert_eq!(micro.iter().map(|p| p.1).collect::<Vec<_>>(), vec![1_500_000.0, 1_250_000.0, 1_000_000.0, 750_000.0]);
        assert_eq!(to_micro(round6(1.0 / 6.0)), 166_667.0);
        // Readers are scale-free: the §12 vectors land on the same winners (10.5.0 ↔ 10.5.1 ↔ web).
        for v in ["$spin-vector-3:example.com", "$spin-vector-5:example.com", "$spin-vector-23:example.com"] {
            assert_eq!(derive_winner(v, &micro), derive_winner(v, &round6_pool), "{v}");
        }
        let mut r = spin("s1", 1000, &[KAI, SAM], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], VALHEIM);
        r.pool = micro.clone();
        r.seed = "$seed".into();
        let j = r.to_json();
        assert!(no_floats(&j), "Synapse refuses floats in room-v10 state: {j}");
        assert_eq!(j["pool"][0]["weight"], json!(1_500_000));
        let back = SpinRecord::from_json(&r.id, "$x", 1, KAI, &j);
        assert_eq!(back.pool, micro);
        // A pre-10.5.1 float record (history on other servers) still reads, and is never re-scaled.
        let old = SpinRecord::from_json("sp-o", "$o", 1, KAI, &json!({ "sessionId": "s", "pool": [{ "id": PALWORLD, "weight": 1.5 }, { "id": VALHEIM, "weight": 1.25 }] }));
        assert_eq!(old.pool, vec![(PALWORLD.to_string(), 1.5), (VALHEIM.to_string(), 1.25)]);
        assert_eq!(old.to_json()["pool"][0]["weight"], json!(1.5));
    }

    #[test]
    fn settings_wire_form_has_no_floats() {
        let d = Settings::default();
        let w = d.to_wire().unwrap();
        assert!(no_floats(&w), "{w}");
        assert_eq!(w["flexible.weight"], json!(0));
        assert!(w.get("favorite.share").is_none() && w.get("damping.factor").is_none(), "default fractions are omitted");
        assert_eq!(w["roster.presence"], json!("open"));
        assert_eq!(w["regret.cap"], json!(3));
        assert_eq!(Settings::from_json(&w), d, "readers default what was omitted");
        let bonus = d.merged(&json!({ "consensus.bonus": 0.4 }));
        assert_eq!(bonus.to_wire().unwrap()["consensus.bonus"], json!("0.4"));
        assert_eq!(Settings::from_json(&bonus.to_wire().unwrap()).consensus_bonus, Some(0.4));
        assert_eq!(d.merged(&json!({ "favorite.share": 0.6 })).to_wire(), Err(FLOAT_REFUSAL.to_string()));
        assert_eq!(d.merged(&json!({ "damping.factor": 1.0 })).to_wire().unwrap()["damping.factor"], json!(1));
        // Key for key what the web writes (tw-spec §4.7 example, rev bumped).
        let mut two = d.clone();
        two.rev = 2;
        two.regret_cap = 4;
        let w2 = two.to_wire().unwrap();
        assert_eq!(w2["rev"], json!(2));
        assert_eq!(w2["regret.absence_decay"], json!(false));
        assert_eq!(w2["consensus.bonus"], json!(false));
        assert_eq!(w2["admin.min_pl"], json!(50));
    }

    // ---- 10.5.2: admin "Reset spins" (port of the web's model/games.test.ts test of the same name) ----

    /// Tonight in session s1: initial A, veto re-spin B, re-spin C (limit 2 → 0 left) + a race
    /// loser L (sibling of A). Honest records: the written winner is the one the seed derives.
    fn reset_night() -> Vec<SpinRecord> {
        let pool3 = [VALHEIM, ZOMBOID, PALWORLD];
        let mut a = spin("s1", 5000, &[KAI, SAM], &[], &pool3, VALHEIM);
        a.id = "sp-a".into();
        a.sender = KAI.into();
        let mut l = spin("s1", 5200, &[KAI, SAM], &[], &pool3, ZOMBOID);
        l.id = "sp-l".into();
        l.sender = SAM.into();
        let mut b = spin("s1", 6000, &[KAI, SAM], &[], &[ZOMBOID, PALWORLD], ZOMBOID);
        b.id = "sp-b".into();
        b.kind = "respin".into();
        b.supersedes = Some("sp-a".into());
        b.vetoed = s(&[VALHEIM]);
        b.veto_by = [(VALHEIM.to_string(), SAM.to_string())].into_iter().collect();
        let mut c = spin("s1", 7000, &[KAI, SAM], &[], &[ZOMBOID, PALWORLD], PALWORLD);
        c.id = "sp-c".into();
        c.kind = "respin".into();
        c.supersedes = Some("sp-b".into());
        c.vetoed = s(&[VALHEIM]);
        c.veto_by = [(VALHEIM.to_string(), SAM.to_string())].into_iter().collect();
        let mut recs = vec![a, l, b, c];
        for r in recs.iter_mut() {
            let w = derive_winner(&r.event_id, &r.pool);
            r.winner = w.clone();
            r.computed_winner = w;
        }
        recs
    }

    /// The night on the wire: each record's content with `seed` = its first write.
    fn reset_wire(recs: &[SpinRecord]) -> Vec<RawSpin> {
        recs.iter()
            .map(|r| {
                let mut c = r.to_json();
                c["seed"] = json!(r.event_id);
                RawSpin { id: r.id.clone(), event_id: r.event_id.clone(), content: c }
            })
            .collect()
    }

    fn reset_ts(id: &str) -> u64 {
        match id {
            "sp-a" => 5000,
            "sp-l" => 5200,
            "sp-b" => 6000,
            "sp-c" => 7000,
            _ => 0,
        }
    }

    /// What a reader parses. `ts` = what it sorts by: the seed's when fetched, else the CURRENT
    /// event's (a rewrite is "now").
    fn reset_parse(wire: &[RawSpin], rewritten: &HashSet<String>, seed_fetched: bool) -> Vec<SpinRecord> {
        wire.iter()
            .map(|w| {
                let ts = if seed_fetched || !rewritten.contains(&w.id) { reset_ts(&w.id) } else { 90_000 };
                SpinRecord::from_json(&w.id, &w.event_id, ts, KAI, &w.content)
            })
            .collect()
    }

    #[test]
    fn admin_reset_spins_nothing_stands_and_the_next_spin_counts_no_respins() {
        let recs = reset_night();
        let wire0 = reset_wire(&recs);
        let none = HashSet::new();
        let r0 = resolve_session(&reset_parse(&wire0, &none, true));
        assert_eq!(r0.chain, s(&["sp-a", "sp-b", "sp-c"]));
        assert_eq!(respins_used_in(&r0.chain, &reset_parse(&wire0, &none, true)), 2, "limit reached");
        // why a plain void is not a reset: the standing re-spin falls back to B, count unchanged
        let plain: Vec<SpinRecord> = reset_parse(&wire0, &none, true)
            .into_iter()
            .map(|mut r| {
                if r.id == "sp-c" {
                    r.voided = Some(Voided { by: JO.into(), reason: "x".into(), at: 1 });
                }
                r
            })
            .collect();
        assert_eq!(resolve_session(&plain).final_id.as_deref(), Some("sp-b"));
        assert_eq!(respins_used_in(&resolve_session(&plain).chain, &plain), 2);

        let writes = reset_writes(&r0.chain, &wire0, JO, 99);
        // pass 1: the loser, then the chain root → tail (the last write also re-kinds); pass 2: the other re-spin
        assert_eq!(writes.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["sp-l", "sp-a", "sp-b", "sp-c", "sp-b"]);
        // replay write by write: while ANY result stands, the count never drops (nobody gets a re-spin back early)
        let mut wire = wire0.clone();
        let mut rewritten: HashSet<String> = HashSet::new();
        for (i, (id, c)) in writes.iter().enumerate() {
            for w in wire.iter_mut().filter(|w| &w.id == id) {
                *w = RawSpin { id: id.clone(), event_id: format!("$rw{i}"), content: c.clone() };
            }
            rewritten.insert(id.clone());
            let now = reset_parse(&wire, &none, true);
            let r = resolve_session(&now);
            if r.final_id.is_some() {
                assert_eq!(respins_used_in(&r.chain, &now), 2, "after write {i} ({id}) a result stands with the full count");
            } else {
                assert!(i >= 3, "nothing stands only once the standing record is voided (write {i})");
            }
        }
        for (id, c) in &writes {
            let was = &wire0.iter().find(|w| &w.id == id).unwrap().content;
            for k in ["sessionId", "pool", "roster", "late", "winner", "seed", "supersedes", "vetoed", "vetoBy"] {
                assert_eq!(c[k], was[k], "{id}.{k} unchanged");
            }
        }
        let fin: HashMap<String, Value> = wire.iter().map(|w| (w.id.clone(), w.content.clone())).collect();
        for id in ["sp-a", "sp-l", "sp-b", "sp-c"] {
            assert_eq!(fin[id]["voided"], json!({ "by": JO, "reason": RESET_REASON, "at": 99 }));
        }
        assert_eq!(["sp-a", "sp-l", "sp-b", "sp-c"].map(|id| fin[id]["kind"].clone()), [json!("initial"), json!("initial"), json!(RESET_KIND), json!(RESET_KIND)]);
        assert!(writes.iter().all(|(_, c)| c["voided"]["at"].is_u64()), "an integer timestamp — Synapse refuses floats");
        let after = reset_parse(&wire, &none, true);
        let r1 = resolve_session(&after);
        assert_eq!(r1.final_id, None, "nothing stands → Spin enabled");
        assert_eq!(respins_used_in(&r1.chain, &after), 0);
        // a reader that couldn't fetch the seeds sees the rewrites as "now": the loser L becomes the root — still nothing stands
        let unfetched: Vec<SpinRecord> = reset_parse(&wire, &rewritten, false)
            .into_iter()
            .map(|mut r| {
                if r.id == "sp-l" {
                    r.ts = 5200;
                }
                r
            })
            .collect();
        let ru = resolve_session(&unfetched);
        assert_eq!(ru.final_id, None);
        assert_eq!(respins_used_in(&ru.chain, &unfetched), 0);
        // (every rewritten record still verifies against its seed: games.rs `reset_rewrites_still_verify`)
        // the next spin chains onto the tail (web + 10.5.1 do_spin): it stands, 0 re-spins used, no vetoes
        let mut next = spin("s1", 8000, &[KAI, SAM], &[], &[VALHEIM, ZOMBOID, PALWORLD], VALHEIM);
        next.id = "sp-n".into();
        next.supersedes = Some("sp-c".into());
        let with_next: Vec<SpinRecord> = after.iter().cloned().chain(std::iter::once(next.clone())).collect();
        let r2 = resolve_session(&with_next);
        assert_eq!(r2.chain, s(&["sp-a", "sp-b", "sp-c", "sp-n"]));
        assert_eq!(r2.final_id.as_deref(), Some("sp-n"));
        assert_eq!(respins_used_in(&r2.chain, &with_next), 0, "full re-spins back");
        assert!(next.vetoed.is_empty());
        // a re-run writes nothing; a half-done run only finishes the rest; an earlier void keeps its reason
        assert!(reset_writes(&r0.chain, &wire, JO, 100).is_empty());
        let half: Vec<RawSpin> = wire0
            .iter()
            .cloned()
            .map(|mut w| {
                if w.id == "sp-b" {
                    w.content["voided"] = json!({ "by": SAM, "reason": "earlier", "at": 5 });
                }
                w
            })
            .collect();
        let w2 = reset_writes(&r0.chain, &half, JO, 101);
        assert_eq!(w2.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["sp-l", "sp-a", "sp-c", "sp-b"]);
        assert_eq!(w2.last().unwrap().1["voided"], json!({ "by": SAM, "reason": "earlier", "at": 5 }));
        assert_eq!(w2.last().unwrap().1["kind"], json!(RESET_KIND));
        // a record without a seed (the spinner never wrote the winner) gets its current event id as the seed
        let mut bare = recs[0].to_json();
        bare.as_object_mut().unwrap().remove("seed");
        let no_seed = [RawSpin { id: "sp-a".into(), event_id: "$first".into(), content: bare }];
        assert_eq!(reset_writes(&s(&["sp-a"]), &no_seed, JO, 1)[0].1["seed"], json!("$first"));
    }

    #[test]
    fn admin_reset_spins_edge_cases() {
        // emptied records (`{}`, what TINHAT_GAMES_CLEAN writes) and non-objects are skipped
        let recs = reset_night();
        let mut wire = reset_wire(&recs);
        wire.push(RawSpin { id: "sp-gone".into(), event_id: "$g".into(), content: json!({}) });
        wire.push(RawSpin { id: "sp-odd".into(), event_id: "$o".into(), content: json!([1, 2]) });
        let chain = s(&["sp-a", "sp-b", "sp-c"]);
        let ids: Vec<String> = reset_writes(&chain, &wire, JO, 7).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, s(&["sp-l", "sp-a", "sp-b", "sp-c", "sp-b"]));
        // a chain id with no stored record is skipped, not invented
        let ids: Vec<String> = reset_writes(&s(&["sp-a", "sp-missing"]), &wire[..1], JO, 7).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, s(&["sp-a"]));
        // unknown fields a newer client wrote ride along untouched; a non-object `voided` is replaced
        let mut odd = wire[0].clone();
        odd.content["x-future"] = json!({ "keep": [1, "two"] });
        odd.content["voided"] = json!(true);
        let w = reset_writes(&s(&["sp-a"]), &[odd], JO, 7);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].1["x-future"], json!({ "keep": [1, "two"] }));
        assert_eq!(w[0].1["voided"], json!({ "by": JO, "reason": RESET_REASON, "at": 7 }));
        // only re-spins are re-kinded, however many there are; the standing tail's re-kind rides on its void
        let ww = reset_writes(&chain, &reset_wire(&recs), JO, 7);
        assert_eq!(ww.iter().filter(|(_, c)| c["kind"] == json!(RESET_KIND)).count(), 2);
        assert_eq!(ww[3].1["kind"], json!(RESET_KIND), "C's void also re-kinds it (the last write of pass 1)");
        // nothing tonight → nothing to write
        assert!(reset_writes(&[], &[], JO, 7).is_empty());
    }

    // ---- 10.5.2: credits (2026-09-23: admins couldn't reliably undo the spin cost; a toggle between
    // spending credits and free spins) — the same vectors as the web's games.test.ts ----

    fn adj_of(delta: i64, set: Option<i64>, as_of: u64, written: u64) -> Adjustment {
        Adjustment { delta, set, as_of, reason: String::new(), written }
    }
    fn adm<'a>(admins: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |m: &str| admins.contains(&m)
    }

    #[test]
    fn admin_adjustments_land_only_admins_count_last_write_wins() {
        let ev = |state_key: &str, sender: &str, ts: u64, content: Value| AdjustEvent { state_key: state_key.into(), sender: sender.into(), ts, content };
        let events = vec![
            ev(&admin_adjust_key(KAI), JO, 5000, json!({ "delta": 1, "asOf": 4999, "reason": "r", "by": JO, "at": 4999 })),
            ev(KAI, KAI, 6000, json!({ "delta": 9, "asOf": 6000 })), // Kai's own key, Kai no admin → ignored
            ev(&admin_adjust_key(SAM), SAM, 5000, json!({ "delta": 5, "asOf": 5000 })), // forged by a member
            ev(JO, JO, 3000, json!({ "set": 0, "delta": 0, "asOf": 3000 })), // an admin's own (10.5.1) reset …
            ev(&admin_adjust_key(JO), MAX, 2000, json!({ "delta": 2, "asOf": 2000 })), // … is newer → wins whole
            ev(MAX, MAX, 7000, json!({ "delta": 1, "asOf": 7000 })),
            ev(&admin_adjust_key(MAX), JO, 7000, json!({ "delta": 3, "asOf": 7000 })), // same ms → the admin key
            ev(&admin_adjust_key(RILEY), JO, 8000, json!({})), // emptied
            ev("admin:riley", JO, 8000, json!({ "delta": 4 })), // not a user id
            ev("admin:", JO, 8000, json!({ "delta": 4 })),
            ev(RILEY, JO, 8000, json!({ "delta": 4 })), // @-key of someone else (Matrix refuses it anyway)
            ev(&admin_adjust_key(RILEY), JO, 9000, json!({ "delta": -2, "asOf": 9000 })),
        ];
        let m = merge_adjustments(&events, adm(&[JO, MAX]));
        let mut keys: Vec<&String> = m.keys().collect();
        keys.sort();
        let mut want = vec![JO, KAI, MAX, RILEY];
        want.sort();
        assert_eq!(keys, want);
        assert_eq!(m[KAI], Adjustment { reason: "r".into(), ..adj_of(1, None, 4999, 5000) });
        assert_eq!(m[JO], adj_of(0, Some(0), 3000, 3000));
        assert_eq!(m[MAX], adj_of(3, None, 7000, 7000));
        assert_eq!(m[RILEY], adj_of(-2, None, 9000, 9000));
        assert!(!m.contains_key(SAM), "a member can't forge an admin adjustment");
        // the member becomes an admin → the same event counts; the admin is demoted → theirs stop counting
        assert_eq!(merge_adjustments(&events, adm(&[JO, MAX, SAM])).get(SAM).map(|a| a.delta), Some(5));
        assert!(!merge_adjustments(&events, adm(&[MAX])).contains_key(KAI));
        assert_eq!(adjust_target("admin:@a:b"), Some(("@a:b".to_string(), true)));
        assert_eq!(adjust_target("@a:b"), Some(("@a:b".to_string(), false)));
        assert_eq!(adjust_target("admin:a"), None);
        assert_eq!(adjust_target(""), None);
    }

    #[test]
    fn a_spin_with_credit_false_is_skipped_like_a_voided_one() {
        let st = Settings::default();
        let off = |mut r: SpinRecord| {
            r.credit = false;
            r
        };
        let f = vec![
            spin("s4", 1004, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
            off(spin("s3", 1003, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], VALHEIM)), // a win that isn't counted
            spin("s2", 1002, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
            off(spin("s1", 1001, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID)),
        ];
        assert_eq!(derive_credit(KAI, &f, &kai_now(), &st, None), 2, "s4 + s2; the uncounted win doesn't end the streak");
        let all: Vec<SpinRecord> = f.iter().cloned().map(|mut r| {
            r.credit = true;
            r
        }).collect();
        assert_eq!(derive_credit(KAI, &all, &kai_now(), &st, None), 1, "all counted: the s3 win ends it");
        // an uncounted night is not an absence either
        let g = vec![spin("s3", 1003, &[SAM], &[], &[VALHEIM, ZOMBOID], ZOMBOID), off(spin("s2", 1002, &[SAM], &[], &[VALHEIM, ZOMBOID], ZOMBOID)), spin("s1", 1001, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID)];
        let decay2 = Settings { regret_absence_decay: Some(2), ..Settings::default() };
        assert_eq!(derive_credit(KAI, &g, &kai_now(), &decay2, None), 1, "one absence (s3) of two needed");
        let g_all: Vec<SpinRecord> = g.iter().cloned().map(|mut r| {
            r.credit = true;
            r
        }).collect();
        assert_eq!(derive_credit(KAI, &g_all, &kai_now(), &decay2, None), 0);
        // wire: only the exception travels; a legacy record (no key) counts; only a literal false disables
        let base = json!({ "v": 1, "sessionId": "s", "pool": [{ "id": VALHEIM, "weight": 1000000 }], "roster": [KAI] });
        let with = |k: Value| {
            let mut c = base.clone();
            c["credit"] = k;
            SpinRecord::from_json("sp", "$e", 1, KAI, &c).credit
        };
        assert!(SpinRecord::from_json("sp", "$e", 1, KAI, &base).credit);
        assert!(!with(json!(false)));
        assert!(with(json!(true)));
        assert!(with(json!(0)));
        assert!(with(json!("false")));
        let r = SpinRecord::from_json("sp", "$e", 1, KAI, &base);
        assert!(r.to_json().get("credit").is_none(), "a counting record is byte-identical to 10.5.1's");
        assert_eq!(SpinRecord { credit: false, ..r }.to_json()["credit"], json!(false));
        assert!(SpinRecord::default().credit, "a record counts unless it says otherwise");
    }

    #[test]
    fn an_adjustments_offset_lapses_when_its_streak_ends() {
        let st = Settings::default();
        let f = vec![
            spin("s3", 3000, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
            spin("s2", 2000, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], VALHEIM), // Kai's favourite won
            spin("s1", 1000, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID),
        ];
        let d = |a: Option<Adjustment>| {
            let x = credit_detail(KAI, &f, &kai_now(), &st, a.as_ref());
            (x.credit, x.offset_live)
        };
        assert_eq!(d(None), (1, true));
        assert_eq!(d(Some(adj_of(1, None, 2500, 2500))), (2, true), "+1 given after the win");
        assert_eq!(d(Some(adj_of(1, None, 1500, 1500))), (1, false), "+1 given before the win: spent");
        assert_eq!(d(Some(adj_of(1, None, 1500, 2000))), (2, true), "written the same ms as the win: kept");
        assert_eq!(d(Some(adj_of(1, None, 2500, 0))), (2, true), "written unknown: asOf decides");
        assert_eq!(d(Some(adj_of(-3, None, 1500, 1500))), (1, false), "a lapsed −3 no longer eats the new streak");
        assert_eq!(d(Some(adj_of(2, Some(0), 2500, 2600))), (3, true), "reset line at 2500: s3 + 2");
        assert_eq!(d(Some(adj_of(0, Some(0), 1500, 1500))), (1, false), "an old reset: nothing to lapse");
        // a favourite change after the adjustment ends the streak too (reset_on_favorite_change)
        let mut g = f.clone();
        g[0] = spin("s3", 3000, &[KAI], &[(KAI, ZOMBOID)], &[VALHEIM, ZOMBOID], VALHEIM);
        let plus = adj_of(1, None, 2500, 2500);
        assert_eq!(credit_detail(KAI, &g, &kai_now(), &st, Some(&plus)), CreditDetail { credit: 0, offset_live: false });
        let keep = Settings { regret_reset_on_favorite_change: false, ..Settings::default() };
        assert!(credit_detail(KAI, &g, &kai_now(), &keep, Some(&plus)).offset_live);
        // an uncounted night never ends a streak
        let mut h = f.clone();
        h[1].credit = false;
        let early = adj_of(1, None, 1500, 1500);
        assert_eq!(credit_detail(KAI, &h, &kai_now(), &st, Some(&early)), CreditDetail { credit: 3, offset_live: true });
        // no favourite / regret off: the writer carries the adjustment forward
        assert_eq!(credit_detail(KAI, &f, &prefs(&[VALHEIM], None, &[], false), &st, Some(&early)), CreditDetail { credit: 0, offset_live: true });
        let off = Settings { regret_cap: 0, ..Settings::default() };
        assert_eq!(credit_detail(KAI, &f, &kai_now(), &off, Some(&early)), CreditDetail { credit: 0, offset_live: true });
        // a huge adjustment saturates at u32::MAX on both sides (no wrap)
        assert_eq!(credit_detail(KAI, &[], &kai_now(), &st, Some(&adj_of(4_294_967_296, None, 0, 0))).credit, u32::MAX);
    }

    #[test]
    fn the_admin_writes_adjust_zero_and_the_credit_flag() {
        assert_eq!(adjust_content(None, true, 1, " make-up ", JO, 5000), json!({ "delta": 1, "asOf": 5000, "reason": "make-up", "by": JO, "at": 5000 }));
        let reset = adj_of(2, Some(0), 3000, 3100);
        assert_eq!(adjust_content(Some(&reset), true, -1, "x", JO, 5000), json!({ "delta": 1, "asOf": 3000, "reason": "x", "by": JO, "at": 5000, "set": 0 }));
        assert_eq!(adjust_content(Some(&reset), false, 1, "x", JO, 5000), json!({ "delta": 1, "asOf": 3000, "reason": "x", "by": JO, "at": 5000, "set": 0 }), "lapsed: the offset starts over, the reset line stays");
        assert_eq!(adjust_content(Some(&adj_of(0, Some(2), 3000, 3100)), false, 1, "x", JO, 5000), json!({ "delta": 1, "asOf": 3000, "reason": "x", "by": JO, "at": 5000, "set": 0 }), "a lapsed set 2 goes, its line stays");
        assert_eq!(adjust_content(Some(&adj_of(2, None, 3000, 3000)), false, 1, "x", JO, 5000), json!({ "delta": 1, "asOf": 5000, "reason": "x", "by": JO, "at": 5000 }));
        assert_eq!(adjust_content(Some(&adj_of(2, None, 3000, 3000)), true, 1, "x", JO, 5000), json!({ "delta": 3, "asOf": 5000, "reason": "x", "by": JO, "at": 5000 }));
        assert_eq!(zero_content("Reset", JO, 5000), json!({ "set": 0, "delta": 0, "asOf": 5000, "reason": "Reset", "by": JO, "at": 5000 }));
        assert_eq!(adjust_content(Some(&adj_of(2, None, 3000, 3000)), true, 1, "x", JO, 5000), json!({ "delta": 3, "asOf": 5000, "reason": "x", "by": JO, "at": 5000 }));
        assert_eq!(credit_override_key("sp-1-abcd"), "credit:sp-1-abcd");
        assert_eq!(credit_override_content(false, JO, 5000), json!({ "credit": false, "by": JO, "at": 5000 }));
        // the override key is not a person: adjustments and Reset all credits never see it
        assert_eq!(adjust_target("credit:sp-1-abcd"), None);
        let ov = AdjustEvent { state_key: "credit:sp-1-abcd".into(), sender: JO.into(), ts: 1, content: json!({ "credit": false }) };
        assert!(merge_adjustments(&[ov], adm(&[JO])).is_empty());
    }

    #[test]
    fn dont_count_is_admin_only_state_and_a_members_rewrite_cant_change_the_flag() {
        let ev = |state_key: &str, sender: &str, content: Value| AdjustEvent { state_key: state_key.into(), sender: sender.into(), ts: 1, content };
        let o = credit_overrides(
            &[
                ev("credit:sp-a", JO, json!({ "credit": false })),
                ev("credit:sp-b", SAM, json!({ "credit": false })), // a member (the event's PL lowered) → ignored
                ev("credit:sp-c", JO, json!({ "credit": true })),
                ev("credit:sp-d", JO, json!({})), // emptied → none
                ev("credit:sp-e", JO, json!({ "credit": "false" })), // junk → none
                ev("credit:", JO, json!({ "credit": false })),
                ev(&admin_adjust_key(KAI), JO, json!({ "credit": false, "delta": 1 })), // an adjustment, not an override
            ],
            adm(&[JO]),
        );
        let mut got: Vec<(String, bool)> = o.into_iter().collect();
        got.sort();
        assert_eq!(got, vec![("sp-a".to_string(), false), ("sp-c".to_string(), true)]);
        assert!(trusted_credit_flag(false, Some(true), false));
        assert!(!trusted_credit_flag(false, Some(true), true));
        assert!(!trusted_credit_flag(false, None, false), "seed not fetched: the copy we have");
        assert!(!trusted_credit_flag(true, Some(false), false));
    }

    #[test]
    fn credits_mode_earn_by_default_free_parses_travels_float_free() {
        assert_eq!(Settings::default().credits_mode, CreditsMode::Earn);
        assert_eq!(Settings::from_json(&json!({})).credits_mode, CreditsMode::Earn);
        assert_eq!(Settings::from_json(&json!({ "credits.mode": "free" })).credits_mode, CreditsMode::Free);
        assert_eq!(Settings::from_json(&json!({ "credits.mode": " FREE " })).credits_mode, CreditsMode::Free);
        assert_eq!(Settings::from_json(&json!({ "credits.mode": "gratis" })).credits_mode, CreditsMode::Earn);
        assert_eq!(Settings::from_json(&json!({ "credits.mode": 1 })).credits_mode, CreditsMode::Earn);
        assert_eq!(Settings::default().to_json()["credits.mode"], json!("earn"));
        let free = Settings::default().merged(&json!({ "credits.mode": "free" }));
        assert_eq!(free.credits_mode, CreditsMode::Free);
        assert_eq!(free.to_wire().unwrap()["credits.mode"], json!("free"));
        assert_eq!(Settings::from_json(&free.to_json()).credits_mode, CreditsMode::Free);
    }

    #[test]
    fn free_spins_ignore_banked_credit_and_flipping_mid_night_never_rewrites_a_spin() {
        let f: Vec<SpinRecord> = (0..2u64).map(|i| spin(&format!("s{i}"), 1000 - i, &[KAI, SAM], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID)).collect();
        let mut pr = HashMap::new();
        pr.insert(KAI.to_string(), kai_now());
        pr.insert(SAM.to_string(), prefs(&[VALHEIM, ZOMBOID], Some(ZOMBOID), &[], false));
        let mut adjs = HashMap::new();
        adjs.insert(SAM.to_string(), adj_of(1, None, 5000, 5000)); // given after both nights (Sam had no favourite on record then)
        let roster = s(&[KAI, SAM, JO]);
        let earn = tonight_credits(&roster, &pr, &f, &adjs, &Settings::default());
        assert_eq!((earn.len(), earn[KAI], earn[SAM]), (2, 2, 1));
        let free_s = Settings { credits_mode: CreditsMode::Free, ..Settings::default() };
        let free = tonight_credits(&roster, &pr, &f, &adjs, &free_s);
        assert_eq!((free.len(), free[KAI], free[SAM]), (2, 0, 0));
        // a night = its final record: an earn spin then a free re-spin → the night doesn't count, and back
        let r1 = SpinRecord { id: "sp-1".into(), ..spin("n", 100, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID) };
        let r2 = SpinRecord { id: "sp-2".into(), kind: "respin".into(), supersedes: Some("sp-1".into()), credit: false, ..spin("n", 200, &[KAI], &[(KAI, VALHEIM)], &[VALHEIM, ZOMBOID], ZOMBOID) };
        let final_of = |recs: &[SpinRecord]| -> SpinRecord {
            let id = resolve_session(recs).final_id.unwrap();
            recs.iter().find(|x| x.id == id).unwrap().clone()
        };
        assert_eq!(derive_credit(KAI, &[final_of(&[r1.clone(), r2.clone()])], &kai_now(), &Settings::default(), None), 0, "free re-spin stands: the night is free");
        let flipped = [SpinRecord { credit: false, ..r1 }, SpinRecord { credit: true, ..r2 }];
        assert_eq!(derive_credit(KAI, &[final_of(&flipped)], &kai_now(), &Settings::default(), None), 1, "earn re-spin after a free spin counts");
        // weights: with free credits Kai's favourite is not weighted up
        let who = s(&[KAI, SAM]);
        let pool = build_pool(&who, &pr, &Settings::default(), &names());
        let w_earn = weights(&pool, &who, &pr, &tonight_credits(&who, &pr, &f, &HashMap::new(), &Settings::default()), &[], &Settings::default());
        let w_free = weights(&pool, &who, &pr, &tonight_credits(&who, &pr, &f, &HashMap::new(), &free_s), &[], &Settings::default());
        let val = |ws: &[GameWeight]| ws.iter().find(|g| g.id == VALHEIM).unwrap().weight;
        assert!(approx(val(&w_earn), 0.5 * (1.0 + 0.5 * 2.0) + 0.5), "earn: Kai 0.5 × (1 + 0.5 × credit 2) + Sam 0.5");
        assert!(approx(val(&w_free), 1.0), "free: 0.5 + 0.5");
    }
}
