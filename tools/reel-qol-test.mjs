// reel-qol-test.mjs — Game Reel QOL (A19.1 / A19.3 / A19.5): runs GameReelPanel.qml's pure reelWhyNot and
// MyGamesEditor.qml's pure importCountText (between their BEGIN … pure / END … pure markers) on the shared vectors
// crates/app/src/testvectors/reel-qol.json (Tin Hat Web: src/features/calendar/model/reelQol.test.ts), and checks the wiring:
// a greyed Spin / Veto shows its reason on hover, the import line counts every game, Enter adds the first result.
//   node --test tools/reel-qol-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const panel = file("../crates/app/qml/GameReelPanel.qml");
const ed = file("../crates/app/qml/MyGamesEditor.qml");
const V = JSON.parse(file("../crates/app/src/testvectors/reel-qol.json"));
const pure = (src, name) => src.split(`// BEGIN ${name} pure`)[1]?.split(`// END ${name} pure`)[0]?.replace(/^[^\n]*\n/, "");

test("reelWhyNot: the shared vectors", () => {
  const body = pure(panel, "reel-whynot");
  assert.ok(body, "GameReelPanel.qml has no reel-whynot pure block");
  const ctx = vm.createContext({});
  vm.runInContext(`${body}\nvar why = reelWhyNot;`, ctx);
  for (const c of V.why) assert.equal(ctx.why(c.button, c.snap), c.expect, c.name);
});

test("importCountText: the shared vectors", () => {
  const body = pure(ed, "import-count");
  assert.ok(body, "MyGamesEditor.qml has no import-count pure block");
  const ctx = vm.createContext({});
  vm.runInContext(`${body}\nvar f = importCountText;`, ctx);
  for (const c of V.importCount) assert.equal(ctx.f(c.total, c.pending), c.expect, `${c.total}/${c.pending}`);
});

test("wiring", () => {
  for (const b of ["spin", "veto"]) {
    assert.match(panel, new RegExp(`HoverHandler \\{ id: ${b}WhyHover \\}`), `${b}: its slot hovers`);
    assert.match(panel, new RegExp(`!enabled && whyNot\\.length > 0 && ${b}WhyHover\\.hovered`), `${b}: greyed + hovered → why`);
    assert.match(panel, new RegExp(`panel\\.reelWhyNot\\("${b}", panel\\._whySnap\\)`));
  }
  assert.match(ed, /ed\.importCountText\(\(impRow\.imp\.items \|\| \[\]\)\.length, Number\(impRow\.imp\.pending\) \|\| 0\)/);
  // Enter: the first result, only when the results are this text's (a pending search keeps the last ones on screen)
  assert.match(ed, /else if \(ed\.resultsCurrent\(text\.trim\(\)\)\) ed\.addTo\("wouldPlay", ed\.results\[0\]\)/);
  assert.match(ed, /function resultsCurrent\(q\) \{ return !searchDebounce\.running && String\(ed\.ctl\.searchFor \|\| ""\) === q \}/);
  const games = file("../crates/app/src/games.rs");
  assert.match(games, /set_search_for\(QString::from\(&q\)\);\s*o\.as_mut\(\)\.set_search_json/, "a search's answer names its text");
  assert.match(games, /set_search_for\(QString::default\(\)\); \/\/ the added game/, "an added game is no search's answer");
});
