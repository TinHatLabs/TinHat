//! dropzones.rs — port of `dropzones.ts`: LAYOUT.md §8.3: the drop zones of a drag, in hit-test order and in keyboard
//! order. A zone is listed only when its operation succeeds on the form (no zone ever promises a refused drop).

use std::collections::BTreeSet;

use crate::form::{find_key, hidden_index, is_island, islands_in, key_of, nodes_in};
use crate::json::{clamp_f, fdiv, is_json_value, mk, J};
use crate::ops::{apply_op, ctx_solve};
use crate::solve::Rect;

#[derive(Clone, Debug)]
pub struct Zone {
    /// corner | window | edge | centre
    pub kind: &'static str,
    pub rect: Rect,
    pub op: J,
}

impl Zone {
    pub fn to_json(&self) -> J {
        mk(vec![("kind", J::s(self.kind)), ("rect", self.rect.to_json()), ("op", self.op.clone())])
    }
}

/// `{hit, keys}`
#[derive(Clone, Debug, Default)]
pub struct Zones {
    pub hit: Vec<Zone>,
    pub keys: Vec<Zone>,
}

impl Zones {
    pub fn to_json(&self) -> J {
        mk(vec![
            ("hit", J::arr(self.hit.iter().map(Zone::to_json).collect())),
            ("keys", J::arr(self.keys.iter().map(Zone::to_json).collect())),
        ])
    }
}

fn cat(kind: &str) -> i64 {
    match kind {
        "corner" => 0,
        "window" => 1,
        "edge" => 2,
        _ => 3,
    }
}
fn side_rank(s: &str) -> i64 {
    match s {
        "right" | "tr" => 1,
        "top" | "bl" => 2,
        "bottom" | "br" => 3,
        _ => 0, // left, tl, and a centre's ""
    }
}

struct Cand {
    zone: Zone,
    target: String,
    side: &'static str,
}

