//! api.rs — the engine as JSON text in / JSON text out: the face the desktop's `LayoutController` (crates/app
//! `layout_ctl.rs`) hands to QML. Every argument is JSON text; an empty (or whitespace-only) argument is absent (the
//! reference's `undefined`), and so is text that is not JSON data (§1.1) — except where the reference reads text itself
//! (`repair`, `load`: not JSON data → `replaced-by-classic not-json` / an ignored document). Every answer is the
//! canonical compact JSON of an object. Nothing here panics on any input (the controller still guards the call).

use crate::dropzones::drop_zones as dz;
use crate::json::{mk, read_json, J};
use crate::migrate::migrate as mg;
use crate::ops::{apply_op as op, OpResult};
use crate::repair::{repair_desktop_text, repair_phone_text, validate_desktop, validate_phone};
use crate::solve::solve as sv;
use crate::sync::{known_of, merge as mrg, project, save as sav, SaveReq, SaveResult};

/// A JSON-text argument: None when empty or not JSON data.
pub fn arg(text: &str) -> Option<J> {
    if text.trim().is_empty() {
        return None;
    }
    read_json(text)
}

/// The answer for an argument that must be present JSON data but is not.
fn not_json(what: &str) -> String {
    mk(vec![("ok", J::Bool(false)), ("error", J::s("bad-layout: not JSON data")), ("arg", J::s(what))]).canon()
}

/// `load(docJson, activeId)` — the projection of the account-data document `dev.tinhat.layouts` (LAYOUT.md §9.4):
/// `{read, layouts, active, name, desktop, phone, notes: {desktop, phone}, conflicts, quarantine}` — `conflicts` = the
/// active layout's concurrent losers `{rev, savedAt, by, deleted}` (merge order), `quarantine` = its current entry keeps
/// an original this version could not read (the host shows both, §11). `docJson` not JSON data → the built-in Classic
/// (read `ignore`); `activeId` empty → none asked.
pub fn load(doc: &str, active: &str) -> String {
    let d = arg(doc);
    project(d.as_ref(), (!active.is_empty()).then_some(active)).to_json().canon()
}

/// `repair(target, text)` — `desktop` | `phone`: a valid projection of any JSON text, `{form, notes}` (§5.1, §5.6);
/// `document`: the same as `load(text, "")`.
pub fn repair(target: &str, text: &str) -> String {
    match target {
        "phone" => repair_phone_text(text).to_json().canon(),
        "document" => load(text, ""),
        _ => repair_desktop_text(text).to_json().canon(),
    }
}

/// `validate(target, formJson)` — `{violations}` (§5.4): empty = valid. Text that is not JSON data is validated as
/// an absent value, which is not JSON data either (§1.4: `replaced-by-classic not-json`).
pub fn validate(target: &str, form: &str) -> String {
    let f = arg(form);
    let v = if target == "phone" { validate_phone(f.as_ref()) } else { validate_desktop(f.as_ref()) };
    mk(vec![("violations", J::strs(&v))]).canon()
}

/// `solve(formJson, ctxJson)` — ctx = `{size, unitPm, shape, content, view}` (§7.1, read leniently); the §7.13 output.
/// The form must be a valid (repaired) desktop form.
pub fn solve(form: &str, ctx: &str) -> String {
    let Some(f) = arg(form) else { return not_json("form") };
    let c = arg(ctx).filter(J::is_obj).unwrap_or_else(J::obj);
    sv(&f, c.get("size").as_ref(), c.get("unitPm").as_ref(), c.get("shape").as_ref(), c.get("content").as_ref(), c.get("view").as_ref())
        .to_json()
        .canon()
}

/// `applyOp(formJson, opJson, ctxJson)` — one transactional operation (§8): `{ok: true, form, remap}` or
/// `{ok: false, error}`. ctx (resize / resetSizes) as for solve.
pub fn apply_op(form: &str, op_text: &str, ctx: &str) -> String {
    let Some(f) = arg(form) else { return not_json("form") };
    let o = arg(op_text);
    let c = arg(ctx);
    let r = op(&f, o.as_ref(), c.as_ref());
    let j = r.to_json();
    j.set("ok", J::Bool(matches!(r, OpResult::Ok { .. })));
    j.canon()
}

/// `dropZones(formJson, ctxJson, dragged)` — `{hit, keys}` (§8.3); `dragged` is the dragged key (plain text).
pub fn drop_zones(form: &str, ctx: &str, dragged: &str) -> String {
    let Some(f) = arg(form) else { return not_json("form") };
    let c = arg(ctx);
    dz(&f, c.as_ref(), Some(&J::s(dragged))).to_json().canon()
}

