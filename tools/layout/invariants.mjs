#!/usr/bin/env node
// invariants.mjs — the INDEPENDENT invariant checker of the layout contract (LAYOUT.md §10.9; ISLANDS-ARCH §6.9).
// Byte-identical in the contract (contract/tools/layout/) and both repos (tools/layout/). It is written WITHOUT the
// solver's code: it reads only the form, the island registry (layout.schema.json), the solve inputs and the output, and
// re-implements only sc() from its one-line definition, so two ports that share a bug still fail here.
//
//   import { checkSolve, checkOpResult } from "./invariants.mjs"      (library)
//   node invariants.mjs DIR|FILE…                                     (CLI: every solve vector's `expect`)
// Schema: $LAYOUT_SCHEMA, else the first of ../../layout.schema.json, ../../src/layout/layout.schema.json,
// ../../crates/layout/layout.schema.json (relative to this file).
import fs from "node:fs";
import path from "node:path";

const HERE = path.dirname(new URL(import.meta.url).pathname);
function loadSchema() {
  const cands = [process.env.LAYOUT_SCHEMA, "../../layout.schema.json", "../../src/layout/layout.schema.json", "../../crates/layout/layout.schema.json"];
  for (const c of cands) {
    if (!c) continue;
    const p = path.isAbsolute(c) ? c : path.join(HERE, c);
    if (fs.existsSync(p)) return JSON.parse(fs.readFileSync(p, "utf8"));
  }
  throw new Error("invariants: layout.schema.json not found");
}
const SCHEMA = loadSchema();
const X = SCHEMA["x-tinhat"];
const REG = X.islands;
const SHAPE = X.shapeDefaults;
const own = (o, k) => (o !== null && typeof o === "object" && Object.prototype.hasOwnProperty.call(o, k) ? o[k] : undefined);
const isInt = (v) => typeof v === "number" && Number.isInteger(v);
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

/** §5.1: sc(v) = sign(v) · floor((|v| · unitPm + 500) / 1000). */
function sc(v, unit) {
  return v < 0 ? -sc(-v, unit) : Math.floor((v * unit + 500) / 1000);
}
const kind = (ref) => ref.split(":")[0];
const known = (ref) => {
  const d = REG[kind(ref)];
  return !!d && Object.prototype.hasOwnProperty.call(REG, kind(ref)) && !!d.multi === ref.includes(":");
};
const compat = (ref) => ref.startsWith("_");
const REQUIRED = Object.keys(REG).filter((k) => REG[k].required === true);

function placedRefs(node) {
  if (!node || typeof node !== "object") return [];
  if (node.t === "island") return typeof node.i === "string" ? [node.i] : [];
  return Array.isArray(node.c) ? node.c.flatMap(placedRefs) : [];
}
function findNode(node, key, parent = null) {
  if (!node || typeof node !== "object") return null;
  const k = node.t === "island" ? node.i : node.t === "row" || node.t === "col" || node.t === "stack" ? node.id : undefined;
  if (k === key) return { node, parent };
  if (node.t === "island" || !Array.isArray(node.c)) return null;
  for (const c of node.c) {
    const r = findNode(c, key, node);
    if (r) return r;
  }
  return null;
}
const inside = (r, b) => r.x >= b.x && r.y >= b.y && r.x + r.w <= b.x + b.w && r.y + r.h <= b.y + b.h;
const overlap = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
const canonish = (v) => JSON.stringify(sortDeep(v));
function sortDeep(v) {
  if (Array.isArray(v)) return v.map(sortDeep);
  if (v && typeof v === "object") return Object.fromEntries(Object.keys(v).sort().map((k) => [k, sortDeep(v[k])]));
  return v;
}

/**
 * Check one solve: `input` = {form, size, unitPm, shape, content, view} as passed to solve, `out` = its output,
 * `out2` (optional) = a second run's output (I6). Returns a list of violations ("I<n> …"); empty = pass.
 */
