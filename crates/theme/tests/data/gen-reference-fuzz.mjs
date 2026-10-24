#!/usr/bin/env node
// gen-reference-fuzz.mjs — differential cases for the Rust port (tinhat-theme): deterministic pseudo-random inputs and
// the TypeScript REFERENCE's exact outputs → reference-fuzz.json, replayed by tests/reference.rs (the private document
// parsers by the unit test in src/parsers.rs). The shared test vectors are the contract; these cases add breadth where
// the vectors are thin: odd expressions and literals, mutated and overridden themes (every refusal path, cycles,
// guards), resolveForApp on edited built-ins, generated files of every import format with noise, the JSONC / plist
// / TOML / YAML parsers on their own, and theme JSON text (parseThemeJson). Keys named like Object.prototype members
// (__proto__, constructor …), 125–131 nesting levels and lone-surrogate escapes appear throughout (ENGINE.md §9,
// PORT-QUESTIONS Q1–Q3); the generator itself writes such keys as OWN properties (setOwn), never with `o[k] = v`.
// So do YAML collections that become text (flow-mapping keys, continuation lines: YAML_STRINGIFY), where the
// reference's String() of its null-prototype documents threw until web d18d688.
//
// Regenerate whenever the reference changes, from a checkout of the reference commit, e.g.
//   git -C <a tinhat-web checkout> archive <commit> src/theme src/ui/color.ts tools/theme-lib | tar -x -C /tmp/ref
//   cd /tmp/ref && REF_COMMIT=<commit> node --no-warnings --import ./tools/theme-lib/register.mjs \
//     <desktop>/crates/theme/tests/data/gen-reference-fuzz.mjs "$PWD" > <desktop>/crates/theme/tests/data/reference-fuzz.json
// Options: --scale N (N × the default case counts; the committed file uses 1 — for a big one-off run write the file
// elsewhere and point THEME_REFERENCE_FUZZ at it), --seed S (default 20260926). Sections (default counts): expr 1,204,
// literal 800, decimal 400, mutate 1,500, overrides 400, limits 300 (the §4 limits: 8 KB boundary, depth 16/17 and
// 128/129 via {"$nest": [n, leaf]}, 80 code points, URL rule, paths, backdrop), app 46, import 300, parsers 250,
// themeJson 150. The file itself is JSON both sides can read: no lone surrogate (only as escape TEXT), shallow.
//
// Results are canonical compact JSON (canon.ts canonJson; the parsers: JSON.stringify in JavaScript's own key order,
// TOML bare values as {"$raw": …}) or, when longer than 160 bytes, "sha256:" + the first 24 hex digits of its SHA-256.
import path from "node:path";
import fs from "node:fs";
import { createHash } from "node:crypto";

const argv = process.argv.slice(2);
const web = argv[0] && !argv[0].startsWith("--") ? argv[0] : process.cwd();
const opt = (name, dflt) => (argv.includes(name) ? Number(argv[argv.indexOf(name) + 1]) : dflt);
const SCALE = opt("--scale", 1);
const SEED = opt("--seed", 20260926);
const T = (f) => import(path.join(web, "src/theme", f));
const { parseExpr, exprText } = await T("expr.ts");
const { parseLiteral, formatColour, parseDecimal4 } = await T("literal.ts");
const { resolveTheme, resolveForApp, parseThemeJson, ROLE_IDS, BUILTINS } = await T("engine.ts");
const { importTheme } = await T("import.ts");
const { parseJsonc, parsePlist, parseToml, parseYaml, RawToken, ParseError } = await T("parsers.ts");
const { canonJson } = await T("canon.ts");
const { utf8Length } = await T("limits.ts");
const LIB = JSON.parse(fs.readFileSync(path.join(web, "src/theme/themes-library.json"), "utf8"));
const { VECTOR_TOKENS: SYN } = await import(path.join(web, "tools/theme-lib/synthetic-tokens.mjs"));
const SPEC = JSON.parse(fs.readFileSync(path.join(web, "src/theme/tokens.json"), "utf8"));