/// `save(rawJson, knownJson, reqJson)` — a new entry that wins its own merge + the document to put (§9.5): req = `{id,
/// edit, start?, also?, now, by, seq, key?}` (edit `{name?, desktop, phone, discardQuarantine?}` or `{deleted: true}`;
/// `also` = the conflict a Restore / Dismiss incorporates; `key` = the chain key, default `by`, §9.7); `known` =
/// `{<id>: entry or [entries]}`. Answers `{ok: true, entry, doc}`, `{ok: true, noop: true}` or `{ok: false, error}`.
pub fn save(raw: &str, known: &str, req: &str) -> String {
    let r = arg(raw);
    let k = known_of(arg(known).as_ref());
    let q = arg(req).filter(J::is_obj).unwrap_or_else(J::obj);
    let rq = SaveReq {
        id: q.get("id"),
        edit: q.get("edit"),
        start: q.get("start").filter(|s| !s.is_null()),
        also: q.get("also").filter(|s| !s.is_null()),
        now: q.get("now"),
        by: q.get("by"),
        seq: q.get("seq"),
        key: q.get("key"),
    };
    let res = sav(r.as_ref(), &k, &rq);
    let j = res.to_json();
    j.set("ok", J::Bool(!matches!(res, SaveResult::Err(_))));
    j.canon()
}

/// `merge(rawJson, knownJson, justSaved)` — the join of the server document and the known entries (§9.6):
/// `{doc, tooLarge}` (`doc.conflicts` holds the concurrent losers); `justSaved` empty = none.
pub fn merge(raw: &str, known: &str, just_saved: &str) -> String {
    let r = arg(raw);
    let k = known_of(arg(known).as_ref());
    let (doc, too_large) = mrg(r.as_ref(), &k, (!just_saved.is_empty()).then_some(just_saved));
    mk(vec![("doc", doc), ("tooLarge", J::Bool(too_large))]).canon()
}

/// `migrate(argsJson)` — args = `{prefs?, H?, L?, chromeH?}` (§6.3): `{view}`.
/// `preset(name)` — a built-in desktop form by its name (`x-tinhat.presets`: classic, compact, handheld, sideMonitor,
/// callFirst …) or the internal migration preset `classic-flush` (§3.3 of ISLANDS-ARCH: the pre-islands geometry for
/// the 0-px gates; solve only, never stored) — `{form}`, or `{form: null}` for an unknown name. The dock host's form
/// until layouts are stored (C9).
pub fn preset(name: &str) -> String {
    let f = crate::schema::preset_desktop(name).or_else(|| crate::schema::internal_desktop(name));
    mk(vec![("form", f.unwrap_or(J::Null))]).canon()
}

pub fn migrate(args: &str) -> String {
    let a = arg(args).filter(J::is_obj).unwrap_or_else(J::obj);
    let v = mg(a.get("prefs").as_ref(), a.get("H").as_ref(), a.get("L").as_ref(), a.get("chromeH").as_ref());
    mk(vec![("view", v)]).canon()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_api_round_trips() {
        let classic = crate::schema::classic_desktop().canon();
        let p = read_json(&load("", "")).unwrap();
        assert_eq!(p.get("active").unwrap().as_str(), Some("main"));
        assert_eq!(p.get("desktop").unwrap().canon(), classic);
        let r = read_json(&repair("desktop", "{nope")).unwrap();
        assert_eq!(r.get("notes").unwrap().canon(), r#"["replaced-by-classic not-json"]"#);
        assert_eq!(validate("desktop", &classic), r#"{"violations":[]}"#);
        let o = read_json(&solve(&classic, r#"{"size":{"w":1560,"h":960}}"#)).unwrap();
        assert!(o.get("islands").unwrap().get("timeline").is_some());
        let op = read_json(&apply_op(&classic, r#"{"op":"hide","ref":"timeline"}"#, "")).unwrap();
        assert_eq!(op.canon(), r#"{"error":"bad-op: required island","ok":false}"#);
        let op = read_json(&apply_op(&classic, r#"{"op":"hide","ref":"rooms"}"#, "")).unwrap();
        assert!(op.get("ok").unwrap().is_true() && op.get("form").unwrap().get("hidden").is_some());
        assert!(solve("garbage", "").contains("not JSON data"));
        let z = read_json(&drop_zones(&classic, r#"{"size":{"w":1560,"h":960}}"#, "rooms")).unwrap();
        assert!(!z.get("hit").unwrap().is_empty());
        let s = read_json(&save(
            "",
            "",
            &format!(r#"{{"id":"main","edit":{{"desktop":{classic},"phone":{{}}}},"now":1,"by":"d","seq":1}}"#),
        ))
        .unwrap();
        assert!(s.get("ok").unwrap().is_true() && s.get("entry").unwrap().get("rev").unwrap().int_val() == Some(1));
        let m = read_json(&merge("", "", "")).unwrap();
        assert_eq!(m.get("doc").unwrap().canon(), r#"{"format":"tinhat-layouts","layouts":[],"v":1}"#);
        assert_eq!(migrate(r#"{"prefs":{"nav_mode":"corner"}}"#), r#"{"view":{"forceCollapse":["navcol"]}}"#);
    }
}
