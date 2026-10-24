//! normalize.rs — port of `normalize.ts`: LAYOUT.md §4, N1-N9, after every operation and as repair's last step;
//! idempotent. Never removes, unwraps or hoists a node that carries unknown keys (itself or in its size), never edits
//! unknown node types or unknown islands (their `c` children are normalized like any children).

use std::collections::{BTreeMap, BTreeSet};

use crate::form::{
    all_locs, carries_unknown, fix_size, hidden_of, is_container, is_island, is_known_type, is_stack, is_unknown_island,
    key_of, kids_arr, kids_of, r1ok, room_locs, size_has_unknown, Ids,
};
use crate::json::{js_str, mk, J};
use crate::schema::def_of;

pub struct Normalized {
    pub form: J,
    /// old key → new key of the N2/N3 replacements (chains collapsed)
    pub remap: BTreeMap<String, String>,
    pub notes: Vec<String>,
}

struct Norm {
    notes: Vec<String>,
    remap: BTreeMap<String, String>,
}

impl Norm {
    fn note(&mut self, n: &str, s: &str) {
        self.notes.push(format!("{n} {s}"));
    }
    fn set_remap(&mut self, from: &str, to: &str) {
        for v in self.remap.values_mut() {
            if v == from {
                *v = to.to_string();
            }
        }
        self.remap.insert(from.to_string(), to.to_string());
    }

    /// N1-N3, post-order; the root is never removed. None = removed.
    fn norm(&mut self, n: &J, is_root: bool) -> Option<J> {
        if is_island(n) {
            return Some(n.clone());
        }
        if let Some(c) = kids_arr(n) {
            let mut kept = Vec::new();
            for k in c.items() {
                let r = if k.is_obj() { self.norm(&k, false) } else { Some(k) };
                if let Some(r) = r {
                    kept.push(r);
                }
            }
            n.set("c", J::arr(kept));
        }
        if !is_known_type(n) {
            return Some(n.clone()); // unknown node types stay (N1-N3 never remove or unwrap them)
        }
        let kids = kids_of(n);
        let id = js_str(n.get("id").as_ref());
        if kids.is_empty() {
            if is_root || carries_unknown(n) {
                return Some(n.clone());
            }
            self.note("empty-container-removed", &id);
            return None;
        }
        if kids.len() == 1 && !carries_unknown(n) {
            let child = &kids[0];
            let hoistable = is_known_type(child)
                && !is_unknown_island(child)
                && !size_has_unknown(child.get("size").as_ref())
                && (!is_root || is_container(child));
            if hoistable && (is_container(n) || is_stack(n)) {
                let s = n.get("size");
                if is_root {
                    match s {
                        Some(s) => child.set("size", s),
                        None => child.del("size"),
                    }
                } else {
                    child.set("size", s.unwrap_or_else(|| mk(vec![("share", J::int(1))])));
                }
                self.note("single-child-unwrapped", &id);
                if let Some(nk) = key_of(child) {
                    self.set_remap(&id, &nk);
                }
                return Some(child.clone());
            }
        }
        Some(n.clone())
    }
}