pub fn drop_zones(form: &J, ctx: Option<&J>, dragged: Option<&J>) -> Zones {
    let Some(dragged) = dragged.and_then(J::as_str).map(str::to_string) else { return Zones::default() };
    // §1.4: a form or ctx outside the input domain → no zones (a missing ctx is a 0 × 0 window: no zones either)
    if !form.is_obj() || !is_json_value(Some(form)) || ctx.is_some_and(|c| !is_json_value(Some(c))) {
        return Zones::default();
    }
    let hidden = hidden_index(form, &dragged).is_some();
    let l = find_key(form, &dragged);
    if l.is_none() && !hidden {
        return Zones::default();
    }
    let excluded: BTreeSet<String> = match &l {
        Some(l) => {
            let mut s: BTreeSet<String> = nodes_in(&l.node).iter().map(|n| key_of(n).map(|k| k.to_string()).unwrap_or_default()).collect();
            s.extend(islands_in(&l.node).iter().map(|r| r.to_string()));
            s
        }
        None => BTreeSet::from([dragged.clone()]),
    };
    let det = ctx_solve(form, ctx);
    let c = ctx.filter(|c| c.is_obj()).cloned().unwrap_or_else(J::obj);
    let size = c.get("size").filter(J::is_obj);
    let dim = |k: &str| -> i64 {
        match size.as_ref().and_then(|s| s.get(k)) {
            Some(v) if v.is_int() => clamp_f(v.as_f64().unwrap_or(0.0), 0.0, 16384.0) as i64,
            _ => 0,
        }
    };
    let (ww, hh) = (dim("w"), dim("h"));
    let host = det.host;
    let mut cands: Vec<Cand> = Vec::new();
    let wrap = |op: J| -> J {
        if !hidden {
            return op;
        }
        let to = J::obj();
        for k in op.keys() {
            if k != "ref" {
                to.set(&k, op.get(&k).unwrap_or_default());
            }
        }
        mk(vec![("op", J::s("restore")), ("ref", J::s(&dragged)), ("to", to)])
    };
    let mut add = |kind: &'static str, rect: Rect, op: J, target: &str, side: &'static str| {
        if rect.w <= 0 || rect.h <= 0 {
            return;
        }
        let full = wrap(op);
        if !apply_op(form, Some(&full), ctx).is_ok() {
            return;
        }
        cands.push(Cand { zone: Zone { kind, rect, op: full }, target: target.to_string(), side });
    };
    let cw = host.w.min(48);
    let chh = host.h.min(48);
    let corners: [(&'static str, i64, i64); 4] = [
        ("tl", host.x, host.y),
        ("tr", host.x + host.w - cw, host.y),
        ("bl", host.x, host.y + host.h - chh),
        ("br", host.x + host.w - cw, host.y + host.h - chh),
    ];
    for (a, x, y) in corners {
        let op = mk(vec![("op", J::s("float")), ("ref", J::s(&dragged)), ("anchor", J::s(a)), ("dx", J::int(8)), ("dy", J::int(8))]);
        add("corner", Rect { x, y, w: cw, h: chh }, op, "", a);
    }
    let bw = ww.min(24);
    let bh = hh.min(24);
    let edges: [(&'static str, Rect); 4] = [
        ("left", Rect { x: 0, y: 0, w: bw, h: hh }),
        ("right", Rect { x: ww - bw, y: 0, w: bw, h: hh }),
        ("top", Rect { x: 0, y: 0, w: ww, h: bh }),
        ("bottom", Rect { x: 0, y: hh - bh, w: ww, h: bh }),
    ];
    for (s, r) in edges {
        add("window", r, mk(vec![("op", J::s("edge")), ("ref", J::s(&dragged)), ("side", J::s(s))]), "", s);
    }
    // det.out.islands is keyed in code-point order already
    for (rf, t) in &det.out.islands {
        if t.z != 0 || !t.vis || excluded.contains(rf) {
            continue;
        }
        let Some(tl) = find_key(form, rf) else { continue };
        if !is_island(&tl.node) {
            continue;
        }
        let d = 48.min(fdiv(t.w, 4));
        let dv = 48.min(fdiv(t.h, 4));
        let bands: [(&'static str, Rect); 4] = [
            ("left", Rect { x: t.x, y: t.y, w: d, h: t.h }),
            ("right", Rect { x: t.x + t.w - d, y: t.y, w: d, h: t.h }),
            ("top", Rect { x: t.x, y: t.y, w: t.w, h: dv }),
            ("bottom", Rect { x: t.x, y: t.y + t.h - dv, w: t.w, h: dv }),
        ];
        for (s, r) in bands {
            let op = mk(vec![("op", J::s("dockBeside")), ("ref", J::s(&dragged)), ("target", J::s(rf)), ("side", J::s(s))]);
            add("edge", r, op, rf, s);
        }
        let op = mk(vec![("op", J::s("stackAsTab")), ("ref", J::s(&dragged)), ("target", J::s(rf))]);
        add("centre", Rect { x: t.x + d, y: t.y + dv, w: t.w - 2 * d, h: t.h - 2 * dv }, op, rf, "");
    }
    let area = |z: &Cand| z.zone.rect.w * z.zone.rect.h;
    let tie = |p: &Cand, q: &Cand| p.target.cmp(&q.target).then(side_rank(p.side).cmp(&side_rank(q.side)));
    let mut hit: Vec<usize> = (0..cands.len()).collect();
    hit.sort_by(|&i, &j| {
        let (p, q) = (&cands[i], &cands[j]);
        cat(p.zone.kind)
            .cmp(&cat(q.zone.kind))
            .then(area(p).cmp(&area(q)))
            .then(p.zone.rect.y.cmp(&q.zone.rect.y))
            .then(p.zone.rect.x.cmp(&q.zone.rect.x))
            .then(tie(p, q))
    });
    let mut keys: Vec<usize> = (0..cands.len()).collect();
    keys.sort_by(|&i, &j| {
        let (p, q) = (&cands[i], &cands[j]);
        p.zone
            .rect
            .y
            .cmp(&q.zone.rect.y)
            .then(p.zone.rect.x.cmp(&q.zone.rect.x))
            .then(cat(p.zone.kind).cmp(&cat(q.zone.kind)))
            .then(area(p).cmp(&area(q)))
            .then(tie(p, q))
    });
    Zones {
        hit: hit.iter().map(|&i| cands[i].zone.clone()).collect(),
        keys: keys.iter().map(|&i| cands[i].zone.clone()).collect(),
    }
}
