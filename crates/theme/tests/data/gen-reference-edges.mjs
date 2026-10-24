#!/usr/bin/env node
// gen-reference-edges.mjs — exact outputs of the TypeScript REFERENCE on edge inputs → reference-edges.json, which
// tests/unit.rs compares bit for bit (doubles as IEEE-754 big-endian hex): cbrt, OKLab both ways, sRGB encode on the
// thresholds, luminance/contrast, mixok/mix, Qt lighter/darker, over, guard, best, the slugify fold (the TS unit
// test's NFD rule), ui/color.ts itself (+ its Qt 6 16 → 8-bit step, PORT-QUESTIONS Q5), Math.round (ENGINE-QUESTIONS
// A4), the #rrggbbaa alpha, the JavaScript regex's
// answers for isMxcUri (the web unit test's fixed and random strings), the color.ts values the web's engine tests
// compare the built-ins and mix/lighter with, and the synthetic token spec of the edge vectors.
//
// Regenerate whenever the reference maths change, from a checkout of the reference commit, e.g.
//   git -C <a tinhat-web checkout> archive <commit> src/theme src/ui/color.ts tools/theme-lib | tar -x -C /tmp/ref
//   cd /tmp/ref && node --no-warnings --import ./tools/theme-lib/register.mjs \
//     <desktop>/crates/theme/tests/data/gen-reference-edges.mjs "$PWD" > <desktop>/crates/theme/tests/data/reference-edges.json
import path from "node:path";
import fs from "node:fs";
const web = process.argv[2] ?? process.cwd();
const T = (f) => import(path.join(web, "src/theme", f));
const { cbrt, toOklab, fromOklab, linearToSrgb, luminance, contrastOfY, mixok, chroma } = await T("maths.ts");
const { parseLiteral, formatColour, alphaK } = await T("literal.ts");
const { mixC, lighterC, darkerC, overC, cr, guardC, bestC } = await T("ops.ts");
const { slugify } = await T("import.ts");
const colorTs = await import(path.join(web, "src/ui/color.ts")); // Qt's pixels: what mix/lighter/darker/over must equal
const tables = JSON.parse(fs.readFileSync(path.join(web, "src/theme/engine-tables.json"), "utf8"));
const bits = (x) => { const b = Buffer.alloc(8); b.writeDoubleBE(x); return "0x" + b.toString("hex"); };
const lit = (s) => parseLiteral(s);
const out = {};
const nextUp = (x) => { const b = Buffer.alloc(8); b.writeDoubleBE(x); b.writeBigUInt64BE(b.readBigUInt64BE() + 1n); return b.readDoubleBE(); };
const cbrtIn = [0, 5e-324, 1e-300, 1e-10, 0.001, 0.008, 0.125, 0.2140818823528471, 0.5, 0.9999999999999999, 1, 1.0000000000000002, 2, 8, 27, 1e10, 1e300, 1.7976931348623157e308, -0.125, -8];
out.cbrt = cbrtIn.map((x) => [bits(x), bits(cbrt(x))]);
const cols = ["#000000", "#ffffff", "#808080", "#ff0000", "#00ff00", "#0000ff", "#010101", "#fefefe", "#1e6a76", "#e0a020", "#3cbad0", "#123456"];
out.oklab = cols.map((c) => { const l = toOklab(lit(c)); return [c, bits(l.L), bits(l.A), bits(l.B), bits(chroma(lit(c))), bits(luminance(lit(c)))]; });
const labs = [[0, 0, 0], [1, 0, 0], [0.5, 0.1, -0.1], [2, 0, 0], [-1, 0, 0], [0.7, 0.4, 0.4], [0.9999999935, 0, 3.7e-8], [0.62, -0.2, 0.15], [0.3, 0, -0.3]];
out.fromOklab = labs.map(([L, A, B]) => [bits(L), bits(A), bits(B), fromOklab(L, A, B)]);
const mid = tables.ENC_MID;
const encIn = [0, -1, -0, 1e-10, mid[0], nextUp(mid[0]), mid[127], nextUp(mid[127]), mid[254], nextUp(mid[254]), 0.5, 1, 2, Infinity, NaN, 0.21404114048223255];
out.encode = encIn.map((x) => [bits(x), linearToSrgb(x)]);
const pairs = [["#000000", "#ffffff"], ["#ff0000", "#0000ff"], ["#1e6a76", "#ffffff"], ["#282c34", "#abb2bf"], ["#e0a020/0.5", "#000000"], ["#ffffff/0.0001", "#123456/0.9999"]];
const ts = [0, 0.0001, 0.06, 0.25, 0.5, 0.6, 0.9999, 1, 0.123456789, 0.4999999999999999];
out.mixok = []; out.mix = [];
for (const [a, b] of pairs) for (const t of ts) { out.mixok.push([a, b, bits(t), formatColour(mixok(lit(a), lit(b), t))]); out.mix.push([a, b, bits(t), formatColour(mixC(lit(a), lit(b), t))]); }
const qcols = ["#000000", "#ffffff", "#0a0a0a", "#1e6a76", "#ff8000", "#808080", "#e0a020/0.5", "#010203", "#fefdfc", "#3cbad0", "#c74848"];
const factors = [0, 0.004, 0.005, 0.5, 0.99, 1, 1.005, 1.1, 1.18, 1.2, 1.5, 2, 3, 5, 99.5, 9999.9999];
out.qt = [];
for (const c of qcols) for (const f of factors) out.qt.push([c, bits(f), formatColour(lighterC(lit(c), f)), formatColour(darkerC(lit(c), f))]);
const overs = [["#ff0000/0.5", "#0000ff"], ["#ff0000/0.5", "#0000ff/0.2"], ["#ffffff/0.0001", "#000000"], ["#123456", "#ffffff"], ["#808080/0.3333", "#fefefe"], ["#e0a020/0", "#101010"]];
out.over = overs.map(([a, b]) => [a, b, formatColour(overC(lit(a), lit(b))), bits(cr(lit(a), lit(b)))]);
const guards = [["#6f757f", "#282c34", 4.5], ["#2e2e2e", "#1c1c1c", 4.5], ["#d6d6d6", "#eeeeee", 3], ["#777777", "#777777", 21], ["#ff0000/0.3", "#ffffff", 4.5], ["#808080", "#7f7f7f", 1], ["#e0a020", "#ffffff", 3]];
out.guard = guards.map(([a, b, n]) => [a, b, bits(n), formatColour(guardC(lit(a), lit(b), n))]);
out.best = [["#808080", ["#7f7f7f", "#818181"]], ["#1e6a76", ["#ffffff", "#282c34", "#abb2bf", "#000000"]], ["#ffff00", ["#ffffff", "#000000"]]].map(([bg, cs]) => [bg, cs, formatColour(bestC(lit(bg), cs.map(lit)))]);
out.contrast = [["#ffffff", "#000000"], ["#777777", "#777777"], ["#e0e0e0", "#0a0a0a"], ["#abb2bf", "#282c34"]].map(([a, b]) => [a, b, bits(contrastOfY(luminance(lit(a)), luminance(lit(b))))]);
// slugify fold (the TS unit test's NFD rule) for U+00C0..U+017F
const special = { 0xc6: "ae", 0xe6: "ae", 0xd0: "d", 0xf0: "d", 0xd8: "o", 0xf8: "o", 0xde: "th", 0xfe: "th", 0xdf: "ss", 0xd7: "", 0xf7: "", 0x110: "d", 0x111: "d", 0x126: "h", 0x127: "h", 0x131: "i", 0x132: "ij", 0x133: "ij", 0x138: "k", 0x13f: "l", 0x140: "l", 0x141: "l", 0x142: "l", 0x149: "n", 0x14a: "n", 0x14b: "n", 0x152: "oe", 0x153: "oe", 0x166: "t", 0x167: "t", 0x17f: "s" };
out.fold = [];
for (let cp = 0xc0; cp <= 0x17f; cp++) { const ch = String.fromCodePoint(cp); const want = cp in special ? special[cp] : ch.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase(); out.fold.push(want); if (slugify("x" + ch + "y") !== (want ? `x${want}y` : "x-y")) throw new Error("fold " + cp); }
// ui/color.ts itself (opaque inputs; the engine carries alpha separately): mix, qtLighter, qtDarker, over
out.colorTs = [];
for (const c of ["#1e6a76", "#0a0a0a", "#ff8000", "#808080", "#fefdfc", "#3cbad0"]) {
  for (const f of [0.3, 1.18, 1.5, 0.5, 2, 99.5]) {
    out.colorTs.push([c, bits(f), colorTs.mix(c, "#ffffff", Math.min(f, 1)), colorTs.qtLighter(c, f), colorTs.qtDarker(c, f)]);
  }
}
// Math.round (ENGINE-QUESTIONS A4): the port's round must give these bits for x >= 0 (negatives: the same number;
// JavaScript returns -0 where the port returns +0 — every caller clamps at 0)
const roundIn = [0, 0.49999999999999994, 0.5, 0.5000000000000001, 1.4999999999999998, 1.5, 2.5, 2.4999999999999996,
  117.99999999999999, 65534.5, 4503599627370495.5, 4503599627370496, 4503599627370497, 9007199254740991, 9007199254740992,
  1e300, 5e-324, 1.7976931348623157e308, Infinity, 0.1 * 3 * 10000, 0.18 * 10000, 45.9, 127.5, 254.99999999999997,
  -0.5, -1.5, -2.5, -0.49999999999999994, -0.3, -2.5000000000000004, -0, -1e-300];
