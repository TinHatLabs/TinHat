//! palette.rs — port of `palette.ts`: the Ctrl+K command palette over rooms, pending invites, people and actions
//! (ACTIONS.md §10).
//! Pure: the snapshot and the query in, the ranked entries out; every result is the reference's, byte for byte (the
//! shared vectors palette/*.json).
//!
//! Portability rules (as in eval.rs): own keys only, strings compared by code point, case folded with the Unicode
//! default lower-casing (`String.prototype.toLowerCase` = `str::to_lowercase`), word separators from the table, no
//! locale functions, no regular expressions, stable sorts with explicit tie-breaks.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::eval::{default_keys, present, subject_entry, EvalInput, Snapshot};
use crate::js::{is_arr, is_obj, is_str, num, obj, os, s, str_at, JObj};
use crate::table::{action_by_id, table, Action, App};

// ---- folding and scoring --------------------------------------------------------------------------------------------

/// A word separator (the table's inclusive code-point ranges).
pub fn is_sep(cp: u32) -> bool {
    let x = cp as f64;
    table().palette.separators.iter().any(|&(lo, hi)| x >= lo && x <= hi)
}

/// Lower-case, every separator run → one space, trimmed (ACTIONS.md §10.3).
pub fn fold(text: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for ch in text.to_lowercase().chars() {
        if is_sep(ch as u32) {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    out
}

fn subsequence(q: &str, t: &str) -> bool {
    let qs: Vec<char> = q.chars().filter(|&c| c != ' ').collect();
    if qs.is_empty() {
        return false;
    }
    let mut i = 0;
    for ch in t.chars() {
        if ch == ' ' {
            continue;
        }
        if ch == qs[i] {
            i += 1;
            if i == qs.len() {
                return true;
            }
        }
    }
    false
}

/// How well a folded text matches a folded, non-empty query as a NAME (ACTIONS.md §10.4): prefix > word start >
/// every query word starts a word (two or more words) > substring > subsequence (only when `subseq`: room, invite,
/// person and choice names — never an action's longer labels) > 0.
pub fn name_tier(t: &str, q: &str, subseq: bool) -> f64 {
    let tiers = &table().palette.tiers;
    if t.is_empty() || q.is_empty() {
        return 0.0;
    }
    if t.starts_with(q) {
        return tiers.prefix;
    }
    if t.contains(&format!(" {q}")) {
        return tiers.word_start;
    }
    let qw: Vec<&str> = q.split(' ').collect();
    if qw.len() > 1 {
        let tw: Vec<&str> = t.split(' ').collect();
        if qw.iter().all(|w| tw.iter().any(|x| x.starts_with(w))) {
            return tiers.words;
        }
    }
    if t.contains(q) {
        return tiers.substring;
    }
    if subseq && subsequence(q, t) {
        return tiers.subsequence;
    }
    0.0
}

/// How well a folded text matches as a KEYWORD (an action's keywords): prefix > word start > substring.
pub fn keyword_tier(t: &str, q: &str) -> f64 {
    let tiers = &table().palette.tiers;
    if t.is_empty() || q.is_empty() {
        return 0.0;
    }
    if t.starts_with(q) {
        return tiers.keyword_prefix;
    }
    if t.contains(&format!(" {q}")) {
        return tiers.keyword_word_start;
    }
    if t.contains(q) {
        return tiers.keyword_substring;
    }
    0.0
}

/// An ALIAS (a room's or an invite's address, a person's user id) contains the query → the alias tier.
pub fn alias_tier(t: &str, q: &str) -> f64 {
    if !t.is_empty() && !q.is_empty() && t.contains(q) {
        table().palette.tiers.alias
    } else {
        0.0
    }
}

// ---- entries --------------------------------------------------------------------------------------------------------

/// `EntryKind = "invite" | "room" | "person" | "action" | "choice"` — `invite` = a pending invite to a room or a DM
/// (2026-09-28: Ctrl+K lists them again, ACTIONS.md §10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Invite,
    Room,
    Person,
    Action,
    Choice,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Invite => "invite",
            EntryKind::Room => "room",
            EntryKind::Person => "person",
            EntryKind::Action => "action",
            EntryKind::Choice => "choice",
        }
    }
}