export function checkSolve(input, out, out2) {
  const errs = [];
  const form = input.form;
  const view = input.view && typeof input.view === "object" ? input.view : {};
  const content = input.content && typeof input.content === "object" ? input.content : {};
  const W = isInt(own(input.size, "w")) ? clamp(input.size.w, 0, 16384) : 0;
  const H = isInt(own(input.size, "h")) ? clamp(input.size.h, 0, 16384) : 0;
  const unit = isInt(input.unitPm) ? clamp(input.unitPm, 500, 4000) : 1000;
  const shapeIn = input.shape && typeof input.shape === "object" ? input.shape : {};
  const SMIN = X.bounds.shapeMin;
  const sh = (k) => Math.max(SMIN[k] ?? 0, isInt(own(form, k)) && (k === "gap" || k === "margin") ? form[k] : isInt(own(shapeIn, k)) ? clamp(shapeIn[k], 0, 8192) : SHAPE[k]);
  const m = Math.min(sc(sh("margin"), unit), Math.floor(W / 8), Math.floor(H / 8));
  const g = sc(sh("gap"), unit);
  const host = { x: m, y: m, w: Math.max(0, W - 2 * m), h: Math.max(0, H - 2 * m) };
  const win = { x: 0, y: 0, w: W, h: H };
  if (out2 !== undefined && canonish(out) !== canonish(out2)) errs.push("I6 not deterministic");
  const cnt = (ref, k) => (isInt(own(own(content, ref), k)) ? clamp(content[ref][k], 0, 16384) : undefined);
  const roomOpen = typeof view.roomOpen === "boolean" ? view.roomOpen : true;
  const popouts = view.popouts && typeof view.popouts === "object" ? view.popouts : {};
  const present = (ref) => {
    if (compat(ref)) return true;
    if (!known(ref)) return false;
    if (Object.prototype.hasOwnProperty.call(popouts, ref) && REG[kind(ref)].popout === true) return false; // popped out
    if (!roomOpen && ["header", "composer", "panel"].includes(ref)) return false; // (the call: callShown alone)
    const p = REG[kind(ref)].presence;
    return { always: true, room: roomOpen, call: view.callShown === true, open: view.panelOpen === true, content: own(own(content, ref), "empty") !== true }[p];
  };
  const regMin = (ref, a) => {
    const d = REG[kind(ref)];
    const oh = own(own(form.opts, ref), "orient") === "h" && d.orientH ? d.orientH : d;
    return Math.max(sc(oh.min[a], unit), cnt(ref, a === "w" ? "minW" : "minH") ?? 0);
  };
  const isl = out.islands;
  const pad = sc(sh("drawerPad"), unit);
  const panelBox = (d) => {
    const p = d.panel;
    const pd = Math.min(pad, Math.floor(p.w / 4), Math.floor(p.h / 4));
    return { x: p.x, y: p.y, w: p.w, h: Math.max(p.h, (d.scroll ?? 0) + 2 * pd) };
  };
  // I1 integers, non-negative sizes; I8 visible => non-empty
  const rects = [];
  for (const [k, r] of Object.entries(isl)) rects.push([`island ${k}`, r]);
  for (const [k, r] of Object.entries(out.stacks ?? {})) rects.push([`stack ${k}`, r]);
  for (const [k, r] of Object.entries(out.containers ?? {})) rects.push([`container ${k}`, r]);
  for (const d of out.drawers) rects.push([`drawer ${d.id} handle`, d.handle], [`drawer ${d.id} panel`, d.panel]);
  for (const gp of out.gaps) rects.push([`gap ${gp.id}`, gp]);
  for (const [name, r] of rects) {
    for (const f of ["x", "y", "w", "h"]) if (!isInt(r[f])) errs.push(`I1 ${name}.${f} not an integer`);
    if (r.w < 0 || r.h < 0) errs.push(`I1 ${name} negative size`);
  }
  for (const [k, r] of Object.entries(isl)) {
    if (!isInt(r.z)) errs.push(`I1 island ${k}.z not an integer`);
    if (r.vis && (r.w <= 0 || r.h <= 0)) errs.push(`I8 ${k} visible but empty`);
  }
  // I2 containment
  const panels = out.drawers.map(panelBox);
  for (const [k, r] of Object.entries(isl)) {
    if (r.w <= 0 || r.h <= 0) continue;
    if (r.z === 0 || r.z === 2 || (r.z >= 10 && r.z < 20)) {
      if (!inside(r, host)) errs.push(`I2 ${k} ${JSON.stringify(r)} outside the host ${JSON.stringify(host)}`);
    } else if (r.z === 20) {
      if (!panels.some((p) => inside(r, p))) errs.push(`I2 ${k} outside every drawer panel (incl. its scroll length)`);
    } else errs.push(`I2 ${k} has an unknown layer z ${r.z}`);
  }
  // I3 visible docked islands never overlap
  const vis0 = Object.entries(isl).filter(([, r]) => r.vis && r.z === 0);
  for (let i = 0; i < vis0.length; i++)
    for (let j = i + 1; j < vis0.length; j++) if (overlap(vis0[i][1], vis0[j][1])) errs.push(`I3 ${vis0[i][0]} overlaps ${vis0[j][0]}`);
  // I5 every present placed island is in the output (islands under an applied `hide` step excepted); I10 maximised
  const floats = Array.isArray(form.floats) ? form.floats : [];
  const floatedRefs = floats.flatMap((f) => placedRefs(f.node));
  const exp = [...placedRefs(form.root), ...floatedRefs].filter(present);
  const hiddenByStep = new Set();
  const stepOf = (c) => [c.slice(0, c.lastIndexOf(":")), c.slice(c.lastIndexOf(":") + 1)]; // refs may hold a ':'
  for (const c of out.collapsed) {
    const [ref, to] = stepOf(c);
    if (to !== "hide") continue;
    const f = findNode(form.root, ref);
    for (const r of f ? placedRefs(f.node) : [ref]) hiddenByStep.add(r);
  }
  const mx = typeof view.maximized === "string" && known(view.maximized) && REG[kind(view.maximized)].maximizable === true ? view.maximized : null;
  const maxed = mx !== null && exp.includes(mx);
  if (maxed) {
    if (Object.keys(isl).length !== 1 || !isl[mx]) errs.push("I10 maximised: not exactly the maximised island");
    else if (canonish(isl[mx]) !== canonish({ x: host.x, y: host.y, w: host.w, h: host.h, z: 0, vis: host.w > 0 && host.h > 0 }))
      errs.push("I10 maximised island is not the host rect");
    if (out.drawers.length) errs.push("I10 maximised: drawers listed");
  } else {
    for (const r of exp) if (!isl[r] && !hiddenByStep.has(r)) errs.push(`I5 present island ${r} missing`);
  }
  for (const k of Object.keys(isl)) if (!exp.includes(k)) errs.push(`I5 ${k} listed but not a present placed island`);
  // I4 required content
  const E = out.emergency === true;
  if (!maxed) {
    for (const r of REQUIRED) {
      if (!exp.includes(r)) continue;
      const o = isl[r];
      if (!o) {
        errs.push(`I4 required ${r} missing`);
        continue;
      }
      if (!E && (o.w < regMin(r, "w") || o.h < regMin(r, "h"))) errs.push(`I4 ${r} ${o.w}x${o.h} below its floor outside the emergency arrangement`);
    }
    if (E && !out.tooSmall) {
      const compFloor = exp.includes("composer") ? Math.max(regMin("composer", "h"), cnt("composer", "h") ?? 0) : 0;
      // the call's usable minimum: its bar, at least the registry's `bar` (a measurement cannot shrink it)
      const callB = Math.max(sc(REG.call.bar, unit), cnt("call", "barH") ?? 0);
      if (exp.includes("call")) {
        const c = isl.call;
        if (!c || c.h < callB) errs.push(`I4 emergency: the call's controls below their usable height (${c && c.h} < ${callB})`);
      }
      if (exp.includes("composer")) {
        const c = isl.composer;
        const room = host.h >= (exp.includes("call") ? callB + g : 0) + compFloor;
        if (room && (!c || c.h < compFloor)) errs.push(`I4 emergency: composer missing or below ${compFloor} with room for it`);
      }
    }
    // I11 the required controls are never covered: no visible overlay (z 2) or always-float (z 10-19, not a hover
    // float) intersects the visible composer or the visible call (docked or floating); drawers and hover floats open on
    // demand and are exempt
    const hoverZ = new Set(floats.map((f, i) => (f && f.reveal === "hover" ? 10 + i : -1)));
    const callZ = isl.call ? isl.call.z : -1;
    const ctl = ["composer", "call"].filter((r) => isl[r] && isl[r].vis).map((r) => [r, isl[r]]);
    for (const [k, r] of Object.entries(isl)) {
      if (!r.vis || r.z < 2 || r.z >= 20 || hoverZ.has(r.z)) continue;
      for (const [cr, c] of ctl) if (k !== cr && !(cr === "call" && r.z === callZ) && overlap(r, c)) errs.push(`I11 ${k} covers the ${cr}`);
    }
    if (E) {
      // the emergency column is full width, docked; its only z-2 island is the panel, inside the timeline's rect
      for (const r of ["header", "call", "timeline", "composer"])
        if (isl[r] && isl[r].vis && (isl[r].x !== host.x || isl[r].w !== host.w || isl[r].z !== 0)) errs.push(`I11 ${r} is not a full-width column island`);
      if (out.gaps.length) errs.push("I11 gap handles listed in the emergency arrangement");
      for (const [k, r] of Object.entries(isl)) {
        if (r.z !== 2) continue;
        if (k !== "panel") errs.push(`I11 ${k} at z 2 in the emergency arrangement`);
        else if (r.vis && !(isl.timeline && isl.timeline.vis && inside(r, isl.timeline))) errs.push("I11 the emergency panel is not inside the timeline's rect");
      }
      for (const [k, r] of Object.entries(isl)) if (r.z >= 10 && r.z < 20) errs.push(`I11 ${k} at a float layer (z ${r.z}) in the emergency arrangement`);
    }
  }
  // I9 drawer chrome inside the window; I12 one open drawer; I14 handles never overlap; I15 chips never cover docked
  // (an overflow chip is a handle in its container's layer: docked / overlay / float chips inside the host, a chip in a
  // drawer inside that drawer's panel incl. its scroll length)
  for (const d of out.drawers) {
    if (!inside(d.panel, win)) errs.push(`I9 drawer ${d.id} panel ${JSON.stringify(d.panel)} outside the window`);
    if (d.side !== "chip") {
      if (!inside(d.handle, win)) errs.push(`I9 drawer ${d.id} handle ${JSON.stringify(d.handle)} outside the window`);
    } else if (d.z === 20) {
      if (!out.drawers.some((p) => p !== d && inside(d.handle, panelBox(p)))) errs.push(`I9 chip ${d.id} outside every drawer panel`);
    } else if (!inside(d.handle, host)) errs.push(`I9 chip ${d.id} ${JSON.stringify(d.handle)} outside the host`);
  }
  const opened = out.drawers.filter((d) => d.open).map((d) => d.id);
  if (opened.length > 1) errs.push(`I12 several drawers open ${opened}`);
  // (collapse drawers only: a hover float's drawer has its anchor as `side` — `l` / `r` too — and its id is a float id)
  const floatIds = new Set(floats.map((f) => f.id));
  const hl = out.drawers.filter((d) => (d.side === "l" || d.side === "r") && !floatIds.has(d.id)).map((d) => d.handle);
  for (let i = 0; i < hl.length; i++) for (let j = i + 1; j < hl.length; j++) if (overlap(hl[i], hl[j])) errs.push("I14 drawer handles overlap");
  const chips = out.drawers.filter((d) => d.side === "chip");
  for (const d of chips) if (d.z === 0) for (const [k, r] of vis0) if (overlap(d.handle, r)) errs.push(`I15 overflow chip ${d.id} covers ${k}`);
  // I13 every squeezed island sits in an overflow chip's drawer
  for (const r of out.squeezed ?? []) {
    const o = isl[r];
    if (!o || o.z !== 20 || !chips.some((d) => inside(o, panelBox(d)))) errs.push(`I13 squeezed ${r} not inside an overflow chip's drawer`);
  }
  // I17 stacks: the active tab is one of the tabs, the strip fits
  for (const [k, s] of Object.entries(out.stacks ?? {})) {
    if (!s.tabs.includes(s.active)) errs.push(`I17 stack ${k} active ${s.active} not a tab`);
    if (s.tab > s.h) errs.push(`I17 stack ${k} strip taller than the stack`);
  }
  // I18 an overflow panel is a plain list: the drawers with side "chip" take `squeezed` in runs of `count`, in order;
  // each run's islands stack from the panel's padded top with the form gap between them, full inner width, each at
  // least its registry minimum height (nothing inside is squeezed again), visible exactly while the drawer is open
  {
    const sq = out.squeezed ?? [];
    let at = 0;
    for (const d of chips) {
      const refs = sq.slice(at, at + (isInt(d.count) ? d.count : 0));
      at += refs.length;
      if (refs.length !== d.count) {
        errs.push(`I18 overflow drawer ${d.id} counts ${d.count} but ${refs.length} squeezed refs remain`);
        continue;
      }
      const p = d.panel;
      const pd = Math.min(pad, Math.floor(p.w / 4), Math.floor(p.h / 4));
      const iw = Math.max(0, p.w - 2 * pd);
      const ih = Math.max(0, p.h - 2 * pd);
      let y = p.y + pd;
      let listH = 0;
      refs.forEach((ref, i) => {
        const o = isl[ref];
        if (!o) return errs.push(`I18 ${ref} of ${d.id} not listed`);
        if (o.z !== 20 || o.x !== p.x + pd || o.w !== iw || o.y !== y) errs.push(`I18 ${ref} is not in ${d.id}'s list slot (${o.x},${o.y},${o.w}) vs (${p.x + pd},${y},${iw})`);
        if (o.h < regMin(ref, "h")) errs.push(`I18 ${ref} squeezed below its minimum inside ${d.id} (${o.h} < ${regMin(ref, "h")})`);
        if (o.vis !== (iw > 0 && o.h > 0 && d.open === true)) errs.push(`I18 ${ref} visibility does not follow ${d.id}`);
        y += o.h + g;
        listH += o.h + (i > 0 ? g : 0);
      });
      if (listH > ih ? d.scroll !== listH : d.scroll !== undefined) errs.push(`I18 ${d.id} scroll ${d.scroll} for a list of ${listH} in ${ih}`);
    }
    if (at !== sq.length) errs.push(`I18 ${sq.length - at} squeezed refs in no overflow drawer`);
  }
  // I19 usable chrome (x-tinhat.bounds.shapeMin): a tab strip, a drawer handle and a stored drawer width never shrink
  // below their usable minimum unless the window itself is smaller
  {
    const hx = Math.min(Math.max(m, 4), Math.floor(W / 8), Math.floor(H / 8));
    const hsMin = Math.max(0, Math.min(sc(SMIN.handle, unit), Math.floor((W - 2 * hx - 4) / 2), H - 2 * hx));
    for (const [k, s] of Object.entries(out.stacks ?? {})) if (s.tab < Math.min(sc(SMIN.tab, unit), s.h)) errs.push(`I19 stack ${k} tab strip ${s.tab} below the usable minimum`);
    const stepW = new Map();
    for (const st of Array.isArray(form.collapse) ? form.collapse : []) if (st && typeof st.ref === "string" && !stepW.has(st.ref)) stepW.set(st.ref, st.w);
    for (const d of out.drawers) {
      if (d.side === "chip") continue;
      if (d.handle.w !== d.handle.h || d.handle.w < hsMin) errs.push(`I19 drawer ${d.id} handle ${d.handle.w}x${d.handle.h} below the usable ${hsMin}`);
      if (floatIds.has(d.id)) continue; // a hover float's panel is the float itself
      const room = Math.max(0, W - 2 * hx);
      if (room > 0 && d.panel.w <= 0) errs.push(`I19 drawer ${d.id} has an empty panel`);
      if (!d.merged && isInt(stepW.get(d.id)) && d.panel.w < Math.min(sc(SMIN.drawerW, unit), room)) errs.push(`I19 drawer ${d.id} panel ${d.panel.w} narrower than the usable minimum`);
    }
  }
  // I16 no form step that is not applied has a true `below` condition in the final geometry
  if (!E && !maxed) {
    const applied = new Set(out.collapsed.map((c) => stepOf(c)[0]));
    const forced = new Set(Array.isArray(view.forceCollapse) ? view.forceCollapse : []);
    for (const st of Array.isArray(form.collapse) ? form.collapse : []) {
      if (applied.has(st.ref) || forced.has(st.ref) || !isInt(st.below)) continue;
      const f = findNode(form.root, st.ref);
      if (!f) continue;
      if (placedRefs(f.node).some((r) => ["header", "timeline", "composer", "call"].includes(r))) continue; // unsafe: never applied
      const nodeHere = f.node.t === "island" ? isl[st.ref] && isl[st.ref].z === 0 && !(out.squeezed ?? []).includes(st.ref) : own(out.containers, st.ref)?.z === 0 || own(out.stacks, st.ref)?.z === 0;
      if (!nodeHere) continue;
      const P = f.parent;
      let prect;
      if (!P || P === form.root) prect = host;
      else {
        prect = own(out.containers, P.id) ?? own(out.stacks, P.id);
        if (!prect || prect.z !== 0) continue; // the parent is not laid out docked (collapsed away or squeezed)
      }
      const len = P && P.t === "row" ? prect.w : prect.h;
      if (len < sc(st.below, unit)) errs.push(`I16 step ${st.ref}:${st.to} not applied although its parent is ${len} < ${sc(st.below, unit)}`);
    }
  }
  return errs;
}

