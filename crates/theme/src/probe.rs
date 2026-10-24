//! probe.rs — port of `probe.ts` (contract THEMES-UI.md §15): the Pick tool's probe colours. To find which named
//! colours paint the element under the pointer, an app swaps in a PROBE THEME — every role and element token a
//! different colour that encodes its index — reads the element's colour properties, and swaps the real theme back, all
//! inside one synchronous step (no frame is drawn in between: the desktop's bindings update at once and the scene graph
//! only syncs after the GUI thread returns to its loop). A colour read in the probe pass decodes back to the id that
//! painted it; alpha is ignored (`alpha(token, n)` keeps the token's rgb), anything computed from a token with other
//! maths does not decode (and is reported as not named). The same codec in both apps (web `src/theme/probe.ts`, the
//! REFERENCE), pinned by `testvectors/editor/probe-*.json` and `picklist-*.json` (tests/editor.rs).

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Value};

use crate::engine::TokensSpec;
use crate::roles::ROLE_IDS;

/// At most this many ids (the codec's 12-bit index).
pub const PROBE_MAX: usize = 4095;

/// The ids the probe paints, in index order: every role (sorted), then every token of the spec (sorted). More than
/// [`PROBE_MAX`] ids is the reference's error `probe: too many ids`.
pub fn probe_ids(spec: &TokensSpec) -> Result<Vec<String>, String> {
    let mut ids: Vec<String> = ROLE_IDS.iter().map(|s| s.to_string()).collect();
    ids.extend(spec.ids().iter().cloned()); // TokensSpec keeps its ids sorted (code-point order)
    if ids.len() > PROBE_MAX {
        return Err("probe: too many ids".into());
    }
    Ok(ids)
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// The probe colour of index `i` (0-based, below [`PROBE_MAX`]): j = i + 1; r = j >> 4;
/// g = (j & 15) << 4 | (7j + 3) & 15; b = (151j + 89) & 255. (For i ≥ PROBE_MAX, where the reference's r is no longer a
/// byte, r wraps.)
pub fn probe_rgb(i: usize) -> (u8, u8, u8) {
    // wrapping: only the low bits are kept, so any index is safe (the codec's domain never wraps)
    let j = (i as u64).wrapping_add(1);
    let g = ((j & 15) << 4) | (j.wrapping_mul(7).wrapping_add(3) & 15);
    let b = j.wrapping_mul(151).wrapping_add(89) & 255;
    ((j >> 4) as u8, g as u8, b as u8)
}

/// `#rrggbb` of [`probe_rgb`]`(i)`.
pub fn probe_colour(i: usize) -> String {
    let (r, g, b) = probe_rgb(i);
    let mut s = String::with_capacity(7);
    s.push('#');
    for v in [r, g, b] {
        s.push(HEX[(v >> 4) as usize] as char);
        s.push(HEX[(v & 15) as usize] as char);
    }
    s
}

/// The index a colour decodes to (0 ≤ index < count), or `None` when it is not a probe colour of this set (the
/// reference's -1).
pub fn probe_index(r: u8, g: u8, b: u8, count: usize) -> Option<usize> {
    let j = ((r as u64) << 4) | ((g as u64) >> 4);
    if j < 1 || j > count as u64 {
        return None;
    }
    if (g as u64 & 15) != ((j * 7 + 3) & 15) {
        return None;
    }
    if b as u64 != ((j * 151 + 89) & 255) {
        return None;
    }
    Some(j as usize - 1)
}

/// id → probe colour for every probe id (what the app paints during the probe pass).
pub fn probe_colours(spec: &TokensSpec) -> Result<BTreeMap<String, String>, String> {
    Ok(probe_ids(spec)?.into_iter().enumerate().map(|(i, id)| (id, probe_colour(i))).collect())
}

/// What a picked colour property is (the Pick list's badge).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    Text,
    Fill,
    Border,
    Icon,
    Shadow,
    Other,
}

impl PickKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PickKind::Text => "text",
            PickKind::Fill => "fill",
            PickKind::Border => "border",
            PickKind::Icon => "icon",
            PickKind::Shadow => "shadow",
            PickKind::Other => "other",
        }
    }

    /// `text` / `fill` / `border` / `icon` / `shadow` / `other`; anything else none.
    pub fn parse(s: &str) -> Option<PickKind> {
        [PickKind::Text, PickKind::Fill, PickKind::Border, PickKind::Icon, PickKind::Shadow, PickKind::Other]
            .into_iter()
            .find(|k| k.as_str() == s)
    }
}

/// A named colour an element paints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickHit {
    pub id: String,
    pub kind: PickKind,
}

impl PickHit {
    /// `{"id": …, "kind": …}`
    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "kind": self.kind.as_str() })
    }
}

/// One element under the pointer (nearest first) and the named colours it paints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickLayer {
    pub element: String,
    pub hits: Vec<PickHit>,
}

/// The reference's default `max` of [`pick_list`] (THEMES-UI.md §15.2: at most 8 layers).
pub const PICK_MAX_LAYERS: usize = 8;

/// The Pick list (§15.3): the ids of the layers in order, each id once (its first kind), at most `max` layers.
pub fn pick_list(layers: &[PickLayer], max: usize) -> Vec<PickHit> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for l in layers.iter().take(max) {
        for h in &l.hits {
            if !seen.insert(h.id.as_str()) {
                continue;
            }
            out.push(h.clone());
        }
    }
    out
}