/// One candidate of the palette before ranking.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub kind: EntryKind,
    /// room id (a room or an invite), user id, action id, or the chosen value
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    /// the first default key of an action ("Ctrl+Shift+M"), None otherwise
    pub hint: Option<String>,
    pub enabled: bool,
    /// an action that opens its target picker (stage 2)
    pub pick: bool,
    /// an action's / a choice's toggle state (the theme in use, the layout in use …), None for invites, rooms and people
    pub active: Option<bool>,
    /// an emoji drawn as the glyph (a status preset), None otherwise
    pub emoji: Option<String>,
    /// the action an action / choice entry runs (None for invites, rooms and people)
    pub a: Option<String>,
    /// the target a choice runs its action with (None otherwise)
    pub target: Option<JObj>,
    /// sorts after every entry without it: the room you are in, an unavailable action or choice
    pub last: bool,
    // ranking inputs (not part of the output)
    pub names: Vec<String>,
    /// names may match as a subsequence (rooms, invites, people, choices; not actions)
    pub subseq: bool,
    pub keywords: Vec<String>,
    pub aliases: Vec<String>,
    pub boost: f64,
    pub index: usize,
}

/// `valueOf(e, n)`: `e.values[n]`.
fn value_of<'a>(e: Option<&'a JObj>, n: &str) -> Option<&'a Value> {
    is_obj(e?.get("values"))?.get(n)
}

/// The ids of one subject kind in the snapshot's list order (`snapshot.lists.<list>`, `list` defaulting to the kind;
/// ids without a subject entry dropped, repeats dropped), else the subjects' ids in code-point order.
pub fn list_of(snap: &Snapshot, kind: &str, list: Option<&str>) -> Vec<String> {
    let list = list.unwrap_or(kind);
    let by_kind = is_obj(is_obj(snap.get("subjects")).and_then(|sj| sj.get(kind)));
    let l = is_obj(snap.get("lists")).and_then(|ls| ls.get(list));
    if let Some(l) = is_arr(l) {
        let mut out: Vec<String> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for x in l {
            if let Value::String(x) = x {
                if by_kind.is_some_and(|b| is_obj(b.get(x)).is_some()) && seen.insert(x.as_str()) {
                    out.push(x.clone());
                }
            }
        }
        return out;
    }
    match by_kind {
        // only ids whose subject entry is an object (as with a list)
        Some(b) => {
            let mut ids: Vec<String> = b.iter().filter(|(_, v)| v.is_object()).map(|(k, _)| k.clone()).collect();
            ids.sort();
            ids
        }
        None => Vec::new(),
    }
}

/// Room subjects as palette entries (`roomLikeEntries`): the JOINED rooms (`EntryKind::Room`, in `lists.room` order)
/// or the pending INVITES (`EntryKind::Invite`: `room.invited` and not `room.joined`, in `lists.invite` order — the
/// apps list them newest first). An invite is matched and ranked like a room: its name (the room list's), its address,
/// the unread / DM boosts, and the room you are in sorts last (ACTIONS.md §10.4–§10.5).
fn room_like_entries(snap: &Snapshot, kind: EntryKind) -> Vec<Entry> {
    let cur = str_at(snap.get("current"), "room");
    let boosts = &table().palette.boosts;
    let mut out: Vec<Entry> = Vec::new();
    for id in list_of(snap, "room", Some(kind.as_str())) {
        let e = subject_entry(snap, "room", &id);
        let joined = matches!(value_of(e, "room.joined"), Some(Value::Bool(true)));
        let listed = match kind {
            EntryKind::Invite => !joined && matches!(value_of(e, "room.invited"), Some(Value::Bool(true))),
            _ => joined,
        };
        if !listed {
            continue;
        }
        let name = e.and_then(|e| is_str(e.get("name"))).unwrap_or(&id).to_string();
        let alias = e.and_then(|e| is_str(e.get("alias")));
        let boost = (if matches!(value_of(e, "room.hasUnread"), Some(Value::Bool(true))) { boosts.unread } else { 0.0 })
            + (if matches!(value_of(e, "room.isDm"), Some(Value::Bool(true))) { boosts.dm } else { 0.0 });
        let index = out.len();
        out.push(Entry {
            kind,
            last: cur == Some(id.as_str()),
            names: vec![fold(&name)],
            aliases: alias.map(|al| vec![fold(al)]).unwrap_or_default(),
            id,
            label: name,
            icon: None,
            hint: None,
            enabled: true,
            pick: false,
            active: None,
            emoji: None,
            a: None,
            target: None,
            subseq: true,
            keywords: Vec::new(),
            boost,
            index,
        });
    }
    out
}

