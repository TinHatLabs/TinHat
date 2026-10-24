//! sync.rs — port of `sync.ts`: LAYOUT.md §9 (v1.1): the account-data document `dev.tinhat.layouts` — entry order,
//! version vectors (chain keys), the per-layout reduction (the winner + the concurrent losers, "conflicts"), save() (a
//! new entry that always wins its own merge), merge() (a join over the entries; the size rule only drops `previous`
//! snapshots), and the device protocol (incoming documents, lost-race detection, re-assert, the newer-format latch) as a
//! deterministic state machine.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::json::{canon_bytes, canon_opt, cp_len, is_json_value, js_str, mk, utf16_len, J};
use crate::repair::{repair_desktop, repair_phone};
use crate::schema::{classic_desktop, classic_phone, is_id_syntax, schema};

pub const MAX_REV: i64 = 2147483647;
pub const MAX_SAVED_AT: i64 = 9007199254740991;
pub const ENTRY_KEYS: [&str; 11] = ["id", "name", "rev", "savedAt", "by", "vv", "deleted", "desktop", "phone", "previous", "quarantine"];
const DOC_KEYS: [&str; 4] = ["format", "v", "layouts", "conflicts"];
/// §9.2 input budget: a document over 1 MiB canonical or with more than 1024 entries is not read (ignored).
pub const DOC_BUDGET_BYTES: usize = 1048576;
pub const DOC_BUDGET_ENTRIES: usize = 1024;

fn name_ok(n: Option<&J>) -> bool {
    n.and_then(J::as_str).is_some_and(|s| {
        let l = cp_len(s);
        (1..=40).contains(&l)
    })
}
/// An entry's `by` as read: at most 64 code points, no "#" ("" = unknown device, the default of an older draft).
fn by_ok_str(s: &str) -> bool {
    cp_len(s) <= 64 && !s.contains('#')
}
fn by_ok(s: Option<&J>) -> bool {
    s.and_then(J::as_str).is_some_and(by_ok_str)
}
/// A device id (a saver): 1..64 code points, no "#" (the separator of chain keys).
fn device_ok_str(s: &str) -> bool {
    by_ok_str(s) && utf16_len(s) >= 1
}
fn device_ok(s: Option<&J>) -> bool {
    s.and_then(J::as_str).is_some_and(device_ok_str)
}
/// The device part of a version-vector key: `<device>` or `<device>#<n>` (a later chain of that device, §9.6).
fn key_base(k: &str) -> &str {
    match k.find('#') {
        Some(i) => &k[..i],
        None => k,
    }
}
/// A version-vector key: a device id, optionally followed by "#" and a canonical integer of 1..10 digits.
pub fn vv_key_ok(k: &str) -> bool {
    match k.find('#') {
        None => device_ok_str(k),
        Some(i) => {
            let n = &k.as_bytes()[i + 1..];
            device_ok_str(&k[..i]) && !n.is_empty() && n.len() <= 10 && (b'1'..=b'9').contains(&n[0]) && n.iter().all(u8::is_ascii_digit)
        }
    }
}

// ---- entries -----------------------------------------------------------------------------------------------------
/// An entry's layout id (a valid id string), else None (such an entry cannot be merged).
pub fn entry_id(e: Option<&J>) -> Option<String> {
    let e = e.filter(|e| e.is_obj())?;
    e.get_str("id").filter(|id| is_id_syntax(id)).map(|s| s.to_string())
}

fn int_range(v: Option<J>, lo: i64, hi: i64) -> Option<i64> {
    let v = v?;
    if !v.is_int() {
        return None;
    }
    let x = v.as_f64()?;
    (x >= lo as f64 && x <= hi as f64).then_some(x as i64)
}