/** Multiset of island refs across root, floats and hidden. */
export function islandMultiset(form) {
  const refs = [...placedRefs(form.root), ...(Array.isArray(form.floats) ? form.floats.flatMap((f) => placedRefs(f.node)) : []), ...(Array.isArray(form.hidden) ? form.hidden.map((h) => (typeof h === "string" ? h : h.i)) : [])];
  return refs.sort();
}
/** An operation's properties (§10.9): the island multiset is preserved by every successful non-preset operation. */
export function checkOpResult(before, op, res) {
  const errs = [];
  if (!res.ok) return errs;
  if (op.op === "applyPreset" || op.op === "reset") return errs;
  if (JSON.stringify(islandMultiset(before)) !== JSON.stringify(islandMultiset(res.form))) errs.push(`O1 ${op.op}: the island multiset changed`);
  return errs;
}

// ---- CLI ---------------------------------------------------------------------------------------------------------
function* vectorFiles(p) {
  const st = fs.statSync(p);
  if (st.isDirectory()) for (const f of fs.readdirSync(p).sort()) yield* vectorFiles(path.join(p, f));
  else if (p.endsWith(".json")) yield p;
}
if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(new URL(import.meta.url).pathname)) {
  const args = process.argv.slice(2);
  let n = 0;
  let bad = 0;
  const presets = X.presets;
  for (const a of args) {
    for (const f of vectorFiles(a)) {
      const v = JSON.parse(fs.readFileSync(f, "utf8"));
      if (v.kind !== "solve") continue;
      const form = v.form ?? (v.preset ? presets[v.preset]?.desktop ?? X.internalPresets[v.preset]?.desktop : undefined);
      const errs = checkSolve({ form, size: v.size, unitPm: v.unitPm, shape: v.shape, content: v.content, view: v.view }, v.expect);
      n++;
      if (errs.length) {
        bad++;
        console.log(`FAIL ${path.basename(f)}: ${errs.slice(0, 3).join("; ")}`);
      }
    }
  }
  console.log(`invariants: ${n} solve vectors, ${bad} failing`);
  process.exit(bad ? 1 : 0);
}
