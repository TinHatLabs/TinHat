#!/usr/bin/env node
// diff-cases.mjs — the differential corpus for the Rust port (crates/layout/tests/differential.rs): seeded random
// cases of EVERY vector kind, each computed with the TypeScript reference and written as one JSON line in the vector
// format ({kind, …inputs, expect}); the Rust test recomputes `expect` with the port and compares canonically.
//
//   node --import <REF>/tools/theme-lib/register.mjs diff-cases.mjs <REF> <CASES> <SEED> [<FIRST>]
//
// Case c (c = FIRST … FIRST+CASES−1, FIRST default 0) is seeded with (SEED + c·7919) mod 2³² and starts from preset
// c mod the preset count, so shards (FIRST = 0, n, 2n, …) together are exactly one run of all their cases
// (rust-bridge/diff-shards.sh).
//
// <REF> = a tinhat-web tree (or a pinned `git archive` of one: src/layout, src/theme/{canon,limits}.ts, tools/layout,
// tools/theme-lib, package.json). One case = one seed, the flow of the reference's tools/layout/differential.mjs: a
// random operation sequence from a preset (every operation a record; solves, drop zones along the way), three solves of
// the final form, a corrupted copy through repair (text) and validate (value), and — by chance — phone repair, document
// projection, save, merge, a scripted sync run and migrate. Every record's inputs go through JSON text first, so the
// reference computes on exactly the value the port reads. Prints a summary line on stderr.
import path from "node:path";
import { pathToFileURL } from "node:url";

const [REF, CASES = "1000", SEED = "20260927", FIRST = "0"] = process.argv.slice(2);
if (!REF) {
  console.error("usage: diff-cases.mjs <REF> <CASES> <SEED> [<FIRST>]");
  process.exit(2);
}
const imp = (p) => import(pathToFileURL(path.join(REF, p)).href);
const { computeExpect } = await imp("tools/layout/vector-run.mjs");
const { corrupt, randomCtx, randomOp, xorshift32 } = await imp("tools/layout/gen-lib.mjs");
const { applyOp } = await imp("src/layout/ops.ts");
const { presetDesktop, presetPhone, PRESET_NAMES, REGION_ORDER } = await imp("src/layout/schema.ts");
const { allLocs, hiddenOf, keyOf } = await imp("src/layout/form.ts");
const { merge } = await imp("src/layout/sync.ts");

const N = Number(CASES);
const S0 = Number(SEED) >>> 0;
const C0 = Number(FIRST);
if (!Number.isInteger(N) || N < 0 || !Number.isInteger(C0) || C0 < 0) {
  console.error(`diff-cases: bad CASES ${CASES} / FIRST ${FIRST}`);
  process.exit(2);
}
const counts = {};
const out = [];
let pending = 0;
const flush = () => {
  if (out.length) {
    process.stdout.write(out.join(""));
    out.length = 0;
    pending = 0;
  }
};
/** One record: the inputs through JSON text, then the reference's expect. */
function emit(kind, fields) {
  const rec = JSON.parse(JSON.stringify({ kind, ...fields }));
  rec.expect = computeExpect(rec);
  const line = JSON.stringify(rec) + "\n";
  out.push(line);
  pending += line.length;
  counts[kind] = (counts[kind] ?? 0) + 1;
  if (pending > 1 << 20) flush();
  return rec;
}

// ---- generators for the kinds gen-lib does not cover ----------------------------------------------------------
const pick = (rnd, a) => a[rnd.int(a.length)];
const JUNK = [null, 1.5, -0, "x", true, [], {}, 1e21, "Bad Ref", 5, "zz9"];

function randomPhone(rnd) {
  const f = JSON.parse(JSON.stringify(presetPhone(pick(rnd, PRESET_NAMES))));
  const refs = [...REGION_ORDER, "buttons:a1", "buttons:b2", "buttons", "rooms:x", "futureThing", "Bad Ref", "timeline", "call", "panel"];
  const n = rnd.range(0, 6);
  for (let i = 0; i < n; i++) {
    const k = pick(rnd, ["rail", "list", "hidden"]);
    switch (rnd.int(8)) {
      case 0:
        f[k] = pick(rnd, [5, "x", null, {}]);
        break;
      case 1:
        if (!Array.isArray(f[k])) f[k] = [];
        f[k].push(rnd.chance(700) ? pick(rnd, refs) : pick(rnd, JUNK));
        break;
      case 2:
        if (Array.isArray(f[k]) && f[k].length) f[k].splice(rnd.int(f[k].length), 1);
        break;
      case 3:
        if (Array.isArray(f[k])) f[k].reverse();
        break;
      case 4:
        delete f[k];
        break;
      case 5:
        f[k] = Array.from({ length: rnd.range(10, 40) }, (_, j) => (rnd.chance(500) ? `buttons:q${j}` : `zz${j}`));
        break;
      case 6:
        f.opts = pick(rnd, [{ rooms: { compact: rnd.chance(500), orient: pick(rnd, ["h", "v", "x"]) } }, "x", { "Bad Key": {} }, { nav: { widgets: [{ w: "upNext", size: "M" }, { w: 5 }] } }]);
        break;
      default:
        f[pick(rnd, ["future", "rail2"])] = pick(rnd, JUNK);
    }
  }
  return f;
}