/// (rev, savedAt, by) with the defaults of an older draft: an invalid field reads as 0 / 0 / "".
pub fn tuple_of(e: &J) -> (i64, i64, String) {
    let rev = int_range(e.get("rev"), 0, MAX_REV).unwrap_or(0);
    let at = int_range(e.get("savedAt"), 0, MAX_SAVED_AT).unwrap_or(0);
    let by = e.get("by").filter(|b| by_ok(Some(b))).and_then(|b| b.as_str().map(str::to_string)).unwrap_or_default();
    (rev, at, by)
}
pub fn cmp_tuple(a: &J, b: &J) -> Ordering {
    let (x, y) = (tuple_of(a), tuple_of(b));
    x.0.cmp(&y.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2))
}
/// The merge order: the tuple, then the canonical text (code-point order). A total order on entries.
pub fn cmp_entry(a: &J, b: &J) -> Ordering {
    cmp_tuple(a, b).then_with(|| a.canon().cmp(&b.canon()))
}
/// The higher of two entries (`a` on a tie).
pub fn higher(a: Option<&J>, b: Option<&J>) -> Option<J> {
    match (a, b) {
        (None, b) => b.cloned(),
        (Some(a), None) => Some(a.clone()),
        (Some(a), Some(b)) => Some(if cmp_entry(a, b) != Ordering::Less { a.clone() } else { b.clone() }),
    }
}
pub fn is_live(e: &J) -> bool {
    !e.get("deleted").is_some_and(|d| d.is_true())
}
/// The entry's version vector: chain key → counter (valid keys, integers 1..2^31−1; anything else is absent).
pub fn vv_of(e: Option<&J>) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    let Some(v) = e.and_then(|e| e.get("vv")).filter(J::is_obj) else { return out };
    for k in v.keys() {
        if !vv_key_ok(&k) {
            continue;
        }
        if let Some(n) = int_range(v.get(&k), 1, MAX_REV) {
            out.insert(k, n);
        }
    }
    out
}
/// The entry's own chain key (its save's "dot", §9.1): among the keys of its vv whose device part is its `by`, the one
/// with the largest counter (ties: the smaller key); None when there is none (an entry of an older draft).
pub fn own_key(e: Option<&J>) -> Option<String> {
    let e = e?;
    let by = tuple_of(e).2;
    if by.is_empty() {
        return None;
    }
    let v = vv_of(Some(e));
    let mut best: Option<&String> = None;
    for (k, n) in &v {
        if key_base(k) == by && best.is_none_or(|b| *n > v[b]) {
            best = Some(k);
        }
    }
    best.cloned()
}
/// B includes A (A's save is in B's history): B.vv[own(A)] ≥ A.vv[own(A)]; for an entry without an own key, B.vv[k] ≥
/// A.vv[k] for every k of A.vv (so an entry without counters is included by every entry).
pub fn includes(b: &J, a: &J) -> bool {
    let vb = vv_of(Some(b));
    let va = vv_of(Some(a));
    if let Some(own) = own_key(Some(a)) {
        return vb.get(&own).copied().unwrap_or(0) >= va[&own];
    }
    va.iter().all(|(k, &n)| vb.get(k).copied().unwrap_or(0) >= n)
}
fn vv_key(e: &J) -> String {
    let o = J::obj();
    for (k, n) in vv_of(Some(e)) {
        o.set(&k, J::int(n));
    }
    o.canon()
}
/// B strictly includes A: B includes A and A does not include B.
fn strictly_includes(b: &J, a: &J) -> bool {
    includes(b, a) && !includes(a, b)
}
/// The same entry (its save), ignoring a `previous` the size rule may have dropped from one copy.
pub fn same_entry(a: &J, b: &J) -> bool {
    strip_previous(a).canon() == strip_previous(b).canon()
}
/// a and b claim the same save (the same own key and counter) — two different entries can only do so when a
/// nonconforming writer reused a dot (a copied device id, a reset counter).
fn same_dot(a: &J, b: &J) -> bool {
    let Some(k) = own_key(Some(a)) else { return false };
    own_key(Some(b)).as_deref() == Some(k.as_str()) && vv_of(Some(a)).get(&k) == vv_of(Some(b)).get(&k)
}
/// §9.7: w carries m's save — w is m, or a different save whose history includes m (the complete merge order: an equal
/// tuple or a reused dot never passes for m).
pub fn carries(w: &J, m: &J) -> bool {
    same_entry(w, m) || (includes(w, m) && !same_dot(w, m))
}
/// §9.2 the input budget: more than 1024 entries in layouts + conflicts, or over 1 MiB canonical.
pub fn over_budget(doc: &J) -> bool {
    let count = |k: &str| doc.get(k).filter(J::is_arr).map_or(0, |a| a.len());
    count("layouts") + count("conflicts") > DOC_BUDGET_ENTRIES || canon_bytes(doc) > DOC_BUDGET_BYTES
}
/// At most 16 counters, keys sorted (§9.1): first the keys of `first` in that order (a save: its own chain key, then
/// the own keys of the entries it incorporates; a read: the entry's own key), then the others, the larger counter
/// first (ties: the smaller key). An entry's own key is never dropped.
fn cap_vv(vv: &BTreeMap<String, i64>, first: &[Option<String>]) -> J {
    let mut kept: Vec<&String> = vv.keys().collect();
    if kept.len() > schema().lim.vv {
        let mut head: Vec<&String> = Vec::new();
        for k in first.iter().flatten() {
            if let Some((key, _)) = vv.get_key_value(k) {
                if !head.contains(&key) {
                    head.push(key);
                }
            }
        }
        let mut rest: Vec<&String> = vv.keys().filter(|k| !head.contains(k)).collect();
        rest.sort_by(|a, b| vv[*b].cmp(&vv[*a]).then(a.cmp(b)));
        head.extend(rest);
        head.truncate(schema().lim.vv);
        kept = head;
    }
    let o = J::obj();
    for k in kept {
        o.set(k, J::int(vv[k]));
    }
    o
}

// ---- documents ---------------------------------------------------------------------------------------------------
#[derive(Clone, Debug)]
pub enum DocRead {
    Ignore,
    Newer(J),
    Ok(J),
}
impl DocRead {
    pub fn kind(&self) -> &'static str {
        match self {
            DocRead::Ignore => "ignore",
            DocRead::Newer(_) => "newer",
            DocRead::Ok(_) => "ok",
        }
    }
}
/// §9.2 read: not JSON data, not an object, not format tinhat-layouts, or over the input budget (1 MiB canonical, 1024
/// entries in `layouts` + `conflicts`) → ignore; v a number > 1 → newer (read-only, latched); v = 1 → ok.
pub fn read_doc(v: Option<&J>) -> DocRead {
    let Some(v) = v.filter(|v| v.is_obj() && is_json_value(Some(v)) && v.is("format", "tinhat-layouts")) else { return DocRead::Ignore };
    let Some(ver) = v.get("v").and_then(|x| x.as_f64()) else { return DocRead::Ignore };
    if over_budget(v) {
        return DocRead::Ignore;
    }
    if ver > 1.0 {
        return DocRead::Newer(v.clone());
    }
    if ver != 1.0 {
        return DocRead::Ignore;
    }
    DocRead::Ok(v.clone())
}

/// §9.1 defaults-filled (a copy): rev / savedAt / by = their tuple values; vv = its valid counters, capped at 16 (the
/// entry's `by` kept first, then the largest), keys sorted; a tombstone keeps only id, rev, savedAt, by, vv, deleted
/// and its unknown keys; a live entry loses a `deleted` that is not `true`, gets its id as name when it has no valid
/// one, the built-in Classic for a form that is not an object, loses a `previous` that is not a snapshot (a snapshot's
/// own `previous` and `quarantine` are dropped) and a `quarantine` that is not an object. Unknown keys stay.
pub fn fill_defaults(e: &J) -> J {
    let o = e.deep();
    let (rev, at, by) = tuple_of(&o);
    o.set("rev", J::int(rev));
    o.set("savedAt", J::int(at));
    o.set("by", J::s(&by));
    o.set("vv", cap_vv(&vv_of(Some(&o)), &[own_key(Some(&o))]));
    if !is_live(&o) {
        o.set("deleted", J::Bool(true));
        for k in ["name", "desktop", "phone", "previous", "quarantine"] {
            o.del(k);
        }
        return o;
    }
    if o.has("deleted") {
        o.del("deleted");
    }
    if !name_ok(o.get("name").as_ref()) {
        o.set("name", o.get("id").unwrap_or_default());
    }
    if !o.get("desktop").is_some_and(|d| d.is_obj()) {
        o.set("desktop", classic_desktop());
    }
    if !o.get("phone").is_some_and(|d| d.is_obj()) {
        o.set("phone", classic_phone());
    }
    if o.has("previous") {
        let p = o.get("previous").unwrap_or_default();
        if !p.is_obj() || !p.get("desktop").is_some_and(|d| d.is_obj()) || !p.get("phone").is_some_and(|d| d.is_obj()) {
            o.del("previous");
        } else {
            p.del("previous");
            p.del("quarantine");
        }
    }
    if o.has("quarantine") && !o.get("quarantine").is_some_and(|q| q.is_obj()) {
        o.del("quarantine");
    }
    o
}