fn person_entries(snap: &Snapshot) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for id in list_of(snap, "person", None) {
        let e = subject_entry(snap, "person", &id);
        if matches!(value_of(e, "person.me"), Some(Value::Bool(true))) {
            continue;
        }
        let name = e.and_then(|e| is_str(e.get("name"))).unwrap_or(&id).to_string();
        let index = out.len();
        out.push(Entry {
            kind: EntryKind::Person,
            names: vec![fold(&name)],
            aliases: vec![fold(&id)],
            id,
            label: name,
            icon: None,
            hint: None,
            enabled: true,
            pick: false,
            active: None,
            emoji: None,
            a: None,
            target: None,
            last: false,
            subseq: true,
            keywords: Vec::new(),
            boost: 0.0,
            index,
        });
    }
    out
}

/// May this action be listed in the palette at all (ACTIONS.md §10.2)?
pub fn in_palette(a: &Action, app: App) -> bool {
    a.apps.iter().any(|x| x == app.as_str()) && a.contexts.is_none() && a.palette != Some(false) && !a.hold && a.pending.is_none()
}

fn action_entries(app: App, snap: &Snapshot) -> Vec<Entry> {
    let tbl = table();
    let mut group_rank: HashMap<&str, usize> = HashMap::new();
    for (i, g) in tbl.groups.iter().enumerate() {
        group_rank.insert(g.id.as_str(), i); // `new Map(…)`: a repeated id → the last index
    }
    let rank_of = |a: &Action| group_rank.get(a.group.as_str()).copied().unwrap_or(99);
    let mut cands: Vec<(&Action, String)> = tbl.actions.iter().filter(|a| in_palette(a, app)).map(|a| (a, fold(&a.label))).collect();
    // the catalogue order: group, then folded base label, then id
    cands.sort_by(|x, y| rank_of(x.0).cmp(&rank_of(y.0)).then_with(|| x.1.cmp(&y.1)).then_with(|| x.0.id.cmp(&y.0.id)));
    let mut out: Vec<Entry> = Vec::new();
    for (a, _) in cands {
        let p = present(&EvalInput::new(app, &a.id, snap));
        if !p.visible {
            continue;
        }
        let mut names = vec![fold(&p.label)];
        if let Some(sl) = &p.short_label {
            names.push(fold(sl));
        }
        if p.label != a.label {
            names.push(fold(&a.label));
        }
        let keywords = a.keywords.iter().flatten().map(|k| fold(k)).collect();
        let keys = default_keys(a, app);
        let index = out.len();
        out.push(Entry {
            kind: EntryKind::Action,
            id: a.id.clone(),
            label: p.label,
            icon: p.icon,
            hint: keys.into_iter().next(),
            enabled: p.enabled,
            pick: p.pick,
            active: p.active,
            emoji: p.emoji,
            a: Some(a.id.clone()),
            target: None,
            last: !p.enabled,
            names,
            subseq: false,
            keywords,
            aliases: Vec::new(),
            boost: 0.0,
            index,
        });
    }
    out
}