/// Normalize `f` IN PLACE; returns the old → new key remapping of N2/N3 and the notes (repair uses them).
pub fn normalize_in_place(f: &J) -> Normalized {
    let mut s = Norm { notes: Vec::new(), remap: BTreeMap::new() };

    if let Some(root) = f.get("root").filter(J::is_obj) {
        let r = s.norm(&root, true).unwrap_or(root);
        f.set("root", r);
        // N5: the root is a row or col
        let root = f.get("root").unwrap_or_default();
        if !is_container(&root) {
            let mut ids = Ids::new(f);
            let id = ids.body();
            f.set("root", mk(vec![("t", J::s("row")), ("id", J::s(&id)), ("c", J::arr(vec![root]))]));
            s.note("root-wrapped", &id);
        }
    }
    if let Some(fl) = f.get("floats").filter(J::is_arr) {
        let mut i = 0usize;
        while i < fl.len() {
            let x = fl.at(i).unwrap_or_default();
            let node = x.get("node").filter(J::is_obj);
            let r = node.and_then(|n| s.norm(&n, false));
            match r {
                None => {
                    let subj = x.get_str("id").map(|s| s.to_string()).unwrap_or_else(|| format!("#{i}"));
                    s.note("bad-float-fixed", &subj);
                    fl.remove_at(i);
                }
                Some(r) => {
                    x.set("node", r);
                    i += 1;
                }
            }
        }
    }

    let locs = all_locs(f);
    // N4: a stack's `active` names one of its children, else the first
    for l in &locs {
        if !is_stack(&l.node) {
            continue;
        }
        let refs: Vec<Option<J>> = kids_of(&l.node).iter().filter(|k| is_island(k)).map(|k| k.get("i")).collect();
        if refs.is_empty() {
            // an empty stack kept for its unknown keys names no tab
            if l.node.has("active") {
                l.node.del("active");
                s.note("bad-field-fixed", &js_str(l.node.get("id").as_ref()));
            }
            continue;
        }
        let a = l.node.get_str("active");
        let ok = a.as_ref().is_some_and(|a| refs.iter().any(|r| r.as_ref().is_some_and(|r| r.eq_str(a))));
        if !ok {
            match &refs[0] {
                Some(r) => l.node.set("active", r.clone()),
                None => l.node.del("active"),
            }
            s.note("bad-field-fixed", &js_str(l.node.get("id").as_ref()));
        }
    }
    // N6: sizes of known nodes (not unknown islands), then the siblings' minPm sum
    for l in &locs {
        if !is_known_type(&l.node) || is_unknown_island(&l.node) || !l.node.has("size") {
            continue;
        }
        let (size, changed) = fix_size(l.node.get("size").as_ref());
        if changed {
            match size {
                Some(sz) => l.node.set("size", sz),
                None => l.node.del("size"),
            }
            s.note("bad-size-fixed", &js_str(key_of(&l.node).map(|k| J::Str(k)).as_ref()));
        }
    }
    for h in hidden_of(f) {
        if !h.is_obj() || !h.has("size") || is_unknown_island(&h) {
            continue;
        }
        let (size, changed) = fix_size(h.get("size").as_ref());
        if changed {
            match size {
                Some(sz) => h.set("size", sz),
                None => h.del("size"),
            }
            s.note("bad-size-fixed", &js_str(h.get("i").as_ref()));
        }
    }
    for l in &locs {
        if !is_container(&l.node) {
            continue;
        }
        let kids: Vec<J> = kids_of(&l.node).into_iter().filter(|k| is_known_type(k) && !is_unknown_island(k)).collect();
        let pm = |k: &J| -> f64 {
            let sz = k.get("size");
            sz.filter(J::is_obj).and_then(|s| s.get("minPm")).and_then(|v| v.as_f64()).unwrap_or(0.0)
        };
        let mut sum: f64 = kids.iter().map(pm).sum();
        if sum <= 1000.0 {
            continue;
        }
        let mut i = kids.len();
        while i > 0 && sum > 1000.0 {
            i -= 1;
            let v = pm(&kids[i]);
            if v == 0.0 {
                continue;
            }
            if let Some(sz) = kids[i].get("size") {
                sz.del("minPm");
            }
            sum -= v;
        }
        s.note("minpm-capped", &js_str(l.node.get("id").as_ref()));
    }

    // N7: hidden and collapse deduplicated (first wins); remapped refs; steps whose ref is gone or whose `to` its new
    // node does not allow are dropped
    let mut placed: BTreeSet<String> = BTreeSet::new();
    for l in all_locs(f) {
        if is_island(&l.node) {
            placed.insert(js_str(l.node.get("i").as_ref()));
        }
    }
    if let Some(hid) = f.get("hidden").filter(J::is_arr) {
        let mut seen_h: BTreeSet<String> = BTreeSet::new();
        let mut i = 0usize;
        while i < hid.len() {
            let h = hid.at(i).unwrap_or_default();
            let r = js_str(h.get("i").as_ref());
            if placed.contains(&r) || seen_h.contains(&r) {
                s.note("dropped-duplicate", &r);
                hid.remove_at(i);
                continue;
            }
            seen_h.insert(r);
            // a `from.parent` follows a remap only onto a container or stack (restore never accepts an island; an
            // island's ref need not even be id syntax): otherwise it stays, a stale hint
            let from = h.get("from").filter(J::is_obj);
            if let Some(from) = from {
                if let Some(fp) = from.get_str("parent") {
                    if let Some(to) = s.remap.get(&*fp).cloned() {
                        let found = all_locs(f)
                            .iter()
                            .any(|x| (is_container(&x.node) || is_stack(&x.node)) && x.node.is("id", &to));
                        if found {
                            from.set("parent", J::s(&to));
                        }
                    }
                }
            }
            i += 1;
        }
    }
    if let Some(col) = f.get("collapse").filter(J::is_arr) {
        let root_locs: Vec<_> = all_locs(f).into_iter().filter(|l| l.float < 0).collect();
        let mut seen_c: BTreeSet<String> = BTreeSet::new();
        let mut i = 0usize;
        while i < col.len() {
            let st = col.at(i).unwrap_or_default();
            let mut r = js_str(st.get("ref").as_ref());
            if let Some(to) = s.remap.get(&r).cloned() {
                r = to;
                st.set("ref", J::s(&r));
            }
            let l = root_locs.iter().find(|l| is_known_type(&l.node) && key_of(&l.node).as_deref() == Some(r.as_str()));
            let mut ok = l.is_some();
            if let Some(l) = l {
                if is_island(&l.node) {
                    if let Some(d) = def_of(&r) {
                        let to = st.get_str("to");
                        if !to.is_some_and(|t| d.collapse_allows(&t)) {
                            ok = false;
                        }
                    }
                } else if st.is("to", "fold") {
                    ok = false;
                }
            }
            if !ok {
                s.note("bad-collapse-dropped", &r);
                col.remove_at(i);
                continue;
            }
            if seen_c.contains(&r) {
                s.note("dropped-duplicate", &r);
                col.remove_at(i);
                continue;
            }
            seen_c.insert(r);
            i += 1;
        }
    }

    // N9: opts.composer.composerTop and opts.header.headerBottom record the room group's order in the tree
    let room = room_locs(f);
    if r1ok(&room) {
        let t = room.timeline.as_ref().expect("r1ok");
        let c = room.composer.as_ref().expect("r1ok");
        let k = J::arr(kids_of(t.parent.as_ref().expect("r1ok")));
        let ci = k.index_of(&c.node).map_or(-1, |i| i as i64);
        let ti = k.index_of(&t.node).map_or(-1, |i| i as i64);
        set_flag(f, "composer", "composerTop", ci < ti, &mut s);
    }
    if let Some(h) = &room.header {
        if h.float < 0 {
            if let Some(p) = h.parent.as_ref().filter(|p| p.is("t", "col")) {
                let k = kids_of(p);
                let i = k.iter().position(|x| x.same(&h.node)).map_or(-1, |i| i as i64);
                set_flag(f, "header", "headerBottom", k.len() > 1 && i == k.len() as i64 - 1, &mut s);
            }
        }
    }
    Normalized { form: f.clone(), remap: s.remap, notes: s.notes }
}