out.round = roundIn.map((x) => [bits(x), bits(Math.round(x))]);
// #rrggbbaa: k = round(aa × 10000 / 255) for every aa
out.alphaAA = Array.from({ length: 256 }, (_, aa) => alphaK(lit("#102030" + aa.toString(16).padStart(2, "0"))));
// isMxcUri ≡ /^mxc:\/\/[A-Za-z0-9.:-]{1,255}\/[A-Za-z0-9_-]{1,255}$/: the regex's answers on the web unit test's
// fixed strings, and on its 20,000 strings from mulberry32(4) (as '0'/'1', in order)
const RE = /^mxc:\/\/[A-Za-z0-9.:-]{1,255}\/[A-Za-z0-9_-]{1,255}$/;
const fixed = ["mxc://a/b", "mxc://example.com/AbC_d-9", "mxc://a:8448/b", "mxc:///b", "mxc://a/", "mxc://a/b/c", "mxc://a b/c",
  "MXC://a/b", "mxc://a/b\n", "\nmxc://a/b", " mxc://a/b", "mxc:/a/b", "mxc://é/b", "mxc://a/é", "mxc://a/b.c", "mxc://a_b/c",
  `mxc://${"s".repeat(255)}/${"m".repeat(255)}`, `mxc://${"s".repeat(256)}/m`, `mxc://s/${"m".repeat(256)}`];