/// §9.3 the per-layout reduction of a set of entries of one id (defaults-filled): identical entries once; per version
/// vector the highest in merge order; w = the highest of all; the conflicts = every other remaining entry that no
/// other remaining entry strictly includes and that w does not include, without `previous`. Returns [w, …conflicts]
/// (conflicts in merge order, highest first). Order-independent.
pub fn reduce(entries: &[J]) -> Vec<J> {
    if entries.is_empty() {
        return Vec::new();
    }
    // identical entries once (the reference keeps the last object of equal text: equal content either way)
    let mut by_text: Vec<(String, J)> = Vec::new();
    for e in entries {
        let t = e.canon();
        match by_text.iter_mut().find(|(x, _)| *x == t) {
            Some(slot) => slot.1 = e.clone(),
            None => by_text.push((t, e.clone())),
        }
    }
    let mut by_vv: Vec<(String, J)> = Vec::new();
    for (_, e) in &by_text {
        let k = vv_key(e);
        match by_vv.iter_mut().find(|(x, _)| *x == k) {
            Some(slot) => slot.1 = higher(Some(&slot.1), Some(e)).expect("an entry"),
            None => by_vv.push((k, e.clone())),
        }
    }
    let rest: Vec<J> = by_vv.into_iter().map(|(_, e)| e).collect();
    let mut wi = 0usize;
    for (i, e) in rest.iter().enumerate() {
        if cmp_entry(e, &rest[wi]) == Ordering::Greater {
            wi = i;
        }
    }
    let w = rest[wi].clone();
    let mut conflicts: Vec<J> = rest
        .iter()
        .enumerate()
        .filter(|(i, e)| *i != wi && !includes(&w, e) && !rest.iter().enumerate().any(|(j, d)| j != *i && strictly_includes(d, e)))
        .map(|(_, e)| e.clone())
        .collect();
    conflicts.sort_by(|a, b| cmp_entry(b, a));
    let mut out = vec![w];
    out.extend(conflicts.iter().map(strip_previous));
    out
}
fn strip_previous(e: &J) -> J {
    if !e.has("previous") {
        return e.clone();
    }
    let c = e.deep();
    c.del("previous");
    c
}

/// `known`: per layout id what this side knows — a list of entries (an array) or one entry (an object).
pub type Known = BTreeMap<String, J>;

/// The document's entries per id (`layouts` ∪ `conflicts`, valid ids, defaults-filled), reduced; ids in code-point
/// order.
pub fn entries_of(doc: Option<&J>) -> BTreeMap<String, Vec<J>> {
    let mut acc: BTreeMap<String, Vec<J>> = BTreeMap::new();
    let Some(doc) = doc.filter(|d| truthy(Some(d))) else { return BTreeMap::new() };
    for key in ["layouts", "conflicts"] {
        let Some(ls) = doc.get(key).filter(J::is_arr) else { continue };
        for e in ls.items() {
            let Some(id) = entry_id(Some(&e)) else { continue };
            acc.entry(id).or_default().push(fill_defaults(&e));
        }
    }
    acc.into_iter().map(|(id, list)| (id, reduce(&list))).collect()
}
/// The current entry (the winner) of an id in a document, or None.
pub fn raw_entry(doc: Option<&J>, id: &str) -> Option<J> {
    entries_of(doc).get(id).and_then(|l| l.first().cloned())
}

/// `known[k]` as lists of defaults-filled entries whose own id is valid (keyed by that id).
fn known_entries(known: &Known) -> BTreeMap<String, Vec<J>> {
    let mut out: BTreeMap<String, Vec<J>> = BTreeMap::new();
    for v in known.values() {
        let list = if v.is_arr() {
            v.items()
        } else if v.is_obj() {
            vec![v.clone()]
        } else {
            vec![]
        };
        for e in list {
            let Some(id) = entry_id(Some(&e)) else { continue };
            if !is_json_value(Some(&e)) {
                continue; // §1.4: a value outside the input domain is not an entry
            }
            out.entry(id).or_default().push(fill_defaults(&e));
        }
    }
    out
}

/// merge(raw, known, justSaved) — per id reduce(raw's entries ∪ known's); `layouts` = the winners sorted by id,
/// `conflicts` (only when not empty) = the other entries sorted by id, then merge order; raw's unknown top-level keys
/// kept; then the size rule (drop `previous` from the live winner with the oldest savedAt, `justSaved` last);
/// `tooLarge` when even that is not enough.
pub fn merge(raw: Option<&J>, known: &Known, just_saved: Option<&str>) -> (J, bool) {
    let lim = &schema().lim;
    let raw_obj = raw.filter(|r| r.is_obj() && is_json_value(Some(r))); // §1.4: anything else reads as no document
    let mut acc: BTreeMap<String, Vec<J>> = BTreeMap::new();
    for (id, list) in entries_of(raw_obj) {
        acc.entry(id).or_default().extend(list);
    }
    for (id, list) in known_entries(known) {
        acc.entry(id).or_default().extend(list);
    }
    let mut layouts: Vec<J> = Vec::new();
    let mut conflicts: Vec<J> = Vec::new();
    for list in acc.values() {
        let r = reduce(list);
        layouts.push(r[0].deep());
        conflicts.extend(r[1..].iter().map(J::deep));
    }
    let doc = J::obj();
    if let Some(raw) = raw_obj {
        for k in raw.keys() {
            if !DOC_KEYS.contains(&k.as_str()) {
                doc.set(&k, raw.get(&k).map(|v| v.deep()).unwrap_or_default());
            }
        }
    }
    doc.set("format", J::s("tinhat-layouts"));
    doc.set("v", J::int(1));
    doc.set("layouts", J::arr(layouts.clone()));
    if !conflicts.is_empty() {
        doc.set("conflicts", J::arr(conflicts));
    }
    let id_of = |x: &J| x.get_str("id").map(|s| s.to_string()).unwrap_or_default();
    while canon_bytes(&doc) > lim.doc_bytes {
        let mut cands: Vec<&J> = layouts.iter().filter(|x| is_live(x) && x.has("previous") && Some(id_of(x).as_str()) != just_saved).collect();
        cands.sort_by(|a, b| tuple_of(a).1.cmp(&tuple_of(b).1).then(id_of(a).cmp(&id_of(b))));
        let victim = match cands.first() {
            Some(v) => Some((*v).clone()),
            None => layouts.iter().find(|x| Some(id_of(x).as_str()) == just_saved && is_live(x) && x.has("previous")).cloned(),
        };
        match victim {
            Some(v) => v.del("previous"),
            None => return (doc, true),
        }
    }
    (doc, false)
}