/// The choices of stage 2 for an action whose target is picked (ACTIONS.md §10.6), or None when the action has
/// nothing to pick here.
pub fn choice_entries(app: App, snap: &Snapshot, id: &str) -> Option<Vec<Entry>> {
    let a = action_by_id(id)?;
    if !in_palette(a, app) {
        return None;
    }
    let spec = a.target.as_ref()?;
    let tbl = table();
    let kind = tbl.target_kinds.get(&spec.kind)?;
    struct Choice {
        v: String,
        label: String,
        icon: Option<String>,
        key: String,
    }
    let mut values: Vec<Choice> = Vec::new();
    let absent_pick = spec.absent.as_deref() == Some("pick");
    let pickable = spec.pickable != Some(false);
    match &kind.key {
        Some(key) if kind.stored && !kind.portable && absent_pick && pickable => {
            // a device-local kind: this device's own subjects of the kind, in the picker's list (snapshot.lists.inputs /
            // outputs / cameras / audioSources — the target's picker, else the kind's)
            let subject_kind = kind.subject.as_deref().unwrap_or("undefined");
            let list = spec.picker.as_deref().or(kind.picker.as_deref()).unwrap_or(subject_kind);
            for sid in list_of(snap, subject_kind, Some(list)) {
                let e = subject_entry(snap, subject_kind, &sid);
                let label = e.and_then(|e| is_str(e.get("name"))).unwrap_or(&sid).to_string();
                values.push(Choice { v: sid, label, icon: None, key: key.clone() });
            }
        }
        Some(key) if kind.stored && absent_pick && pickable => {
            let vt = kind.value.as_ref().and_then(|v| tbl.value_types.get(v));
            let listed = spec.values.as_ref().or(vt.and_then(|vt| vt.values.as_ref()));
            if let Some(listed) = listed {
                for v in listed {
                    let label = match spec.labels.as_ref().and_then(|m| m.get(v)) {
                        Some(Value::String(l)) => l.clone(),
                        _ => v.clone(),
                    };
                    let icon = match spec.icons.as_ref().and_then(|m| m.get(v)) {
                        Some(Value::String(i)) => Some(i.clone()),
                        _ => None,
                    };
                    values.push(Choice { v: v.clone(), label, icon, key: key.clone() });
                }
            }
            if let Some(subject_kind) = &kind.subject {
                if vt.is_none_or(|vt| vt.values.is_none() || vt.or.is_some()) {
                    for sid in list_of(snap, subject_kind, None) {
                        let e = subject_entry(snap, subject_kind, &sid);
                        if subject_kind == "room" && !matches!(value_of(e, "room.joined"), Some(Value::Bool(true))) {
                            continue;
                        }
                        if subject_kind == "space" && !matches!(value_of(e, "space.joined"), Some(Value::Bool(true))) {
                            continue;
                        }
                        if subject_kind == "person" && matches!(value_of(e, "person.me"), Some(Value::Bool(true))) {
                            continue;
                        }
                        let label = e.and_then(|e| is_str(e.get("name"))).unwrap_or(&sid).to_string();
                        values.push(Choice { v: sid, label, icon: None, key: key.clone() });
                    }
                }
            }
        }
        _ => {
            if let Some(params) = &spec.params {
                // the first required enum param the action still needs (code-point order)
                for (pk, ps) in params {
                    if !ps.required || ps.ty != "enum" {
                        continue;
                    }
                    let Some(pvals) = &ps.values else { continue };
                    for v in pvals {
                        let label = match ps.labels.as_ref().and_then(|m| m.get(v)) {
                            Some(Value::String(l)) => l.clone(),
                            _ => v.clone(),
                        };
                        let icon = match ps.icons.as_ref().and_then(|m| m.get(v)) {
                            Some(Value::String(i)) => Some(i.clone()),
                            _ => None,
                        };
                        values.push(Choice { v: v.clone(), label, icon, key: pk.clone() });
                    }
                    break;
                }
            }
        }
    }
    if values.is_empty() {
        return None;
    }
    let mut out: Vec<Entry> = Vec::new();
    for x in values {
        let mut target = obj();
        target.insert(x.key.clone(), s(&x.v));
        let tv = Value::Object(target.clone());
        let p = present(&EvalInput { target: Some(&tv), transient: true, ..EvalInput::new(app, &a.id, snap) });
        if !p.visible {
            continue;
        }
        let index = out.len();
        out.push(Entry {
            kind: EntryKind::Choice,
            names: vec![fold(&x.label)],
            aliases: if x.label != x.v { vec![fold(&x.v)] } else { Vec::new() },
            id: x.v,
            label: x.label,
            icon: x.icon,
            hint: None,
            enabled: p.enabled,
            pick: false,
            active: p.active,
            emoji: p.emoji,
            a: Some(a.id.clone()),
            target: Some(target),
            last: !p.enabled,
            subseq: true,
            keywords: Vec::new(),
            boost: 0.0,
            index,
        });
    }
    Some(out)
}