fn set_flag(f: &J, r: &str, key: &str, on: bool, s: &mut Norm) {
    let opts = f.get("opts");
    let o = opts.as_ref().filter(|o| o.is_obj()).and_then(|o| o.get(r));
    let cur = o.as_ref().filter(|o| o.is_obj()).and_then(|o| o.get(key));
    if on {
        if cur.as_ref().is_some_and(|c| c.is_true()) {
            return;
        }
        let oo = match opts.filter(J::is_obj) {
            Some(oo) => oo,
            None => {
                let oo = J::obj();
                f.set("opts", oo.clone());
                oo
            }
        };
        let rr = match oo.get(r).filter(J::is_obj) {
            Some(rr) => rr,
            None => {
                let rr = J::obj();
                oo.set(r, rr.clone());
                rr
            }
        };
        rr.set(key, J::Bool(true));
        s.note("opts-fixed", r);
    } else if cur.is_some() {
        if let Some(o) = o.filter(J::is_obj) {
            o.del(key);
            if o.nkeys() == 0 {
                if let Some(opts) = f.get("opts") {
                    opts.del(r);
                }
            }
            s.note("opts-fixed", r);
        }
    }
}

/// A normalized copy (operations: step 8).
pub fn normalize(f: &J) -> Normalized {
    normalize_in_place(&f.deep())
}