let clock = 0;
function randomEntry(rnd, ids) {
  const id = rnd.chance(950) ? pick(rnd, ids) : pick(rnd, ["Bad Id", 5, "", "x".repeat(40)]);
  const by = pick(rnd, ["a", "b", "c", "dev-z", "", "é🎨", "a#2", "b"]);
  const rev = rnd.chance(900) ? rnd.range(0, 6) : pick(rnd, [-1, 2147483647, 2147483648, 1.5, "3"]);
  const e = { id, rev, savedAt: rnd.chance(900) ? rnd.range(0, 60) : pick(rnd, [-5, 9007199254740992, "x"]), by, vv: { [by || "a"]: rnd.range(1, 9) } };
  if (rnd.chance(300)) e.vv[pick(rnd, ["a", "b", "c", "a#3", "b#12", "c#0", "a#x", "#1"])] = rnd.range(1, 9);
  if (rnd.chance(100)) e.vv = pick(rnd, [{ a: 0, b: 1.5, c: 3 }, "x", { d0: 1, d1: 2, d2: 3, d3: 4, d4: 5, d5: 6, d6: 7, d7: 8, d8: 9, d9: 10, e0: 11, e1: 12, e2: 13, e3: 14, e4: 15, e5: 16, e6: 17 }]);
  if (rnd.chance(200)) return { ...e, deleted: rnd.chance(800) ? true : pick(rnd, [false, "yes", 1]) };
  e.name = rnd.chance(900) ? pick(rnd, ["Main", "Side", "Hand", "é🎨", "n".repeat(41), ""]) : pick(rnd, [5, null]);
  e.desktop = rnd.chance(950) ? { ...presetDesktop(pick(rnd, PRESET_NAMES)), tag: `${by}${rnd.int(4)}` } : pick(rnd, ["x", null, [1]]);
  e.phone = rnd.chance(950) ? presetPhone(pick(rnd, PRESET_NAMES)) : pick(rnd, [5, null]);
  if (rnd.chance(300)) e.previous = pick(rnd, [{ rev: 1, savedAt: 1, by: "b", desktop: { t: 1 }, phone: {} }, { desktop: { t: 2 }, phone: {}, previous: { x: 1 } }, "x", { desktop: 5 }]);
  if (rnd.chance(100)) e.futureEntryKey = pick(rnd, [{ x: 1.5 }, [1, 2], "z"]);
  if (rnd.chance(80)) e.quarantine = pick(rnd, [{ desktop: { root: 5 } }, "x", { phone: 7 }]);
  if (rnd.chance(40)) e.desktop = pick(rnd, [{ root: { t: "row", id: "r", c: Array.from({ length: 20 }, (_, i) => ({ t: "island", i: `buttons:q${i}` })) } }, { pad: "p".repeat(9000) }, { root: 5 }]);
  return e;
}
function randomDoc(rnd, ids) {
  if (rnd.chance(40)) return pick(rnd, [null, "junk", 5, { format: "something-else" }, { format: "tinhat-layouts", v: 2, layouts: [] }, { format: "tinhat-layouts", v: "1", layouts: [] }, { format: "tinhat-layouts", v: 1, layouts: "x" }]);
  const d = { format: "tinhat-layouts", v: rnd.chance(970) ? 1 : pick(rnd, [1.5, 2, "1", 0]), layouts: Array.from({ length: rnd.range(0, 6) }, () => (rnd.chance(950) ? randomEntry(rnd, ids) : pick(rnd, JUNK))) };
  if (rnd.chance(250)) d.conflicts = Array.from({ length: rnd.range(0, 4) }, () => (rnd.chance(950) ? randomEntry(rnd, ids) : pick(rnd, JUNK)));
  if (rnd.chance(100)) d.futureTop = [1, 2];
  if (rnd.chance(20)) d.layouts = Array.from({ length: rnd.range(60, 75) }, (_, i) => ({ id: `t${i}`, rev: 1 + (i % 5), savedAt: 900 - i, by: "a", vv: { a: 1 }, deleted: true }));
  return d;
}
function randomKnown(rnd, ids) {
  const k = {};
  const n = rnd.range(0, 4);
  for (let i = 0; i < n; i++) {
    const e = randomEntry(rnd, ids);
    k[typeof e.id === "string" && rnd.chance(900) ? e.id : pick(rnd, ["zz", "main"])] = e;
  }
  return k;
}
function randomEdit(rnd) {
  const r = rnd.int(10);
  if (r === 0) return { deleted: true };
  if (r === 1) return pick(rnd, [null, "x", { desktop: {}, phone: 5 }, { name: "N" }]);
  const e = { desktop: { ...presetDesktop(pick(rnd, PRESET_NAMES)), tag: `t${rnd.int(3)}` }, phone: presetPhone(pick(rnd, PRESET_NAMES)) };
  if (rnd.chance(600)) e.name = pick(rnd, ["Main", "Other", "", 5, "n".repeat(41)]);
  if (rnd.chance(100)) e.discardQuarantine = pick(rnd, [true, false, 1]);
  return e;
}
function randomSaveReq(rnd, ids, raw, known) {
  const id = rnd.chance(950) ? pick(rnd, ids) : pick(rnd, ["Bad Id", 5]);
  const cands = [...(raw && Array.isArray(raw.layouts) ? raw.layouts : []), ...Object.values(known)].filter((e) => e && typeof e === "object" && e.id === id);
  const req = { id, edit: randomEdit(rnd), now: rnd.chance(950) ? rnd.range(0, 100) : pick(rnd, [-1, 1.5, "1"]), by: rnd.chance(950) ? pick(rnd, ["a", "b", "dev-z"]) : pick(rnd, [5, "x".repeat(65)]), seq: rnd.chance(950) ? rnd.range(1, 9) : pick(rnd, [0, 2147483648, "2"]) };
  if (cands.length && rnd.chance(600)) req.start = pick(rnd, cands);
  else if (rnd.chance(200)) req.start = randomEntry(rnd, ids);
  if (rnd.chance(150)) req.also = cands.length ? pick(rnd, cands) : randomEntry(rnd, ids);
  if (rnd.chance(100) && req.start && req.start.desktop && typeof req.edit === "object" && req.edit && !req.edit.deleted) {
    req.edit = { name: req.start.name, desktop: req.start.desktop, phone: req.start.phone }; // a no-op session
  }
  return req;
}
function randomSync(rnd) {
  const devices = rnd.chance(500) ? ["a", "b"] : ["a", "b", "c"];
  const steps = [];
  let now = 0;
  const snaps = [];
  const n = rnd.range(8, 40);
  for (let i = 0; i < n; i++) {
    const device = pick(rnd, devices);
    const id = pick(rnd, ["main", "side"]);
    const r = rnd.int(100);
    if (r < 10) steps.push({ device, event: "begin", id });
    else if (r < 30) steps.push({ device, event: "save", id, edit: rnd.chance(900) ? { name: "Main", desktop: { ...presetDesktop("classic"), tag: `${device}${++now}` }, phone: presetPhone("classic") } : randomEdit(rnd), now: ++now });
    else if (r < 34) steps.push({ device, event: "delete", id, now: ++now });
    else if (r < 40) steps.push({ device, event: pick(rnd, ["restore", "dismiss"]), id, now: ++now, ...(rnd.chance(300) ? { pick: pick(rnd, [0, 1, 2, -1, 1.5, "0"]) } : {}) });
    else if (r < 62) steps.push({ device, event: "put" });
    else if (r < 88) steps.push({ device, event: "deliver" });
    else if (r < 94) steps.push({ device, event: "incoming", doc: rnd.chance(700) && snaps.length ? pick(rnd, snaps) : randomDoc(rnd, ["main", "side"]) });
    else if (r < 97) steps.push({ event: "serverPut", doc: rnd.chance(600) && snaps.length ? pick(rnd, snaps) : randomDoc(rnd, ["main", "side"]) });
    else steps.push({ device, event: pick(rnd, ["nonsense", "begin"]), ...(rnd.chance(500) ? {} : { id }) });
    if (rnd.chance(200)) snaps.push(randomDoc(rnd, ["main", "side"]));
  }
  for (const d of devices) steps.push({ device: d, event: "deliver" }, { device: d, event: "put" });
  for (const d of devices) steps.push({ device: d, event: "deliver" });
  const v = { devices, steps };
  if (rnd.chance(200)) v.server = randomDoc(rnd, ["main", "side"]);
  return v;
}
function randomMigrate(rnd) {
  const v = {};
  if (rnd.chance(900)) {
    v.prefs = {};
    if (rnd.chance(500)) v.prefs.nav_mode = pick(rnd, ["corner", "persistent", 5]);
    if (rnd.chance(850)) v.prefs["call-pane-frac"] = rnd.chance(900) ? rnd.int(1300) / 1000 - 0.1 + rnd.int(1000) / 1e7 : pick(rnd, ["0.4", null, 1e300, -0, 0.4255, 0.0005]);
  }
  for (const k of ["H", "L", "chromeH"]) if (rnd.chance(800)) v[k] = rnd.chance(900) ? rnd.range(-50, 17000) : pick(rnd, [1.5, "900", null, 1e20]);
  return v;
}
function draggedFor(rnd, form) {
  const keys = allLocs(form).map((l) => keyOf(l.node)).filter(Boolean);
  const hidden = hiddenOf(form).map((h) => h.i);
  const r = rnd.int(10);
  if (r < 6 && keys.length) return pick(rnd, keys);
  if (r < 8 && hidden.length) return pick(rnd, hidden);
  return pick(rnd, ["nope", "buttons", 5, null, "timeline"]);
}