function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const r4 = mulberry32(4);
const alpha = ["m", "x", "c", ":", "/", ".", "-", "_", "a", "Z", "9", " ", "\n", "é", "M"];
let bitsRandom = "";
for (let i = 0; i < 20000; i++) {
  let s = r4() < 0.7 ? "mxc://" : "";
  const n = Math.floor(r4() * 12);
  for (let j = 0; j < n; j++) s += alpha[Math.floor(r4() * alpha.length)];
  bitsRandom += RE.test(s) ? "1" : "0";
}
out.mxc = { fixed: fixed.map((s) => [s, RE.test(s)]), alphabet: alpha, random: bitsRandom };
// what the web's engine tests compare with (color.ts = today's Qt pixels)
out.colorTsEngine = { mix: colorTs.mix("#1e6a76", "#ffffff", 0.3), lighter: colorTs.qtLighter("#0a0a0a", 1.18) };
out.builtinHover = { oled: colorTs.qtLighter("#0a0a0a", 1.18), goose: colorTs.qtLighter("#2a1030", 1.18) };
// PORT-QUESTIONS Q5: color.ts (= the engine's lighter/darker) rounds 16 → 8 bits like Qt 6 — its values for the unit test
out.q5 = { colorTs: [colorTs.qtLighter("#1e6a76", 1.15), colorTs.qtDarker("#000009", 1.2)] };
// the edge vectors' small synthetic token spec (tools/theme-lib/synthetic-tokens.mjs), for the "never throws" mirror
out.syn = (await import(path.join(web, "tools/theme-lib/synthetic-tokens.mjs"))).VECTOR_TOKENS;
console.log(JSON.stringify(out));