// ---- ranking --------------------------------------------------------------------------------------------------------

/// An entry's score for a folded, non-empty query (0 = no match): the best of its name, keyword and alias tiers,
/// plus its boost when it matched at all.
pub fn score_entry(e: &Entry, q: &str) -> f64 {
    let mut best: f64 = 0.0;
    for n in &e.names {
        best = best.max(name_tier(n, q, e.subseq));
    }
    for k in &e.keywords {
        best = best.max(keyword_tier(k, q));
    }
    for al in &e.aliases {
        best = best.max(alias_tier(al, q));
    }
    if best > 0.0 {
        best + e.boost
    } else {
        0.0
    }
}

/// One ranked palette row.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteItem {
    pub kind: EntryKind,
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub hint: Option<String>,
    pub enabled: bool,
    pub pick: bool,
    pub active: Option<bool>,
    pub emoji: Option<String>,
    pub a: Option<String>,
    pub target: Option<JObj>,
    pub score: f64,
}

impl PaletteItem {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("kind".into(), s(self.kind.as_str()));
        o.insert("id".into(), s(&self.id));
        o.insert("label".into(), s(&self.label));
        o.insert("icon".into(), os(self.icon.as_deref()));
        o.insert("hint".into(), os(self.hint.as_deref()));
        o.insert("enabled".into(), Value::Bool(self.enabled));
        o.insert("pick".into(), Value::Bool(self.pick));
        o.insert("active".into(), self.active.map(Value::Bool).unwrap_or(Value::Null));
        o.insert("emoji".into(), os(self.emoji.as_deref()));
        o.insert("a".into(), os(self.a.as_deref()));
        o.insert("target".into(), self.target.clone().map(Value::Object).unwrap_or(Value::Null));
        o.insert("score".into(), num(self.score));
        Value::Object(o)
    }
}

/// The palette's answer (ACTIONS.md §10.5).
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteOut {
    /// all | room | person | action | stage
    pub mode: String,
    /// the folded query the ranking used
    pub query: String,
    /// the action being picked for (stage 2), else None
    pub stage: Option<String>,
    /// matches before the maxResults cap
    pub total: usize,
    pub items: Vec<PaletteItem>,
}

impl PaletteOut {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("mode".into(), s(&self.mode));
        o.insert("query".into(), s(&self.query));
        o.insert("stage".into(), os(self.stage.as_deref()));
        o.insert("total".into(), num(self.total as f64));
        o.insert("items".into(), Value::Array(self.items.iter().map(PaletteItem::to_json).collect()));
        Value::Object(o)
    }
}

/// Rank entries for a folded query (ACTIONS.md §10.5). Empty query: every entry, `last` ones after the rest, else in
/// entry order. Else the entries with a score, by (last, score descending, kind order, entry order). Returns
/// (total, the first `maxResults` items).
pub fn rank(entries: &[Entry], q: &str) -> (usize, Vec<PaletteItem>) {
    let pal = &table().palette;
    let kind_rank = |k: EntryKind| pal.kind_order.iter().position(|x| x == k.as_str()).unwrap_or(99);
    let mut scored: Vec<(&Entry, f64)> = entries
        .iter()
        .map(|e| (e, if q.is_empty() { 0.0 } else { score_entry(e, q) }))
        .filter(|&(_, sc)| q.is_empty() || sc > 0.0)
        .collect();
    scored.sort_by(|(x, xs), (y, ys)| {
        (x.last as u8)
            .cmp(&(y.last as u8))
            .then_with(|| ys.partial_cmp(xs).unwrap_or(Ordering::Equal))
            .then_with(|| kind_rank(x.kind).cmp(&kind_rank(y.kind)))
            .then_with(|| x.index.cmp(&y.index))
    });
    let total = scored.len();
    let items = scored
        .into_iter()
        .take(pal.max_results)
        .map(|(e, sc)| PaletteItem {
            kind: e.kind,
            id: e.id.clone(),
            label: e.label.clone(),
            icon: e.icon.clone(),
            hint: e.hint.clone(),
            enabled: e.enabled,
            pick: e.pick,
            active: e.active,
            emoji: e.emoji.clone(),
            a: e.a.clone(),
            target: e.target.clone(),
            score: sc,
        })
        .collect();
    (total, items)
}