// ---- the cases ---------------------------------------------------------------------------------------------------
const IDS = ["main", "side", "hand", "l1", "l2"];
for (let c = C0; c < C0 + N; c++) {
  const seed = (S0 + c * 7919) >>> 0;
  const rnd = xorshift32(seed);
  let form = presetDesktop(PRESET_NAMES[c % PRESET_NAMES.length]);
  const nOps = rnd.range(0, 14);
  for (let i = 0; i < nOps; i++) {
    const ctx = randomCtx(rnd, form);
    const op = randomOp(rnd, form, ctx);
    const rec = emit("op", { form, op, ctx });
    if (rec.expect.form !== undefined) form = rec.expect.form;
    if (rnd.chance(250)) {
      const sc = randomCtx(rnd, form);
      emit("solve", { form, size: sc.size, unitPm: sc.unitPm, shape: sc.shape, content: sc.content, view: sc.view });
    }
    if (rnd.chance(40)) emit("dropzones", { form, ctx: randomCtx(rnd, form), dragged: draggedFor(rnd, form) });
  }
  for (let k = 0; k < 3; k++) {
    const sc = randomCtx(rnd, form);
    emit("solve", { form, size: sc.size, unitPm: sc.unitPm, shape: sc.shape, content: sc.content, view: sc.view });
  }
  if (rnd.chance(600)) {
    const bad = corrupt(rnd, JSON.parse(JSON.stringify(form)));
    const text = JSON.stringify(bad);
    emit("repair", { target: "desktop", text });
    emit("validate", { target: "desktop", form: JSON.parse(text) });
  }
  if (rnd.chance(100)) emit("repair", { target: "desktop", text: pick(rnd, ["", "{", "[1,]", "5", "null", '{"root":5}', '{"root":{"t":"island","i":"timeline"}}', JSON.stringify(form).slice(0, rnd.int(200))]) });
  if (rnd.chance(300)) {
    const ph = randomPhone(rnd);
    emit("repair", { target: "phone", text: JSON.stringify(ph) });
    emit("validate", { target: "phone", form: ph });
  }
  if (rnd.chance(150)) {
    const doc = randomDoc(rnd, IDS);
    emit("repair", { target: "document", text: rnd.chance(950) ? JSON.stringify(doc) : "{nope", ...(rnd.chance(700) ? { active: pick(rnd, [...IDS, "zz"]) } : {}) });
  }
  if (rnd.chance(250)) {
    const raw = rnd.chance(800) ? randomDoc(rnd, IDS) : undefined;
    const known = randomKnown(rnd, IDS);
    emit("merge", { ...(raw === undefined ? {} : { raw }), known, ...(rnd.chance(300) ? { justSaved: pick(rnd, IDS) } : {}) });
  }
  if (rnd.chance(250)) {
    const raw = rnd.chance(850) ? randomDoc(rnd, IDS) : null;
    const known = randomKnown(rnd, IDS);
    emit("save", { raw, known, save: randomSaveReq(rnd, IDS, raw, known) });
  }
  if (rnd.chance(60)) emit("sync", randomSync(rnd));
  if (rnd.chance(150)) emit("migrate", randomMigrate(rnd));
}
flush();
void merge;
console.error(`diff-cases: ${N} cases (${C0}…${C0 + N - 1}) from seed ${S0}: ${JSON.stringify(counts)}`);