// ---- the projection (§9.4) ---------------------------------------------------------------------------------------
#[derive(Clone, Debug)]
pub struct ConflictInfo {
    pub rev: i64,
    pub saved_at: i64,
    pub by: String,
    pub deleted: bool,
}

#[derive(Clone, Debug)]
pub struct Projection {
    pub read: &'static str,
    /// the live layout ids, in id order
    pub layouts: Vec<String>,
    /// the layout this device renders: the asked id if live, else main, else the first live one, else Classic
    pub active: String,
    pub name: String,
    pub desktop: J,
    pub phone: J,
    pub notes_desktop: Vec<String>,
    pub notes_phone: Vec<String>,
    /// the active layout's concurrent losers (merge order, highest first)
    pub conflicts: Vec<ConflictInfo>,
    /// the active layout carries a quarantined original (a form this version had to replace)
    pub quarantine: bool,
}

impl Projection {
    pub fn to_json(&self) -> J {
        let conflicts = J::arr(
            self.conflicts
                .iter()
                .map(|c| mk(vec![("rev", J::int(c.rev)), ("savedAt", J::int(c.saved_at)), ("by", J::s(&c.by)), ("deleted", J::Bool(c.deleted))]))
                .collect(),
        );
        mk(vec![
            ("read", J::s(self.read)),
            ("layouts", J::strs(&self.layouts)),
            ("active", J::s(&self.active)),
            ("name", J::s(&self.name)),
            ("desktop", self.desktop.clone()),
            ("phone", self.phone.clone()),
            ("notes", mk(vec![("desktop", J::strs(&self.notes_desktop)), ("phone", J::strs(&self.notes_phone))])),
            ("conflicts", conflicts),
            ("quarantine", J::Bool(self.quarantine)),
        ])
    }
}

/// project(doc, activeId) — what a device renders from an account-data value (LAYOUT.md §9.4). Never panics.
pub fn project(v: Option<&J>, active_id: Option<&str>) -> Projection {
    let r = read_doc(v);
    let e = match &r {
        DocRead::Ignore => BTreeMap::new(),
        DocRead::Newer(d) | DocRead::Ok(d) => entries_of(Some(d)),
    };
    let live: Vec<String> = e.iter().filter(|(_, l)| is_live(&l[0])).map(|(k, _)| k.clone()).collect();
    let active = match active_id {
        Some(a) if live.iter().any(|x| x == a) => Some(a.to_string()),
        _ => {
            if live.iter().any(|x| x == "main") {
                Some("main".to_string())
            } else {
                live.first().cloned()
            }
        }
    };
    let list = active.as_ref().and_then(|a| e.get(a).cloned());
    let entry = list.as_ref().and_then(|l| l.first().cloned());
    let d = repair_desktop(Some(&entry.as_ref().and_then(|x| x.get("desktop")).unwrap_or(J::Null)));
    let p = repair_phone(Some(&entry.as_ref().and_then(|x| x.get("phone")).unwrap_or(J::Null)));
    let nm = entry.as_ref().and_then(|x| x.get("name"));
    let active = active.unwrap_or_else(|| "main".to_string());
    let name = match (&entry, nm) {
        (Some(_), Some(n)) if name_ok(Some(&n)) => n.as_str().unwrap_or_default().to_string(),
        (None, _) if active == "main" => "Main".to_string(),
        _ => active.clone(),
    };
    let conflicts = list
        .as_ref()
        .map(|l| {
            l[1..]
                .iter()
                .map(|c| {
                    let (rev, saved_at, by) = tuple_of(c);
                    ConflictInfo { rev, saved_at, by, deleted: !is_live(c) }
                })
                .collect()
        })
        .unwrap_or_default();
    let quarantine = entry.as_ref().is_some_and(|x| x.get("quarantine").is_some_and(|q| q.is_obj()));
    Projection {
        read: r.kind(),
        layouts: live,
        active,
        name,
        desktop: if entry.is_some() { d.form } else { classic_desktop() },
        phone: if entry.is_some() { p.form } else { classic_phone() },
        notes_desktop: if entry.is_some() { d.notes } else { vec![] },
        notes_phone: if entry.is_some() { p.notes } else { vec![] },
        conflicts,
        quarantine,
    }
}

// ---- save --------------------------------------------------------------------------------------------------------
/// A save request (LAYOUT.md §9.5); the fields hold whatever the caller passed (validated by save()).
#[derive(Clone, Debug, Default)]
pub struct SaveReq {
    pub id: Option<J>,
    /// {name?, desktop, phone, discardQuarantine?} (a live layout) or {deleted: true}
    pub edit: Option<J>,
    /// the entry the edit session started from (None for a new layout)
    pub start: Option<J>,
    /// Restore / Dismiss: the concurrent loser whose version vector this save incorporates
    pub also: Option<J>,
    pub now: Option<J>,
    pub by: Option<J>,
    /// this device's save counter after this save (every successful save increments it)
    pub seq: Option<J>,
    /// the version-vector key this save counts under (absent = `by`): `by`, or `by#<n>` for a later chain (§9.6)
    pub key: Option<J>,
}

#[derive(Clone, Debug)]
pub enum SaveResult {
    Saved { entry: J, doc: J },
    Noop,
    Err(String),
}

impl SaveResult {
    /// The vector / controller form: `{entry, doc}` | `{noop: true}` | `{error}`.
    pub fn to_json(&self) -> J {
        match self {
            SaveResult::Saved { entry, doc } => mk(vec![("entry", entry.clone()), ("doc", doc.clone())]),
            SaveResult::Noop => mk(vec![("noop", J::Bool(true))]),
            SaveResult::Err(e) => mk(vec![("error", J::s(e))]),
        }
    }
}