/// The palette (ACTIONS.md §10): `query` as typed; `stage` = the action id whose target is being picked (stage 2), or
/// None. A leading prefix character (the table's `prefixes`: `>` actions, `@` people, `#` rooms and invites) limits
/// the kind. An empty query lists the pending invites, then the rooms (all / room mode, the room you are in last), the
/// people (person mode), every action (action mode: available ones first) or every choice (stage).
pub fn palette(app: App, snap: &Snapshot, query: &str, stage: Option<&str>) -> PaletteOut {
    if let Some(stage) = stage {
        let ch = choice_entries(app, snap, stage).unwrap_or_default();
        let q = fold(query);
        let (total, items) = rank(&ch, &q);
        return PaletteOut { mode: "stage".into(), query: q, stage: Some(stage.to_string()), total, items };
    }
    let mut mode = String::from("all");
    // leading spaces do not hide a prefix
    let trimmed = query.trim_start_matches(' ');
    let mut rest = query;
    // `rest.slice(i, i + 1)`: one UTF-16 code unit — an astral character's first half, which is no key
    if let Some(first) = trimmed.chars().next().filter(|c| (*c as u32) < 0x10000) {
        let mut buf = [0u8; 4];
        if let Some(pm) = table().palette.prefixes.get(&*first.encode_utf8(&mut buf)) {
            mode = pm.clone();
            rest = &trimmed[first.len_utf8()..];
        }
    }
    let q = fold(rest);
    let entries: Vec<Entry> = if mode == "room" || (mode == "all" && q.is_empty()) {
        let mut rooms = room_like_entries(snap, EntryKind::Invite);
        rooms.extend(room_like_entries(snap, EntryKind::Room));
        rooms
    } else if mode == "person" {
        person_entries(snap)
    } else if mode == "action" {
        action_entries(app, snap)
    } else {
        // all kinds; every kind's own entry order, the kinds in kindOrder
        let mut all = room_like_entries(snap, EntryKind::Invite);
        all.extend(room_like_entries(snap, EntryKind::Room));
        all.extend(action_entries(app, snap));
        all.extend(person_entries(snap));
        all
    };
    let (total, items) = rank(&entries, &q);
    PaletteOut { mode, query: q, stage: None, total, items }
}

/// A plain JSON copy of an entry list without the ranking inputs (for the vectors of the entry builders).
pub fn entries_out(es: &[Entry]) -> Vec<Value> {
    es.iter()
        .map(|e| {
            let mut o = obj();
            o.insert("kind".into(), s(e.kind.as_str()));
            o.insert("id".into(), s(&e.id));
            o.insert("label".into(), s(&e.label));
            o.insert("icon".into(), os(e.icon.as_deref()));
            o.insert("hint".into(), os(e.hint.as_deref()));
            o.insert("enabled".into(), Value::Bool(e.enabled));
            o.insert("pick".into(), Value::Bool(e.pick));
            o.insert("active".into(), e.active.map(Value::Bool).unwrap_or(Value::Null));
            o.insert("emoji".into(), os(e.emoji.as_deref()));
            o.insert("a".into(), os(e.a.as_deref()));
            o.insert("target".into(), e.target.clone().map(Value::Object).unwrap_or(Value::Null));
            o.insert("last".into(), Value::Bool(e.last));
            Value::Object(o)
        })
        .collect()
}