// ---- deterministic randomness (mulberry32, as the reference unit tests) --------------------------------------------
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
let R = mulberry32(SEED); // swapped for the replayed "mutate" section
const int = (n) => Math.floor(R() * n);
const pick = (a) => a[int(a.length)];
const chance = (p) => R() < p;
const clone = (v) => (v === undefined ? undefined : JSON.parse(JSON.stringify(v)));
/** An OWN key (a key named __proto__, constructor … is ordinary data, as in the port; `o[k] = v` would set the prototype). */
const setOwn = (o, k, v) => {
  Object.defineProperty(o, k, { value: v, enumerable: true, writable: true, configurable: true });
  return o;
};
const assignOwn = (target, src) => {
  for (const k of Object.keys(src)) setOwn(target, k, src[k]);
  return target;
};
const INHERITED_NAMES = ["__proto__", "constructor", "toString", "valueOf", "hasOwnProperty", "isPrototypeOf", "__defineGetter__"];
const IN = () => pick(INHERITED_NAMES);
/** `\u` escapes as JSON TEXT (the corpus itself never holds a lone surrogate) */
const ESCAPES = ["\\ud800", "\\udc00", "\\ud83c\\udfa8", "\\udbff\\udfff", "\\ud800\\u0041", "\\udc00\\ud800", "\\uD83D\\uDE00", "\\udbff"];
/** n nested arrays around a leaf (JSON text) */
const nestText = (n, leaf = "0") => "[".repeat(n) + leaf + "]".repeat(n);
const sha = (s) => createHash("sha256").update(s, "utf8").digest("hex");
const out = (v) => {
  const s = canonJson(v);
  return s.length > 160 ? "sha256:" + sha(s).slice(0, 24) : s;
};
function shuffle(a) {
  for (let i = a.length - 1; i > 0; i--) {
    const j = int(i + 1);
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
}
// Edits by code point, never inside a surrogate pair (the Rust side cannot hold a lone surrogate).
function insertAt(s, text) {
  const cps = [...s];
  const p = int(cps.length + 1);
  return cps.slice(0, p).join("") + text + cps.slice(p).join("");
}
const HEXD = "0123456789abcdef";
function hexs(n, mixedCase) {
  let s = "";
  for (let i = 0; i < n; i++) {
    const d = HEXD[int(16)];
    s += mixedCase && chance(0.5) ? d.toUpperCase() : d;
  }
  return s;
}

// ---- expressions ------------------------------------------------------------------------------------------------------
const FNS = ["alpha", "mix", "lighter", "darker", "over", "mixok", "best", "guard"];
const NUMS = ["0", "1", "0.5", ".18", "0.0001", "0.9999", "1.18", "2", "21", "3", "4.5", "7", "9999.9999", "1.0001",
  "12345", "0.12345", "1.", ".", "00.5", "1e2", "10000", "21.0001", "1.5", "0.3", "0.004", "0.005", "99.5", "1.005"];
const REFS = [...ROLE_IDS, "bg", "fg", "cursor", "selectionBg", "selectionFg", ...Array.from({ length: 16 }, (_, i) => `c${i}`),
  ...SYN.tokens.map((t) => t.id), "c16", "nope", "x:y-z", "Accent.base", "name.13"];
function genLiteral() {
  return pick([
    () => "#" + hexs(6),
    () => "#" + hexs(3, true),
    () => "#" + hexs(8),
    () => "#" + hexs(6) + "/" + pick(NUMS),
    () => "#" + hexs(6, true) + "/" + pick(["0", "1", ".5", "0.25", "1.0000", "0.00001", "0.1234"]),
    () => "#" + hexs(pick([2, 4, 5, 7, 9])),
  ])();
}
const genGoodLiteral = () => pick([() => "#" + hexs(6), () => "#" + hexs(3, true), () => "#" + hexs(8), () => "#" + hexs(6) + "/" + pick(["0", "1", ".5", "0.25", "0.1234", "0.0001"])])();
const SLOT_IDS = ["bg", "fg", "cursor", "selectionBg", "selectionFg", ...Array.from({ length: 16 }, (_, i) => `c${i}`)];
const ROLE_REFS = [...ROLE_IDS, ...SLOT_IDS]; // what a role expression of a palette theme may name
const TOKEN_REFS = [...ROLE_IDS, ...SYN.tokens.map((t) => t.id)]; // what a token expression may name
function genExpr(depth, good = false, refs = REFS) {
  const x = R();
  if (depth >= 4 || x < 0.4) return x < 0.15 ? (good ? genGoodLiteral() : genLiteral()) : pick(good && R() < 0.95 ? refs : REFS);
  const f = pick(FNS);
  const colour = () => genExpr(depth + 1, good, refs);
  const goodNums = f === "guard" ? ["1", "3", "4.5", "7", "21", "1.5"] : f === "lighter" || f === "darker"
    ? ["0", "0.5", "1", "1.18", "1.5", "2", "3", "99.5", "0.004", "0.005", "9999.9999", "1.005", "0.99"]
    : ["0", "1", "0.5", ".18", "0.0001", "0.9999", "0.3", "0.25"];
  const n = () => pick(good && R() < 0.97 ? goodNums : NUMS);
  let args;
  switch (f) {
    case "alpha":
    case "lighter":
    case "darker":
      args = [colour(), n()];
      break;
    case "mix":
    case "mixok":
    case "guard":
      args = [colour(), colour(), n()];
      break;
    case "over":
      args = [colour(), colour()];
      break;
    default:
      args = Array.from({ length: 2 + int(4) }, colour);
  }
  if (!good) {
    if (chance(0.05)) args.pop();
    if (chance(0.05)) args.push(chance(0.5) ? n() : colour());
    if (chance(0.04)) args.reverse();
  }
  const sep = () => pick([", ", ",", " , ", ",\n", "\t,"]);
  return f + pick(["(", " (", "(\n"]) + args.join(sep()) + pick([")", " )", "\n)"]);
}
const EXPR_SOUP = ["alpha", "mix", "lighter", "darker", "over", "mixok", "best", "guard", "blend", "(", ")", ",", " ", "  ",
  "\t", "\n", "\r", "#", "#fff", "#FFF", "#1e6a76", "#1e6a7680", "#1e6a76/0.5", "#1e6a76/.5", "#12", "#ggg", "/", ".", "0", "1",
  "9999.9999", "10000", "0.5", ".18", "1.", "1.0001", "21", "21.0001", "0.12345", "1e2", "-1", "accent.base", "bg", "fg", "c3",
  "c16", "x:y-z", "name.12", "é", "🎨", "-", "a", "_", "surface.island", "text.primary", "cursor", "selectionBg", "\u00a0", ":"];
function exprCase() {
  const x = R();
  let s;
  if (x < 0.45) s = genExpr(0);
  else if (x < 0.85) s = Array.from({ length: 1 + int(12) }, () => pick(EXPR_SOUP)).join("");
  else {
    s = genExpr(0);
    const ops = 1 + int(3);
    for (let i = 0; i < ops; i++) {
      const cps = [...s];
      const p = int(cps.length + 1);
      const y = R();
      if (y < 0.4) s = cps.slice(0, p).join("") + pick(EXPR_SOUP) + cps.slice(p).join("");
      else if (y < 0.8) s = cps.slice(0, p).join("") + cps.slice(p + 1 + int(3)).join("");
      else s = cps.slice(0, p).join("") + cps.slice(p).join("").toUpperCase();
    }
  }
  return s;
}
function exprResult(s) {
  const p = parseExpr(s);
  return p.ok ? "ok:" + exprText(p.e, formatColour) : "err:" + p.why;
}

// ---- literals and numbers -------------------------------------------------------------------------------------------
const LIT_CHARS = "0123456789abcdefABCDEFgx/. -#";
function literalCase() {
  if (chance(0.5)) return genLiteral();
  let s = chance(0.8) ? "#" : "";
  const n = int(12);
  for (let i = 0; i < n; i++) s += LIT_CHARS[int(LIT_CHARS.length)];
  if (chance(0.05)) s += pick(["é", "🎨", "\n", "\u00a0"]);
  return s;
}
const DEC_CHARS = "0123456789.x/ -";
function decimalCase() {
  let s = "";
  const n = int(11);
  for (let i = 0; i < n; i++) s += chance(0.7) ? String(int(10)) : DEC_CHARS[int(DEC_CHARS.length)];
  return s;
}

// ---- engine: mutated themes (the web unit test's procedure, with value semantics) ------------------------------------
const MUTATE_KEYS = ["format", "v", "id", "name", "palette", "roles", "tokens", "accent", "variant", "names", "author",
  "source", "backdrop", "x-extra", "__proto__", "constructor", "toString"];
const MUTATE_JUNK = [null, 1, -2.5, true, "", "#fff", "x", "c3", "cursor", [], {}, ["#000"], { a: 1 }, "mix(bg, fg, 0.5)",
  "https://x.y", "mxc://example.com/AbC", { image: "mxc://a/b" }, { image: "https://x/y" }, "light", "dark", "hash-hsl",
  "#12345678", "#123456/0.5", 1.5, "é".repeat(81), "c15", "c16", "Palette", "tinhat-theme", "a".repeat(65),
  Array(16).fill("#101010"), Array(15).fill("#101010"), { bg: "#000", fg: "#fff" }, "#ffffff/0.5", 1e21, 0.1, -0,
  JSON.parse('{"__proto__": {"bg": "#000000", "format": "tinhat-theme"}, "constructor": "#ffffff"}'), JSON.parse('{"__proto__": "#fff"}'),
  JSON.parse('{"image": "mxc://a/b", "__proto__": {"image": "https://x"}}')];
// Replayed by the Rust test from mulberry32(seed): keep every R() call and its order exactly as here.
function mutateCase() {
  const t = clone(LIB.themes[int(LIB.themes.length)]);
  const n = 1 + int(3);
  for (let j = 0; j < n; j++) {
    const k = MUTATE_KEYS[int(MUTATE_KEYS.length)];
    const v = clone(MUTATE_JUNK[int(MUTATE_JUNK.length)]);
    const palObj = t.palette !== null && typeof t.palette === "object" && !Array.isArray(t.palette);
    if (k === "palette" && palObj && R() < 0.5) t.palette[["bg", "fg", "cursor", "ansi", "selectionBg"][int(5)]] = v;
    else if ((k === "roles" || k === "tokens") && R() < 0.7) {
      const id = k === "roles" ? ROLE_IDS[int(ROLE_IDS.length)] : SYN.tokens[int(SYN.tokens.length)].id;
      t[k] = { [id]: typeof v === "string" ? v : "#fff" };
    } else setOwn(t, k, v);
  }
  return t;
}

// ---- engine: themes with random role/token overrides ------------------------------------------------------------------
function overridePatch() {
  const patch = {};
  const nr = int(5);
  const nt = int(4);
  if (nr) {
    patch.roles = {};
    for (let i = 0; i < nr; i++)
      setOwn(patch.roles, chance(0.9) ? pick(ROLE_IDS) : pick(["nope", "Surface.island", "x", "c1", "1abc", "__proto__", "constructor"]), genExpr(2 + int(2), true, ROLE_REFS));
  }
  if (nt) {
    patch.tokens = {};
    for (let i = 0; i < nt; i++)
      setOwn(patch.tokens, chance(0.9) ? pick(SYN.tokens).id : pick(["nope", "surface.island", "a.b", "__proto__", "toString"]), genExpr(2 + int(2), true, TOKEN_REFS));
  }
  if (chance(0.25)) patch.accent = pick(["cursor", "c1", "c4", "c6", "c7", "c9", "c12", "c15", "#e0a020", "#e0a020/0.5", "#fff", "c0"]);
  if (chance(0.15)) patch.variant = pick(["dark", "light"]);
  if (chance(0.1)) patch.names = pick(["palette", "hash-hsl"]);
  if (chance(0.1)) {
    const p = clone(LIB.themes[int(LIB.themes.length)].palette ?? {});
    if (chance(0.5)) p.cursor = "#" + hexs(6);
    if (chance(0.5)) p.selectionBg = "#" + hexs(6);
    if (chance(0.2)) delete p.cursor;
    patch.palette = p;
  }
  return patch;
}

// ---- engine: the §4 limits (size, depth, string length, URL rule, paths, backdrop) --------------------------------------
// A patch may hold {"$pad": [unit, n, chunk]}: both sides expand it to an array of strings, unit repeated n times in
// chunks of `chunk` units (so the file stays small while the theme reaches the 8 KB boundary exactly).
const KEY_BITS = ["a", "b", "~", "/", "~1", "é", "🎨", "backdrop", "image", "x".repeat(39), "y".repeat(41), "http:", "www.", "://", "Z",
  "0", "10", "__x", " ", "", "Ω", ...INHERITED_NAMES];
const STR_BITS = ["a", "é", "日", "🎨", "~", "/", '"', "\\", "\n", "\u0001", " ", "\u007f", "http:", "://", "www.", "mailto:",
  "mxc://a/b", "data:", "Blob:", " ", "x".repeat(40), "🎨".repeat(40), "vbscript:", "abchttp:", "a-www.b", "javascript:", "FILE:",
  "ftp:x", "é".repeat(27)];
const randKey = () => Array.from({ length: 1 + int(3) }, () => pick(KEY_BITS)).join("");
const randString = () => Array.from({ length: int(4) }, () => pick(STR_BITS)).join("");
function randValue(depth) {
  const x = R();
  if (depth < 20 && x < 0.25) {
    const o = {};
    const n = int(3);
    for (let i = 0; i < n; i++) setOwn(o, randKey(), randValue(depth + 1));
    return o;
  }
  if (depth < 20 && x < 0.4) return Array.from({ length: int(3) }, () => randValue(depth + 1));
  if (x < 0.75) return randString();
  return pick([0, 1, -1, 1e21, 1e-7, 5e-324, 0.1, 123456789012345680000, 1.5e300, true, false, null]);
}
function expandPads(v) {
  if (Array.isArray(v)) return v.map(expandPads);
  if (v !== null && typeof v === "object") {
    if (Array.isArray(v.$pad)) {
      const [unit, n, chunk] = v.$pad;
      const out = [];
      for (let left = n; left > 0; left -= chunk) out.push(unit.repeat(Math.min(chunk, left)));
      return out;
    }
    if (Array.isArray(v.$nest)) {
      // {"$nest": [n, leaf]} → n nested arrays around leaf (keeps the corpus file shallow)
      let x = v.$nest[1];
      for (let i = 0; i < v.$nest[0]; i++) x = [x];
      return x;
    }
    const o = {};
    for (const k of Object.keys(v)) setOwn(o, k, expandPads(v[k]));
    return o;
  }
  return v;
}
function limitsCase() {
  const base = int(LIB.themes.length);
  const patch = {};
  const nk = int(3);
  for (let i = 0; i < nk; i++) setOwn(patch, randKey(), randValue(1));
  // around the JSON nesting limit: the theme is level 1, so n arrays under a key reach level n + 1 (129 = not JSON data)
  if (chance(0.15)) setOwn(patch, pick(["nest", "__proto__", "n"]), { $nest: [124 + int(8), pick(["x", 1, null])] });
  if (chance(0.3)) {
    // a chain nested around the depth limit (root = 1, deeper than 16 is refused)
    const d = 12 + int(8);
    let v = chance(0.5) ? randString() : "x";
    for (let i = 0; i < d; i++) v = chance(0.5) ? [v] : { [pick(["k", "~", "/"])]: v };
    patch[pick(["deep", "Deep", "d/e", "d~p"])] = v;
  }
  if (chance(0.25))
    patch.backdrop = pick([{ image: "mxc://example.com/AbC_d-9" }, { image: "mxc://" + "s".repeat(255) + "/" + "m".repeat(255) },
      { image: "mxc://" + "s".repeat(256) + "/m" }, { image: "mxc://a/b", fit: "cover", note: "http://x" }, { image: 5 },
      { image: "mxc://a/b\n" }, "mxc://a/b", { Image: "https://x" }, {}, { image: "MXC://a/b" }, { image: "mxc://a:8448/b" }]);
  if (chance(0.5)) {
    const unit = pick(["x", "é", "日", "🎨", "\n", '"', "\u0001", " ", "ab"]);
    const key = pick(["pad", "zz", "a", "~"]);
    const chunk = pick([80, 40, 81]);
    const probe = assignOwn(assignOwn(clone(LIB.themes[base]), expandPads(patch)), { [key]: [] });
    const size0 = utf8Length(JSON.stringify(probe));
    const unitBytes = utf8Length(JSON.stringify(unit)) - 2;
    const perChunk = 3; // the quotes and the comma of each chunk
    const target = 8192 + pick([-2, -1, 0, 0, 1, 1, 2, 7]);
    let n = 0;
    for (let len = size0; ; n++) {
      const chunks = Math.ceil((n + 1) / chunk);
      const next = size0 + (n + 1) * unitBytes + chunks * perChunk - 1;
      if (next > target) break;
      len = next;
    }
    patch[key] = { $pad: [unit, n, chunk] };
  }
  return [base, patch];
}

// ---- importers --------------------------------------------------------------------------------------------------------
const BAD_COLOURS = ["none", "CellForeground", "red", "background", "", "#", "#12345", "#1234567", "rgb:ff/ff", "rgb:fffff/0/0",
  "0xabcdefab", '""', '"', "#abc/0.5", "#abcdef\u0085", "'#abcdef'", "#abcdefab", "#abcdeg", "#abcd🎨", "rgb:ff/8/0x", "0x12345"];
// A colour as theme files write it: mostly readable spellings (every accepted form, odd spaces), `bad` of the time junk.
function colourSpelling(bad = 0.25) {
  if (R() < bad) return pick(BAD_COLOURS);
  const x = R();
  if (x < 0.55) return "#" + hexs(6, chance(0.2));
  if (x < 0.63) return "#" + hexs(3, true);
  if (x < 0.69) return hexs(6, true);
  if (x < 0.73) return pick(["0x", "0X"]) + hexs(6, true);
  if (x < 0.79) return pick(["rgb:", "RGB:", "Rgb:"]) + hexs(1 + int(4)) + "/" + hexs(1 + int(4)) + "/" + hexs(1 + int(4));
  if (x < 0.86) return '"#' + hexs(6) + '"';
  if (x < 0.89) return pick(["\u00a0", " ", "\t", "\ufeff", "\u2003"]) + "#" + hexs(6) + pick(["", " ", "\u00a0", "\u3000"]);
  if (x < 0.92) return '" #' + hexs(6) + ' "';
  return "#" + hexs(6);
}
const C = () => colourSpelling(0.012); // inside generated files: nearly always readable
const NAME_BITS = ["Dracula", "Rosé", "Pine", "Moon", "Tokyo", "Night", "日本", "🎨", "Straße", "Łódź", "Œuvre", "https://x.org/a",
  "www.example.com", "mailto:a@b.c", "user@example.com", "(https://github.com/x)", "(github.com/x)", "(Cosmos)", "<a@b.c>",
  "Foo, Bar", "a,b", ";", ":", "+", "Theme+", "\u0001", "\t", "  ", "x".repeat(50), "ÀÉÎÕÜ", "ǅ", "ĳ", "ſ", "\u2028", "Ǆ",
  "(", ")", "<", ">", "ﬁ", "Ⅻ", "K", "İ", "\u00a0", "Mc_Name", "-", "--", "v2.0", "base16", "Ω", "\u007f", "a (b.c)", "(x y.z)"];
const nameish = () => Array.from({ length: 1 + int(4) }, () => pick(NAME_BITS)).join(pick([" ", "", "  ", "_"]));
const SP = () => pick([" ", "  ", "\t", " \t", ""]);
const REQUIRED = new Set(["background", "foreground", "Background_Color", "Foreground_Color", "Background Color", "Foreground Color"]);
const keep = (k) => chance(REQUIRED.has(k) ? 0.995 : 0.8); // bg/fg nearly always, cursor/selection often
const ANSI8 = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
const JUNK_LINES = ["", "x", "[", "{", "<", "- a", "  - b", "key: [a, b", "\u2028", "\t", "# comment", "! comment", "// c",
  "include foo.conf", "=", ":", "color16 #ffffff", "color007 #123456", "palette = 17=#ffffff", "\ufeff", "[colors]", "colors:",
  'base0A: "ff0000"', "foreground\u00a0#000000", "color1\t\u00a0#fff"];
function noise(lines, junk) {
  const n = chance(0.5) ? int(4) : 0;
  for (let i = 0; i < n; i++) {
    const x = R();
    const p = int(lines.length + 1);
    if (x < 0.12 && lines.length) lines.splice(int(lines.length), 1);
    else if (x < 0.5 && lines.length) lines.splice(p, 0, lines[int(lines.length)]);
    else if (x < 0.8) lines.splice(p, 0, pick(junk));
    else if (lines.length) {
      const q = int(lines.length);
      lines[q] = pick(["  ", "\t", " "]) + lines[q] + pick(["", " ", "\t", " # c"]);
    }
  }
  return lines;
}
function genKitty() {
  const L = [];
  if (chance(0.5)) L.push("## name:" + SP() + nameish());
  if (chance(0.4)) L.push(pick(["## author: ", "## Author:", "##AUTHOR : ", "## author\u2028: "]) + nameish());
  const slots = [...Array.from({ length: 16 }, (_, i) => `color${i}`), "foreground", "background", "cursor", "selection_background",
    "selection_foreground", "cursor_text_color"];
  for (const s of slots) if (chance(0.995)) L.push(s + pick([" ", "  ", "\t"]) + C() + (chance(0.05) ? " extra" : ""));
  if (chance(0.2)) L.push(IN() + " " + C());
  shuffle(L);
  return noise(L, JUNK_LINES).join("\n");
}
function genGhostty() {
  const L = [];
  for (let i = 0; i < 16; i++)
    if (chance(0.995)) L.push(`palette${SP()}=${SP()}${chance(0.03) ? "0" : ""}${i}${SP()}=${SP()}${C()}`);
  for (const k of ["background", "foreground", "cursor-color", "selection-background", "selection-foreground", "cursor-text"])
    if (keep(k)) L.push(k + SP() + "=" + SP() + C());
  if (chance(0.2)) L.push(IN() + SP() + "=" + SP() + C());
  shuffle(L);
  return noise(L, JUNK_LINES).join("\n");
}
function genXres() {
  const L = [];
  const defs = chance(0.4);
  const prefix = () => pick(["*", "*.", "URxvt*", "URxvt.", "XTerm*", "*vt100.", "*", "*.", "URxvt*", "URxvt.", "XTerm*", "*vt100.", ""]);
  for (let i = 0; i < 16; i++) {
    if (!chance(0.995)) continue;
    if (defs && chance(0.5)) {
      L.push(`#define C${i}${pick([" ", "\t"])}${C()}`);
      L.push(`${prefix()}color${i}:${SP()}C${i}`);
    } else L.push(`${prefix()}color${i}${SP()}:${SP()}${C()}`);
  }
  for (const k of ["foreground", "background", "cursorColor", "highlightColor", "highlightTextColor"])
    if (keep(k)) L.push(`${prefix()}${k}:${SP()}${C()}`);
  if (chance(0.2)) L.push(`${prefix()}${IN()}:${SP()}${C()}`);
  if (chance(0.1)) {
    const d = IN();
    L.push(`#define ${d} ${C()}`, `${prefix()}color${int(16)}: ${d}`);
  }
  shuffle(L);
  return noise(L, [...JUNK_LINES, "#define X X", "#define   Y\u00a0z", "*color3: X", "#define C1 C2", "#define C2 #123456"]).join("\n");
}
function genXrdb() {
  const L = [];
  for (let i = 0; i < 16; i++) if (chance(0.995)) L.push(`#define Ansi_${i}_Color ${C()}`);
  for (const k of ["Background_Color", "Foreground_Color", "Cursor_Color", "Selection_Color", "Selected_Text_Color", "Bold_Color"])
    if (keep(k)) L.push(`#define ${k} ${C()}`);
  if (chance(0.15)) L.push(`#define ${IN()} ${C()}`);
  shuffle(L);
  return noise(L, JUNK_LINES).join("\n");
}
const qs = (s) => pick([`"${s}"`, `'${s}'`, `"${s}"`]);
function genAlacrittyToml() {
  const L = [];
  if (chance(0.3)) L.push("# " + nameish());
  const style = int(3);
  if (style === 0) {
    L.push("[colors.primary]", `background = ${qs(C())}`, `foreground = ${qs(C())}`);
    for (const g of ["normal", "bright"]) {
      L.push(`[colors.${g}]`);
      for (const c of ANSI8) if (chance(0.995)) L.push(`${c}${SP()}=${SP()}${qs(C())}`);
    }
    if (chance(0.6)) L.push("[colors.cursor]", `cursor = ${qs(C())}`, `text = ${qs(C())}`);
    if (chance(0.6)) L.push("[colors.selection]", `background = ${qs(C())}`, `text = ${qs(C())}`);
  } else if (style === 1) {
    L.push(`colors.primary = { background = ${qs(C())}, foreground = ${qs(C())} }`);
    for (const g of ["normal", "bright"]) L.push(`colors.${g} = { ${ANSI8.map((c) => `${c} = ${qs(C())}`).join(", ")} }`);
  } else {
    L.push("[colors]", `primary.background = ${qs(C())}`, `primary.foreground = ${qs(C())}`);
    for (const g of ["normal", "bright"]) for (const c of ANSI8) L.push(`${g}.${c} = ${qs(C())}`);
    if (chance(0.3)) L.push("[[colors.indexed_colors]]", "index = 16", `color = ${qs(C())}`);
  }
  if (chance(0.25))
    L.push(pick([`[${IN()}]`, `[colors.${IN()}]`, `[[${IN()}]]`, `[${IN()}.${IN()}]`, `[colors."${IN()}".primary]`]),
      `${pick(["primary", "background", "cursor", IN()])} = ${pick(['"#ff0000"', '{ background = "#ff0000" }', "1", "[1]"])}`);
  if (chance(0.15)) L.unshift(`${IN()} = { colors = { primary = { background = "#ff0000" } } }`);
  if (chance(0.1)) L.push("[colors.extra]", `deep = ${nestText(125 + int(7))}`);
  return noise(L, [...JUNK_LINES, "x = 1", "y = true", "z = [1, 2]", "w = { a = 1 }", "[colors.primary]", 'bad = "\\q"',
    'm = """a\nb"""', "k.\"q\" = 'v'", "[colors.normal]\nblack = 0x000000"]).join("\n");
}
function genWezterm() {
  const L = ["[colors]"];
  const arr = (n) => "[" + Array.from({ length: n }, () => qs(C())).join(pick([", ", ",", ",\n  "])) + pick(["]", ",]", "\n]"]);
  L.push(`ansi = ${arr(chance(0.9) ? 8 : int(10))}`, `brights = ${arr(chance(0.9) ? 8 : int(10))}`);
  for (const k of ["foreground", "background", "cursor_bg", "cursor_fg", "selection_bg", "selection_fg"])
    if (keep(k)) L.push(`${k} = ${qs(C())}`);
  if (chance(0.2)) L.push(`${IN()} = ${pick([qs(C()), '{ selection_fg = "#ff0000" }', "1"])}`);
  if (chance(0.1)) L.push(`deep = ${nestText(125 + int(7))}`);
  if (chance(0.6)) L.push("[metadata]", `name = ${qs(nameish())}`, `author = ${qs(nameish())}`);
  return noise(L, [...JUNK_LINES, "x = 1", "[colors.indexed]", '16 = "#ffffff"', 'tab_bar = { background = "#000000" }']).join("\n");
}
// A YAML collection as a flow-mapping key or on a plain scalar's continuation line: the reference spells String() out
// (web d18d688 — String() of its null-prototype documents threw a TypeError), the port has js_string.
const YAML_STRINGIFY = ["x: {{a: b}: c}", "k: {[{p: q}, r]: s, [a, , b]: t, : u}", "  {cont: x}", "  [cont, {x: y}, , [z]]"];
/** Whole documents of that kind (a quoted scalar with a continuation line is bad-yaml; the rest parse). */
const YAML_STRINGIFY_DOCS = ["k: {{x: y}: z, [a, , b]: c, [{p: q}, r]: d, : e}\n", "k: a\n  {b: c}\n  [d, {e: f}, [g, ]]\n",
  "- {[{a: b}]: c}\n", "k: {[[a, [b]], {c: d}]: e}\n", "k: plain\n  [x]\n  {y: z}\n  more\n", "{{a: b}: c}\n", "k: 'q'\n  {x: y}\n",
  "a:\n  k: {{x: y}: z}\n  m: b\n    [c, {d: e}]\n", "- k: x\n  [y, {z: w}]\n"];
function genBase(system) {
  const L = [];
  if (system !== null) L.push(`system: ${qs(system)}`);
  L.push(`name: ${chance(0.8) ? qs(nameish()) : nameish()}`);
  if (chance(0.05)) L.push("  " + pick(["[x, {y: z}]", "{a: b}", "[a, , [b]]"])); // continues a plain name (quoted: bad-yaml)
  if (chance(0.7)) L.push(`author: ${qs(nameish())}`);
  if (chance(0.5)) L.push("variant: " + pick(["dark", "light"]));
  const nested = chance(0.7);
  if (nested) L.push("palette:");
  const n = system === "base16" || (system === null && chance(0.5)) ? 16 : 24;
  const ind = nested ? pick(["  ", "    "]) : "";
  for (let i = 0; i < n; i++) {
    if (!chance(0.995)) continue;
    let k = "base" + (i < 16 ? "0" + HEXD[i] : "1" + HEXD[i - 16]).toUpperCase();
    if (chance(0.1)) k = k.toLowerCase();
    if (chance(0.05)) k = qs(k);
    L.push(`${ind}${k}:${pick([" ", "  ", "\t"])}${chance(0.85) ? qs(hexs(6)) : C()}${chance(0.05) ? " # c" : ""}`);
  }
  if (chance(0.15)) L.push(`${ind}${IN()}: ${pick([qs(hexs(6)), `{base00: ${qs(hexs(6))}}`])}`);
  if (chance(0.2)) L.unshift(`${IN()}:`, `  system: ${qs(pick(["base16", "base24"]))}`, `  name: ${qs("Evil")}`);
  if (chance(0.1)) L.push(`deep: ${nestText(124 + int(7))}`);
  return noise(L, [...JUNK_LINES, "---", "...", "%YAML 1.2", "  bad: indent", "x: {a: b}", "y: [1, 2]", "z: |", "  text", "\tt: 1",
    "base0A: '00ff00'", "base0a: \"0000ff\"", ...YAML_STRINGIFY]).join("\n");
}
function genAlacrittyYaml() {
  const L = ["colors:"];
  L.push("  primary:", `    background: ${qs(C())}`, `    foreground: ${qs(C())}`);
  for (const g of ["normal", "bright"]) {
    L.push(`  ${g}:`);
    for (const c of ANSI8) if (chance(0.995)) L.push(`    ${c}: ${chance(0.8) ? qs(C()) : C()}`);
  }
  if (chance(0.5)) L.push("  cursor:", `    cursor: ${qs(C())}`);
  if (chance(0.5)) L.push("  selection:", `    background: ${qs(C())}`, `    text: ${qs(C())}`);
  if (chance(0.3)) L.unshift("# " + nameish());
  if (chance(0.2)) L.push(`  ${IN()}:`, `    primary: {background: '#ff0000', ${IN()}: x}`);
  return noise(L, [...JUNK_LINES, "  bright:", "    x: y", "  - item", "font:", "  size: 12", ...YAML_STRINGIFY]).join("\n");
}
function plistColour() {
  const comp = () => pick([String(R()), (R() * 1.2 - 0.1).toFixed(int(8)), "1", "0", "1e0", ".5", "+0.25", "-0.1", "1.5", "abc", "0.5 ",
    " 0.5", "1E-1", "2e400", "1.", "0.50196078431372548", "0.0019607843137254902"]);
  const tag = () => pick(["real", "real", "string", "integer"]);
  const parts = ["Red", "Green", "Blue"].filter(() => chance(0.97)).map((c) => {
    const t = tag();
    return `<key>${c} Component</key><${t}>${comp()}</${t}>`;
  });
  if (chance(0.5)) parts.push("<key>Alpha Component</key><real>1</real>");
  if (chance(0.5)) parts.push("<key>Color Space</key><string>sRGB</string>");
  shuffle(parts);
  return "<dict>" + parts.join(pick(["", "\n\t\t", " "])) + "</dict>";
}
function genPlist() {
  const entries = [];
  for (let i = 0; i < 16; i++) if (chance(0.995)) entries.push(`<key>Ansi ${i} Color</key>\n${plistColour()}`);
  for (const k of ["Background Color", "Foreground Color", "Cursor Color", "Selection Color", "Selected Text Color", "Bold Color"])
    if (keep(k)) entries.push(`<key>${k}</key>\n${plistColour()}`);
  if (chance(0.2)) entries.push(`<key>${IN()}</key>\n${plistColour()}`);
  if (chance(0.1)) entries.push(`<key>Cursor Color</key><dict><key>${IN()}</key>${plistColour()}</dict>`);
  if (chance(0.1)) {
    const n = 125 + int(7);
    entries.push(`<key>Deep</key>${"<array>".repeat(n)}<string>x</string>${"</array>".repeat(n)}`);
  }
  shuffle(entries);
  if (chance(0.1)) entries.push("<key>Odd</key>");
  if (chance(0.1)) entries.push("<!-- comment --><key>X&amp;Y</key><string>&#x41;&lt;&#66;&#0;&#x110000;&bogus;&#X41;</string>");
  let s = (chance(0.8) ? '<?xml version="1.0" encoding="UTF-8"?>\n' : "") +
    (chance(0.7) ? '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n' : "") +
    `<plist version="1.0">\n<dict>\n${entries.join("\n")}\n</dict>\n</plist>\n`;
  if (chance(0.15)) s = insertAt(s, pick(["<", ">", "</dict>", "<dict>", "<![CDATA[x]]>", "<!-- x", "<key/>", "&amp;", "\u00a0"]));
  return s;
}
function genWT() {
  const scheme = {};
  if (chance(0.8)) scheme.name = nameish();
  for (const k of ["background", "foreground", "cursorColor", "selectionBackground"]) if (keep(k)) scheme[k] = C();
  for (const c of ["black", "red", "green", "yellow", "blue", "purple", "cyan", "white"]) {
    if (chance(0.995)) scheme[c] = C();
    if (chance(0.995)) scheme["bright" + c[0].toUpperCase() + c.slice(1)] = C();
  }
  if (chance(0.25)) setOwn(scheme, IN(), pick([C(), { background: "#ff0000", black: "#ff0000" }, 1]));
  let doc = scheme;
  if (chance(0.4))
    doc = { $schema: "https://aka.ms/terminal-profiles-schema", profiles: { list: [] }, schemes: [...(chance(0.3) ? [1, "x", null] : []), scheme, { name: "Other" }] };
  let s = JSON.stringify(doc, null, pick([0, 2, 4, "\t"]));
  if (chance(0.1)) s = s.slice(0, s.lastIndexOf("}")) + `,"deep":${nestText(125 + int(7))}}`;
  if (chance(0.1)) s = s.replace(/"name": ?"/, (m) => m + pick(ESCAPES));
  if (chance(0.3)) s = "// settings.json\n" + s.replace(/,\n/g, () => (chance(0.1) ? ", // c\n" : ",\n"));
  if (chance(0.2)) s = s.replace(/\n(\s*)\}/, ",\n$1}");
  if (chance(0.1)) s = s.replace(/"name"/, '/* x */ "name"');
  if (chance(0.08)) s = insertAt(s, pick(["}", '"', ",", "#", "/*"]));
  return s;
}
function genTinhat() {
  const t = clone(pick(LIB.themes));
  if (chance(0.3)) t.extra = { n: pick([1, 1.5, -0, 1e21, 1e-7, 123456789012345680000, 5e-324]), s: "é\u2028" };
  if (chance(0.25)) setOwn(t, IN(), pick([{ format: "x", palette: { bg: "#ff0000" } }, "y", 1]));
  let s = JSON.stringify(t, null, chance(0.5) ? 2 : 0);
  if (chance(0.1)) s = s.slice(0, s.lastIndexOf("}")) + `,"deep":${nestText(125 + int(7))}}`;
  if (chance(0.1)) s = s.replace(/"name": ?"/, (m) => m + pick(ESCAPES));
  if (chance(0.3)) s = "# my theme\n" + s;
  if (chance(0.1)) s = s.replace('"v":', '"v" :').replace('"v":1', '"v":1.0');
  if (chance(0.1)) s = s.replace('"v":1', '"v":1e400');
  return s;
}
const HINTS = ["x.conf", "My_Theme.yaml", "C:\\dir\\a_b.itermcolors", "", null, ".conf", "Théme.json", "a/b/c", "Rosé_Pine.Xresources",
  "https://x.org/t.toml", "日本.toml", "x".repeat(90) + ".txt", "tokyonight_night", "a.b.c.JSONC", "dir/.yaml", "\u0001name.theme"];
const GENS = [genKitty, genGhostty, genXres, genXrdb, genAlacrittyToml, genWezterm, () => genBase(pick(["base16", "base24", null, "base16", "other"])),
  genAlacrittyYaml, genPlist, genWT, genTinhat];
function importCase() {
  let text = pick(GENS)();
  const x = R();
  if (x < 0.1) text = text.split("\n").join("\r\n");
  else if (x < 0.13) text = text.split("\n").join("\r");
  if (chance(0.05)) text = "\ufeff" + text;
  if (chance(0.05)) text = pick(["\n\n", "   \n", "\u00a0\n", "\t"]) + text;
  if (chance(0.04)) text += "\n" + pick(YAML_STRINGIFY_DOCS);
  if (chance(0.03)) text = pick(["", " ", "\ufeff", "\n\r\n", "\u2028", "\ufeff\ufeff"]);
  return [text, pick(HINTS)];
}

// ---- parsers ----------------------------------------------------------------------------------------------------------
const TOML_SOUP = ["[", "]", "[[", "]]", "a", "b.c", '"q k"', "'lit'", " = ", "=", '"str"', "'s'", '"""ml\nx"""', "'''m'''", "1", "true",
  "1979-05-27", "[1, 2,]", "{ x = 1 }", "{}", "#c", "\n", "\n", " ", "\t", '"\\u00e9"', '"\\x41"', '"\\q"', ",", "}", "{", "é", "\r\n",
  '"""a\\\n  b""""', 'x.y.z = "v"', "[t]\n", "[[t]]\n", "k = [\n1,\n2,\n]\n", 'k = { a.b = "c", d = ["e"] }\n', '"\\U0001F3A8"', '"\\ud800"',
  "[a.b]\nc = 1\n[a]\nd = 2\n", "t = 1\n[t]\n", "[[a]]\n[a.b]\nc = 1\n", "a = []\n[[a]]\n", "'''\nx''''\n", '"""\\\t\n  y"""',
  "__proto__", "[__proto__]\n", "constructor = 1\n", 'toString.x = "y"\n', "[[__proto__]]\n", '__proto__ = { a = "b" }\n', "[a.__proto__.b]\n"];
const YAML_LINES = ["a: b", "- x", "  - y", "key:", "  sub: 1", '"q": v', "'s''t': v", "[a, b]", "{a: b}", "k: [a, {b: c}]", "k: |", "  text",
  "k: >", "# c", "---", "...", "%YAML 1.2", "\tbad", "k: 'unterminated", 'k: "esc \\x41 \\u00e9 \\q"', "k: a #c", "- - z", "- k: v",
  "    deep: x", 'k: "a" x', "k: [a, [b, c]]", "k: {a: [1, 2], b: {c: d}}", "k:v", "k :v", "k#x: v", "k: a:b", "- ", "-", 'k: ""',
  "  continued line", "k: {[a, b]: c, {x: y}: z, : w}", '"k\\"q": v', 'k: "\\U0001F3A8"', "k: 'a''b' # c", "? complex", "k: &anchor v",
  "k: *ref", "0: zero", "10: ten", "2: two", "k: [a, b", "- [x, y]", "  [z]", "k: 'x'  # c", "k:\t\"t\"", "  - - deep",
  "__proto__: x", "constructor:", "  toString: y", "k: {__proto__: v, constructor: w}", "- __proto__: z", "__proto__:", "  __proto__: q",
  ...YAML_STRINGIFY];
function tomlCase() {
  if (chance(0.08)) {
    const n = 126 + int(5);
    return `x = 1\na = ${chance(0.5) ? nestText(n, '"x"') : "{b = ".repeat(n) + "1" + "}".repeat(n)}`;
  }
  if (chance(0.5)) return pick([genAlacrittyToml, genWezterm])();
  return Array.from({ length: 1 + int(14) }, () => pick(TOML_SOUP)).join(chance(0.5) ? "" : pick([" ", "\n"]));
}
function yamlCase() {
  if (chance(0.08)) return chance(0.5) ? `a: ${nestText(125 + int(5), "x")}` : Array.from({ length: 126 + int(5) }, (_, i) => " ".repeat(i) + "a:").join("\n") + " x\n";
  if (chance(0.06)) return pick(YAML_STRINGIFY_DOCS);
  if (chance(0.5)) return pick([() => genBase(pick(["base16", "base24", null])), genAlacrittyYaml])();
  return Array.from({ length: 1 + int(10) }, () => pick(["", "  ", "    ", " "]) + pick(YAML_LINES)).join("\n");
}
function jsoncCase() {
  if (chance(0.08)) return nestText(126 + int(5));
  if (chance(0.5)) return genWT();
  const bits = ["{", "}", "[", "]", ",", ":", '"a"', '"b\\"c"', "1", "-0", "1e400", "1.5e-7", "true", "null", "// c\n", "# c\n", "/* c */",
    "/*", '"/* not */"', '"#x"', " ", "\n", ", }", ",]", '"\\ud83c\\udfa8"', '"é"', '"\\\\"', '"a\\', '"__proto__"', '"constructor"',
    '"\\ud800"', '"x\\udc00"'];
  return Array.from({ length: 1 + int(16) }, () => pick(bits)).join("");
}
function plistCase() {
  if (chance(0.08)) {
    const n = 126 + int(5);
    return "<plist>" + "<array>".repeat(n) + "</array>".repeat(n) + "</plist>";
  }
  if (chance(0.5)) return genPlist();
  const bits = ["<plist>", "</plist>", "<dict>", "</dict>", "<key>a</key>", "<key>b &amp; c</key>", "<string>x</string>", "<real> 0.5 </real>",
    "<true/>", "<false/>", "<array>", "</array>", "<!-- c -->", "<![CDATA[<z>]]>", "<?xml?>", '<!DOCTYPE x [<!ENTITY y "z">]>', "text",
    "&lt;", "&#x41;", "&#12;", "<key>", "</key>", "< dict >", "<dict/>", "</ dict>", '<a b="c">', "</a>", "<key>0</key>", "<key>10</key>",
    "<key>2</key>", "<key> sp </key>", "<key>__proto__</key>", "<key>constructor</key>"];
  return Array.from({ length: 1 + int(12) }, () => pick(bits)).join(pick(["", "\n"]));
}
function jsJson(v) {
  return JSON.stringify(v, (k, x) => (x instanceof RawToken ? { $raw: x.raw } : x)) ?? "undefined";
}
const PARSE = { toml: parseToml, yaml: parseYaml, jsonc: parseJsonc, plist: parsePlist };
function parserResult(which, text) {
  try {
    const v = PARSE[which](text);
    return which === "jsonc" ? "ok:" + canonJson(v) : "ok:" + jsJson(v);
  } catch (e) {
    if (e instanceof ParseError) return `err:${e.code}: ${e.detail}`;
    return "throw:" + String(e);
  }
}
const cap = (s) => (s.length > 200 ? "sha256:" + sha(s).slice(0, 24) : s);

// ---- assemble ---------------------------------------------------------------------------------------------------------
const doc = { reference: "src/theme @ " + (process.env.REF_COMMIT ?? "unknown"), seed: SEED, scale: SCALE };
doc.rng = (() => {
  const r = mulberry32(1);
  return Array.from({ length: 6 }, r);
})();
doc.syn = SYN;
doc.expr = Array.from({ length: 1200 * SCALE }, () => {
  const s = exprCase();
  return [s, exprResult(s)];
});
for (const d of [31, 32, 33, 40]) {
  const s = "alpha(".repeat(d) + "a" + ", 1)".repeat(d);
  doc.expr.push([s, exprResult(s)]);
}
doc.literal = Array.from({ length: 800 * SCALE }, () => {
  const s = literalCase();
  const c = parseLiteral(s);
  return [s, c ? formatColour(c) : null];
});
doc.decimal = Array.from({ length: 400 * SCALE }, () => {
  const s = decimalCase();
  return [s, parseDecimal4(s)];
});
{
  const main = R;
  R = mulberry32(SEED + 1);
  const results = [];
  for (let i = 0; i < 1500 * SCALE; i++) results.push(out(resolveTheme(mutateCase(), SYN)));
  doc.mutate = { keys: MUTATE_KEYS, junk: MUTATE_JUNK, seed: SEED + 1, results };
  R = main;
}
doc.overrides = Array.from({ length: 400 * SCALE }, () => {
  const base = int(LIB.themes.length);
  const patch = overridePatch();
  const theme = assignOwn(clone(LIB.themes[base]), patch);
  const real = chance(0.15);
  return [base, patch, real ? "real" : "syn", out(resolveTheme(theme, real ? SPEC : SYN))];
});
doc.limits = Array.from({ length: 300 * SCALE }, () => {
  const [base, patch] = limitsCase();
  const theme = assignOwn(clone(LIB.themes[base]), expandPads(patch));
  return [base, patch, out(resolveTheme(theme, SYN))];
});
doc.app = [];
for (const id of ["oled", "goose"]) {
  for (const app of ["desktop", "web", "tv"]) doc.app.push([id, null, app, out(resolveForApp(BUILTINS[id], SPEC, app))]);
  for (let i = 0; i < 20 * SCALE; i++) {
    const x = R();
    const patch = x < 0.4 ? { name: BUILTINS[id].name + (chance(0.5) ? "" : " copy") }
      : x < 0.7 ? { roles: { ...BUILTINS[id].roles, [pick(ROLE_IDS)]: genLiteral() } }
      : x < 0.85 ? { v: 1.0, id } : { tokens: { ...(BUILTINS[id].tokens ?? {}) } };
    const app = pick(["desktop", "web"]);
    doc.app.push([id, patch, app, out(resolveForApp(assignOwn(clone(BUILTINS[id]), patch), SPEC, app))]);
  }
}
doc.import = Array.from({ length: 300 * SCALE }, () => {
  const [text, hint] = importCase();
  try {
    return [text, hint, out(importTheme(text, hint ?? undefined))];
  } catch (e) {
    return [text, hint, "throw:" + String(e)]; // importTheme must never throw: the port's replay reports it
  }
});
// theme JSON text, read like the port reads it (parseThemeJson; ENGINE.md §9, PORT-QUESTIONS Q2/Q3)
function themeJsonCase() {
  const t = clone(pick(LIB.themes));
  if (chance(0.3)) setOwn(t, IN(), pick([{ format: "x" }, "y", 1, { palette: { bg: "#ff0000" } }]));
  let text = JSON.stringify(t);
  const x = R();
  if (x < 0.3) text = text.replace('"name":"', '"name":"' + pick(ESCAPES));
  else if (x < 0.4) text = text.replace('"id":', '"' + pick(ESCAPES) + '":1,"id":');
  else if (x < 0.55) text = text.replace(/}$/, `,"deep":${nestText(124 + int(8))}}`);
  else if (x < 0.62) text = pick(["\ufeff", " ", "\n", "\u00a0", "\t\r\n", "// c\n"]) + text;
  else if (x < 0.7) text = text.replace('"v":1', pick(['"v":1e400', '"v":1.0', '"v":-0', '"v":1,"v":1', '"v":2,"v":1', '"v":1,"x":-1e400']));
  else if (x < 0.76) {
    const cps = [...text];
    text = cps.slice(0, int(cps.length)).join("");
  } else if (x < 0.82) text = text.replace(/}$/, pick([",}", "} ", "}//c", "}x", "}\n"]));
  else if (x < 0.86) text = pick(["[" + text + "]", nestText(127 + int(3)), "42", '"x"', "null", ""]);
  return text;
}
doc.themeJson = Array.from({ length: 150 * SCALE }, () => {
  const text = themeJsonCase();
  const p = parseThemeJson(text);
  return [text, out(p.ok ? resolveTheme(p.theme, SYN) : p)];
});
doc.parsers = [];
for (let i = 0; i < 250 * SCALE; i++) {
  const which = pick(["toml", "yaml", "jsonc", "plist"]);
  const text = { toml: tomlCase, yaml: yamlCase, jsonc: jsoncCase, plist: plistCase }[which]();
  doc.parsers.push([which, text, cap(parserResult(which, text))]);
}
process.stdout.write(JSON.stringify(doc) + "\n");