/// JavaScript truthiness of a value.
fn truthy(v: Option<&J>) -> bool {
    match v {
        None | Some(J::Null) => false,
        Some(J::Bool(b)) => *b,
        Some(J::Num(x)) => *x != 0.0 && !x.is_nan(),
        Some(J::Str(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

fn replaced(notes: &[String]) -> bool {
    notes.first().is_some_and(|n| n.starts_with("replaced-by-classic"))
}

/// reduce(raw's entries of id ∪ known[id] ∪ extra).
fn reduce_with(raw: Option<&J>, known: &Known, id: &str, extra: &[J]) -> Vec<J> {
    let mut list: Vec<J> = entries_of(raw).remove(id).unwrap_or_default();
    list.extend(known_entries(known).remove(id).unwrap_or_default());
    list.extend(extra.iter().cloned());
    reduce(&list)
}

/// save(raw, known, req) → a NEW entry for req.id that wins its own merge + the document to put.
pub fn save(raw: Option<&J>, known: &Known, req: &SaveReq) -> SaveResult {
    let rd = if truthy(raw) { Some(read_doc(raw)) } else { None };
    if matches!(rd, Some(DocRead::Newer(_))) {
        return SaveResult::Err("bad-save: newer format".into());
    }
    let raw_doc = match &rd {
        Some(DocRead::Ok(d)) => Some(d.clone()),
        _ => None,
    };
    let bad = || SaveResult::Err("bad-op: bad request".into());
    // §1.4 / §9.5: the request's shape — start and also are entries: absent / null, or JSON objects
    let entry_arg = |e: &Option<J>| -> Result<Option<J>, ()> {
        match e {
            None | Some(J::Null) => Ok(None),
            Some(x) if x.is_obj() && is_json_value(Some(x)) => Ok(Some(x.clone())),
            Some(_) => Err(()),
        }
    };
    let (Ok(start), Ok(also)) = (entry_arg(&req.start), entry_arg(&req.also)) else { return bad() };
    let (start, also) = (start.as_ref(), also.as_ref());
    let (Some(id), Some(edit)) = (
        req.id.as_ref().and_then(J::as_str).filter(|i| is_id_syntax(i)),
        req.edit.as_ref().filter(|e| e.is_obj() && is_json_value(Some(e))),
    ) else {
        return bad();
    };
    let Some(now) = int_range(req.now.clone(), 0, MAX_SAVED_AT) else { return bad() };
    if !device_ok(req.by.as_ref()) {
        return bad();
    }
    let by = req.by.as_ref().and_then(J::as_str).unwrap_or_default();
    let Some(seq) = req.seq.as_ref().filter(|s| s.is_int()).and_then(|s| s.as_f64()).filter(|s| *s >= 1.0) else { return bad() };
    let vkey = match &req.key {
        None => J::s(by),
        Some(k) => k.clone(),
    };
    let Some(vkey) = vkey.as_str().map(str::to_string) else { return bad() };
    if !vv_key_ok(&vkey) || key_base(&vkey) != by {
        return bad();
    }
    if vkey != by {
        let n: f64 = vkey[by.len() + 1..].parse().unwrap_or(f64::INFINITY);
        if n > MAX_REV as f64 {
            return bad();
        }
    }
    if seq > MAX_REV as f64 {
        return SaveResult::Err("bad-save: exhausted".into());
    }
    let seq = seq as i64;
    let del = edit.get("deleted").is_some_and(|d| d.is_true());
    if !del && (!edit.get("desktop").is_some_and(|d| d.is_obj()) || !edit.get("phone").is_some_and(|d| d.is_obj())) {
        return bad();
    }
    let state = reduce_with(raw_doc.as_ref(), known, id, &[]);
    let base = state.first().cloned();
    if base.as_ref().is_some_and(|b| tuple_of(b).0 >= MAX_REV) {
        return SaveResult::Err("bad-save: exhausted".into());
    }
    let discard = edit.get("discardQuarantine").is_some_and(|d| d.is_true());
    let mut name = J::s(id);
    if del {
        if !base.as_ref().is_some_and(is_live) && also.is_none() {
            return SaveResult::Noop;
        }
    } else {
        name = if name_ok(edit.get("name").as_ref()) {
            edit.get("name").unwrap_or_default()
        } else if let Some(b) = base.as_ref().filter(|b| is_live(b) && name_ok(b.get("name").as_ref())) {
            b.get("name").unwrap_or_default()
        } else {
            J::s(id)
        };
        if also.is_none() && !discard {
            if let Some(start) = start.filter(|s| is_live(s)) {
                let same = canon_opt(start.get("desktop").as_ref()) == canon_opt(edit.get("desktop").as_ref())
                    && canon_opt(start.get("phone").as_ref()) == canon_opt(edit.get("phone").as_ref())
                    && start.get("name").is_some_and(|n| n.as_str().is_some() && n.as_str() == name.as_str());
                if same {
                    return SaveResult::Noop;
                }
            }
        }
        if !base.as_ref().is_some_and(is_live) {
            let (m, _) = merge(raw_doc.as_ref(), known, None);
            let count = m.get("layouts").map(|l| l.items()).unwrap_or_default().iter().filter(|x| is_live(x) && !x.is("id", id)).count();
            if count >= schema().lim.layouts {
                return SaveResult::Err("bad-save: too many layouts".into());
            }
        }
    }
    // version vector: the component-wise max of the incorporated entries, own counter = seq, at most 16 keys
    let mut vv: BTreeMap<String, i64> = BTreeMap::new();
    for src in [start, also] {
        for (k, n) in vv_of(src) {
            let cur = vv.get(&k).copied().unwrap_or(0);
            vv.insert(k, cur.max(n));
        }
    }
    vv.insert(vkey.clone(), seq);
    let e = J::obj();
    if !del {
        if let Some(b) = &base {
            for k in b.keys() {
                if !ENTRY_KEYS.contains(&k.as_str()) {
                    e.set(&k, b.get(&k).map(|v| v.deep()).unwrap_or_default());
                }
            }
        }
    }
    e.set("id", J::s(id));
    e.set("rev", J::int(base.as_ref().map_or(1, |b| tuple_of(b).0 + 1)));
    e.set("savedAt", J::int(now));
    e.set("by", J::s(by));
    e.set("vv", cap_vv(&vv, &[Some(vkey.clone()), own_key(also), own_key(start)]));
    if del {
        e.set("deleted", J::Bool(true));
    } else {
        e.set("name", name);
        e.set("desktop", edit.get("desktop").map(|d| d.deep()).unwrap_or_default());
        e.set("phone", edit.get("phone").map(|d| d.deep()).unwrap_or_default());
        if let Some(b) = base.as_ref().filter(|b| is_live(b)) {
            let differ = canon_opt(b.get("desktop").as_ref()) != canon_opt(e.get("desktop").as_ref())
                || canon_opt(b.get("phone").as_ref()) != canon_opt(e.get("phone").as_ref());
            if differ {
                let p = b.deep();
                p.del("previous");
                p.del("quarantine");
                e.set("previous", p);
            } else if let Some(prev) = b.get("previous").filter(J::is_obj) {
                e.set("previous", prev.deep());
            }
            // quarantine: the originals this version could not read, kept until explicitly discarded (§9.5)
            if !discard {
                let q = b.get("quarantine").filter(J::is_obj).map(|q| q.deep()).unwrap_or_else(J::obj);
                let bd = b.get("desktop").unwrap_or(J::Null);
                if !q.has("desktop") && replaced(&repair_desktop(Some(&bd)).notes) {
                    q.set("desktop", bd.deep());
                }
                let bp = b.get("phone").unwrap_or(J::Null);
                if !q.has("phone") && replaced(&repair_phone(Some(&bp)).notes) {
                    q.set("phone", bp.deep());
                }
                if q.nkeys() > 0 {
                    e.set("quarantine", q);
                }
            }
        }
    }
    let mut k2 = known.clone();
    let mut with = state.clone();
    with.push(e.clone());
    k2.insert(id.to_string(), J::arr(reduce(&with)));
    let (doc, too_large) = merge(raw_doc.as_ref(), &k2, Some(id));
    if too_large {
        return SaveResult::Err("bad-layout: too large".into());
    }
    SaveResult::Saved { entry: e, doc }
}

// ---- the device protocol (§9.6) ---------------------------------------------------------------------------------
/// A layout id as a step names it: a string, or absent (the reference's `undefined`: its object maps coerce it to
/// "undefined", its session Map keeps it apart from every string, and no document entry has it).
pub type StepId = Option<String>;
fn obj_key(id: &StepId) -> String {
    id.clone().unwrap_or_else(|| "undefined".into())
}

#[derive(Clone, Debug)]
struct Chain {
    key: String,
    last: i64,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub name: String,
    pub counter: i64,
    pub known: Known,
    pub own: BTreeMap<String, J>,
    pub lost: BTreeMap<String, J>,
    pub raw: Option<J>,
    /// the newer-format latch: the highest `v` ever seen; > 1 = read-only for good (never cleared by a v1 event)
    pub max_v: f64,
    sessions: Vec<(StepId, Option<J>)>,
    pub want_put: bool,
    pub last_saved: Option<String>,
    /// per layout: the chain key this device's saves count under and the counter of its last save there (§9.6)
    chains: BTreeMap<String, Chain>,
}

impl Device {
    pub fn new(name: &str) -> Device {
        Device {
            name: name.to_string(),
            counter: 0,
            known: Known::new(),
            own: BTreeMap::new(),
            lost: BTreeMap::new(),
            raw: None,
            max_v: 1.0,
            sessions: Vec::new(),
            want_put: false,
            last_saved: None,
            chains: BTreeMap::new(),
        }
    }
    pub fn read_only(&self) -> bool {
        self.max_v > 1.0
    }
    fn session(&self, id: &StepId) -> Option<&Option<J>> {
        self.sessions.iter().find(|(k, _)| k == id).map(|(_, v)| v)
    }
    fn set_session(&mut self, id: &StepId, v: Option<J>) {
        match self.sessions.iter_mut().find(|(k, _)| k == id) {
            Some(slot) => slot.1 = v,
            None => self.sessions.push((id.clone(), v)),
        }
    }
    fn del_session(&mut self, id: &StepId) {
        self.sessions.retain(|(k, _)| k != id);
    }
    /// The vv key of save `seq` of `id` (§9.6): the chain's key while the save builds on this device's last save of the
    /// layout (start or also carries its counter), else a new chain `<name>#<seq>`.
    fn key_for(&self, id: &StepId, start: Option<&J>, also: Option<&J>, seq: i64) -> String {
        let Some(c) = self.chains.get(&obj_key(id)) else { return self.name.clone() };
        let inc = vv_of(start).get(&c.key).copied().unwrap_or(0).max(vv_of(also).get(&c.key).copied().unwrap_or(0));
        if inc >= c.last {
            c.key.clone()
        } else {
            format!("{}#{seq}", self.name)
        }
    }
    fn raw_for_save(&self) -> Option<&J> {
        if self.max_v == 1.0 {
            self.raw.as_ref()
        } else {
            None
        }
    }
    /// The reduction of what this device knows of `id` (raw only while not latched read-only).
    pub fn state(&self, id: &StepId) -> Vec<J> {
        match id {
            None => Vec::new(),
            Some(id) => reduce_with(self.raw.as_ref().filter(|_| self.max_v == 1.0), &self.known, id, &[]),
        }
    }
    pub fn best(&self, id: &StepId) -> Option<J> {
        self.state(id).first().cloned()
    }
    pub fn begin(&mut self, id: &StepId) -> String {
        if self.session(id).is_some() {
            return "open".into();
        }
        let b = self.best(id);
        self.set_session(id, b);
        "begun".into()
    }
    fn commit(&mut self, id: &StepId, r: &SaveResult, seq: i64, key: &str) -> String {
        match r {
            SaveResult::Err(e) => e.clone(),
            SaveResult::Noop => "noop".into(),
            SaveResult::Saved { entry, .. } => {
                let k = obj_key(id);
                self.counter = seq;
                self.chains.insert(k.clone(), Chain { key: key.to_string(), last: seq });
                let mut st = self.state(id);
                st.push(entry.clone());
                self.known.insert(k.clone(), J::arr(reduce(&st)));
                self.own.insert(k.clone(), entry.clone());
                self.want_put = true;
                self.last_saved = Some(k);
                "saved".into()
            }
        }
    }
    pub fn save(&mut self, id: &StepId, edit: Option<J>, now: Option<J>) -> String {
        if self.read_only() {
            return "read-only".into();
        }
        let start = match self.session(id) {
            Some(s) => s.clone(),
            None => self.best(id),
        };
        self.del_session(id);
        let seq = self.counter + 1;
        let key = self.key_for(id, start.as_ref(), None, seq);
        let req = SaveReq {
            id: id.as_ref().map(|s| J::s(s)),
            edit,
            start: start.clone(),
            also: None,
            now,
            by: Some(J::s(&self.name)),
            seq: Some(J::int(seq)),
            key: Some(J::s(&key)),
        };
        let r = save(self.raw_for_save(), &self.known, &req);
        if matches!(r, SaveResult::Err(_)) {
            self.set_session(id, start);
        }
        self.commit(id, &r, seq, &key)
    }
    /// Restore (`take`: the conflict's version becomes current) or Dismiss (the current version stays) of conflict
    /// number `pick` (0 = the highest in merge order) of `id` — a save that incorporates both (§9.6).
    pub fn resolve(&mut self, id: &StepId, now: Option<J>, take: bool, pick: f64) -> String {
        if self.read_only() {
            return "read-only".into();
        }
        let st = self.state(id);
        let m = if pick >= 0.0 && pick.fract() == 0.0 && pick < st.len() as f64 { st.get(1 + pick as usize).cloned() } else { None };
        let Some(m) = m else { return "none".into() };
        if self.session(id).is_some() {
            return "none".into();
        }
        let w = st.first().cloned();
        let src = if take { Some(m.clone()) } else { w.clone() };
        let edit = J::obj();
        match src.filter(is_live) {
            None => edit.set("deleted", J::Bool(true)),
            Some(src) => {
                if let Some(n) = src.get("name").filter(|n| n.is_str()) {
                    edit.set("name", n);
                }
                edit.set("desktop", src.get("desktop").map(|d| d.deep()).unwrap_or_default());
                edit.set("phone", src.get("phone").map(|d| d.deep()).unwrap_or_default());
            }
        }
        let seq = self.counter + 1;
        let key = self.key_for(id, w.as_ref(), Some(&m), seq);
        let req = SaveReq {
            id: id.as_ref().map(|s| J::s(s)),
            edit: Some(edit),
            start: w,
            also: Some(m.clone()),
            now,
            by: Some(J::s(&self.name)),
            seq: Some(J::int(seq)),
            key: Some(J::s(&key)),
        };
        let r = save(self.raw_for_save(), &self.known, &req);
        let res = self.commit(id, &r, seq, &key);
        if res == "saved" {
            let k = obj_key(id);
            if let Some(l) = self.lost.get(&k).cloned() {
                let covering = match &r {
                    SaveResult::Saved { entry, .. } => entry.clone(),
                    _ => m.clone(),
                };
                if includes(&covering, &l) {
                    self.lost.remove(&k);
                }
            }
        }
        res
    }
    /// The document this device would put now (merge of what it has seen and what it knows).
    pub fn pending(&self) -> J {
        merge(self.raw_for_save(), &self.known, self.last_saved.as_deref()).0
    }
    pub fn put(&mut self, server: &mut Option<J>) -> String {
        if !self.want_put || self.read_only() {
            return "idle".into();
        }
        *server = Some(self.pending());
        self.want_put = false;
        self.last_saved = None;
        "put".into()
    }
    /// An incoming account-data document (every event, the echo of this device's own put included).
    pub fn receive(&mut self, d: Option<&J>) -> String {
        let doc = match read_doc(d) {
            DocRead::Ignore => return "ignored".into(),
            DocRead::Newer(doc) | DocRead::Ok(doc) => doc,
        };
        let ver = doc.get("v").and_then(|v| v.as_f64()).unwrap_or(1.0);
        if ver > 1.0 {
            // the newer-format latch: never cleared by a v1 event (a delayed or reordered one cannot re-open writes)
            if ver > self.max_v {
                self.max_v = ver;
            }
            self.raw = Some(doc.deep());
            return "read-only".into();
        }
        if self.read_only() {
            self.raw = Some(doc.deep());
            return "read-only".into();
        }
        let doc = doc.deep();
        // known := per id reduce(known ∪ D); a union over the input budget (§9.2) is not taken: the device keeps what it
        // knows, so `known` never outgrows what any device can read
        let mut next = self.known.clone();
        for (id, list) in entries_of(Some(&doc)) {
            let mut all = known_entries(&self.known).remove(&id).unwrap_or_default();
            all.extend(list);
            next.insert(id, J::arr(reduce(&all)));
        }
        let merged = merge(Some(&doc), &next, None).0;
        if over_budget(&merged) {
            return "ignored".into();
        }
        self.raw = Some(doc.clone());
        self.known = next;
        let mut reports: Vec<String> = Vec::new();
        let ids: Vec<String> = self.own.keys().cloned().collect();
        for id in ids {
            let m = self.own.get(&id).cloned().expect("own entry");
            let Some(w) = self.best(&Some(id.clone())) else { continue };
            if same_entry(&w, &m) {
                continue; // still the current version: pending
            }
            if !carries(&w, &m) {
                self.lost.insert(id.clone(), m);
                reports.push(format!("lost {id}"));
            }
            self.own.remove(&id);
        }
        // a lost save that the current version now carries was resolved (here or on another device): clear the hint
        let lost_ids: Vec<String> = self.lost.keys().cloned().collect();
        for id in lost_ids {
            if let Some(w) = self.best(&Some(id.clone())) {
                if carries(&w, &self.lost[&id]) {
                    self.lost.remove(&id);
                }
            }
        }
        if merged.canon() != doc.canon() {
            self.want_put = true;
        }
        if reports.is_empty() {
            "ok".into()
        } else {
            reports.join(",")
        }
    }
    /// Host plumbing (the apps' layoutDoc): the start a session begun on `id` holds (None = no session). The desktop
    /// keeps its device as text between steps, so it carries the begun sessions across (`set_session_of`).
    pub fn session_of(&self, id: &StepId) -> Option<Option<J>> {
        self.session(id).cloned()
    }
    pub fn set_session_of(&mut self, id: &StepId, start: Option<J>) {
        self.set_session(id, start)
    }
    /// Cancel of an edit session: the begun start is forgotten (the next save builds on the best entry again).
    pub fn end_session(&mut self, id: &StepId) {
        self.del_session(id)
    }
    /// The device state as the apps persist it per account (EDITMODE-UI §16; the web's layoutDoc.ts `persist()`, the
    /// same keys): `{v: 1, name, counter, chains, known, own, lost, maxV, raw, wantPut, lastSaved}`. Not part of the
    /// engine port — host plumbing (the desktop's layout_store.rs writes it to the app's data dir).
    pub fn to_store(&self) -> J {
        let map = |k: &BTreeMap<String, J>| {
            let x = J::obj();
            for (id, e) in k {
                x.set(id, e.deep());
            }
            x
        };
        let chains = J::obj();
        for (id, c) in &self.chains {
            chains.set(id, mk(vec![("key", J::s(&c.key)), ("last", J::int(c.last))]));
        }
        mk(vec![
            ("v", J::int(1)),
            ("name", J::s(&self.name)),
            ("counter", J::int(self.counter)),
            ("chains", chains),
            ("known", map(&self.known)),
            ("own", map(&self.own)),
            ("lost", map(&self.lost)),
            ("maxV", J::Num(self.max_v)),
            ("raw", self.raw.as_ref().map(J::deep).unwrap_or(J::Null)),
            ("wantPut", J::Bool(self.want_put)),
            ("lastSaved", self.last_saved.as_ref().map(|s| J::s(s)).unwrap_or(J::Null)),
        ])
    }
    /// A device from its persisted state (`to_store`), read leniently like the web's layoutDoc.ts `load()`; None when
    /// the value is not a v1 state with a name.
    pub fn from_store(st: &J) -> Option<Device> {
        if !st.is_obj() || st.get("v").and_then(|v| v.int_val()) != Some(1) {
            return None;
        }
        let name = st.get("name").and_then(|n| n.as_str().map(str::to_string))?;
        let mut d = Device::new(&name);
        d.counter = st.get("counter").and_then(|n| n.int_val()).filter(|n| *n >= 0).unwrap_or(0);
        if let Some(ch) = st.get("chains").filter(J::is_obj) {
            for id in ch.keys() {
                let Some(c) = ch.get(&id).filter(J::is_obj) else { continue };
                if let (Some(key), Some(last)) = (c.get("key").and_then(|k| k.as_str().map(str::to_string)), c.get("last").and_then(|l| l.int_val())) {
                    d.chains.insert(id, Chain { key, last });
                }
            }
        }
        if let Some(kn) = st.get("known").filter(J::is_obj) {
            for id in kn.keys() {
                if let Some(list) = kn.get(&id).filter(J::is_arr) {
                    d.known.insert(id, J::arr(list.items().into_iter().filter(J::is_obj).map(|e| e.deep()).collect()));
                }
            }
        }
        for (key, slot) in [("own", &mut d.own), ("lost", &mut d.lost)] {
            if let Some(o) = st.get(key).filter(J::is_obj) {
                for id in o.keys() {
                    if let Some(e) = o.get(&id).filter(J::is_obj) {
                        slot.insert(id, e.deep());
                    }
                }
            }
        }
        d.max_v = st.get("maxV").and_then(|v| v.int_val()).filter(|v| *v >= 1).map(|v| v as f64).unwrap_or(1.0);
        d.raw = st.get("raw").filter(J::is_obj).map(|r| r.deep());
        d.want_put = st.get("wantPut").is_some_and(|w| w.is_true());
        d.last_saved = st.get("lastSaved").and_then(|l| l.as_str().map(str::to_string));
        Some(d)
    }
    pub fn state_json(&self) -> J {
        let map = |k: &BTreeMap<String, J>| {
            let x = J::obj();
            for (id, e) in k {
                x.set(id, e.deep());
            }
            x
        };
        let chains = J::obj();
        for (id, c) in &self.chains {
            chains.set(id, mk(vec![("key", J::s(&c.key)), ("last", J::int(c.last))]));
        }
        mk(vec![
            ("counter", J::int(self.counter)),
            ("chains", chains),
            ("known", map(&self.known)),
            ("own", map(&self.own)),
            ("lost", map(&self.lost)),
            ("maxV", J::Num(self.max_v)),
            ("readOnly", J::Bool(self.read_only())),
            ("wantPut", J::Bool(self.want_put)),
        ])
    }
}

/// Replay a scripted sync scenario (the `sync` vectors, LAYOUT.md §10.1).
pub fn run_sync(v: &J) -> J {
    let names: Vec<String> = v.get("devices").map(|d| d.items()).unwrap_or_default().iter().map(|x| js_str(Some(x))).collect();
    let mut devs: Vec<Device> = Vec::new();
    for n in &names {
        if !devs.iter().any(|d| d.name == *n) {
            devs.push(Device::new(n));
        }
    }
    // the server document as the reference holds it: null at first, then whatever was put (None = undefined, which
    // a serverPut without a `doc` leaves and the output then omits)
    let mut server: Option<J> = Some(v.get("server").filter(J::is_obj).map(|s| s.deep()).unwrap_or(J::Null));
    let mut log: Vec<String> = Vec::new();
    let steps = v.get("steps").map(|s| s.items()).unwrap_or_default();
    for (i, s) in steps.iter().enumerate() {
        let ev = s.get("event");
        let dev_name = s.get("device");
        let d = dev_name.as_ref().and_then(J::as_str).and_then(|n| devs.iter().position(|d| d.name == n));
        let id: StepId = s.get_str("id").map(|x| x.to_string());
        let now = s.get("now");
        let evs = ev.as_ref().and_then(J::as_str).unwrap_or("");
        let needs_device = matches!(evs, "begin" | "save" | "delete" | "restore" | "dismiss" | "put" | "deliver" | "incoming");
        let res: String = match (evs, d) {
            (_, None) if needs_device => "bad-step".into(), // the reference throws (no such device)
            ("begin", Some(d)) => devs[d].begin(&id),
            ("save", Some(d)) => devs[d].save(&id, s.get("edit"), now),
            ("delete", Some(d)) => devs[d].save(&id, Some(mk(vec![("deleted", J::Bool(true))])), now),
            ("restore", Some(d)) | ("dismiss", Some(d)) => {
                let pick = s.get("pick").filter(|p| p.is_int()).and_then(|p| p.as_f64()).unwrap_or(0.0);
                devs[d].resolve(&id, now, evs == "restore", pick)
            }
            ("put", Some(d)) => devs[d].put(&mut server),
            ("deliver", Some(d)) => {
                let doc = server.clone();
                devs[d].receive(doc.as_ref())
            }
            ("incoming", Some(d)) => devs[d].receive(s.get("doc").as_ref()),
            ("serverPut", _) => {
                server = s.get("doc").map(|x| x.deep());
                "set".into()
            }
            _ => "bad-step".into(),
        };
        let dev_txt = match &dev_name {
            None | Some(J::Null) => "-".to_string(),
            Some(x) => js_str(Some(x)),
        };
        log.push(format!("{i} {dev_txt} {}: {res}", js_str(ev.as_ref())));
    }
    let ds = J::obj();
    let mut sorted = names.clone();
    sorted.sort();
    for n in &sorted {
        if let Some(d) = devs.iter().find(|d| d.name == *n) {
            ds.set(n, d.state_json());
        }
    }
    let out = mk(vec![("log", J::strs(&log)), ("devices", ds)]);
    if let Some(sv) = server {
        out.set("server", sv);
    }
    out
}

/// The known map of a JSON value as `Object.assign(Object.create(null), value)` builds it: an object's entries, an
/// array's elements under their indices (each value: an entry or a list of entries).
pub fn known_of(v: Option<&J>) -> Known {
    let mut k = Known::new();
    match v {
        Some(o) if o.is_obj() => {
            for key in o.keys() {
                k.insert(key.clone(), o.get(&key).unwrap_or_default());
            }
        }
        Some(a) if a.is_arr() => {
            for (i, x) in a.items().into_iter().enumerate() {
                k.insert(i.to_string(), x);
            }
        }
        _ => {}
    }
    k
}
