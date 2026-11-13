// two-pane-test.mjs — the two-pane band's rules (docs/TWO-PANE.md): runs the pure functions of crates/app/qml/Main.qml
// (between its BEGIN two-pane pure / END two-pane pure markers, the exact code the app runs) on the shared vectors
// testvectors/two-pane/vectors.json (Tin Hat Web replays the same file: tools/two-pane-unit.mts), and checks that the call
// bar wires them (RowModel's fold, CallIsland's More button).
//   node --test tools/two-pane-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const main = file("../crates/app/qml/Main.qml");
const body = main.split("// BEGIN two-pane pure")[1]?.split("// END two-pane pure")[0];
assert.ok(body, "Main.qml lost its BEGIN two-pane pure / END two-pane pure markers");
const ctx = vm.createContext({});
vm.runInContext(`${body}\nvar tp = { twoPaneSizes: twoPaneSizes, barFit: barFit, foldKeeps: typeof foldKeeps === "function" ? foldKeeps : null };`, ctx);
const { twoPaneSizes, barFit, foldKeeps } = ctx.tp;
const V = JSON.parse(file("../testvectors/two-pane/vectors.json"));
const schema = JSON.parse(file("../crates/layout/layout.schema.json"));
const classic = schema["x-tinhat"].presets.classic.desktop;
const r4 = (x) => Math.round(x * 10000) / 10000;

test("twoPaneSizes: the shared vectors", () => {
  for (const c of V.twoPane) {
    const got = twoPaneSizes(c.form === "classic" ? classic : c.form, c.hostW, c.editing, c.unitPm);
    assert.equal(JSON.stringify(got ?? null), JSON.stringify(c.out), c.name);
  }
});

test("barFit: the shared vectors", () => {
  for (const c of V.barFit) {
    const got = barFit(c.in);
    assert.deepEqual({ fold: got.fold, scale: r4(got.scale) }, c.out, c.name);
  }
});

test("foldKeeps: the shared vectors (UI-1: a hold item never moves into More)", () => {
  assert.equal(typeof foldKeeps, "function", "Main.qml's pure block has no foldKeeps");
  assert.ok(Array.isArray(V.foldKeep) && V.foldKeep.length >= 8, "the vectors lost foldKeep");
  for (const c of V.foldKeep) assert.equal(foldKeeps(c.item, c.keep), c.out, c.name);
});

test("UI-1: the fold and the More menu use foldKeeps; a hold action needs its press or release", () => {
  const island = file("../crates/app/qml/CallIsland.qml");
  const row = file("../crates/app/qml/RowModel.qml");
  const host = file("../crates/app/qml/ActionsHost.qml");
  // RowModel folds only what foldKeeps lets move, and nothing without the rule
  assert.match(row, /rm\.app && typeof rm\.app\.foldKeeps === "function" \? rm\.app\.foldKeeps\(x, rm\.keep\) : true/);
  assert.match(row, /r\.items\[f\]\.state !== "hidden" && !keeps\(r\.items\[f\]\)\) canFold\.push\(f\)/);
  assert.doesNotMatch(row, /rm\.keep\.indexOf\(r\.items\[f\]\.a\) < 0\) canFold/);
  // the measured widths are keyed by the same rule
  assert.match(island, /var kept = island\.app && typeof island\.app\.foldKeeps === "function" \? island\.app\.foldKeeps\(cc\.cell\.item, brow\.keep\) : true\n\s+if \(!kept\) w\[/);
  // a hold row in More is inert
  assert.match(island, /enabled: modelData\.item\.state !== "dimmed" && modelData\.item\.hold !== true/);
  assert.match(island, /onTriggered: if \(modelData\.item\.hold !== true\) brow\.run\(modelData\.item, moreBtn\)/);
  // only an explicit "down" opens the mic
  assert.match(host, /case "call\.pushToTalk\.hold": if \(ctx\.hold === "down" \|\| ctx\.hold === "up"\) app\.callctl\.pttHold\(ctx\.hold === "down"\); return true/);
  assert.doesNotMatch(host, /pttHold\(ctx\.hold !== "up"\)/);
});

test("the call bar and the dock view use them", () => {
  const island = file("../crates/app/qml/CallIsland.qml");
  const row = file("../crates/app/qml/RowModel.qml");
  assert.match(island, /island\.app\.barFit\(\{ fullW: full, availW: parent\.width - Kirigami\.Units\.largeSpacing,\s+minScale: 0\.7, widths: widths, more: moreW, fold0: k0 \}\)/);
  assert.match(island, /full \+= widths\[j\]; full -= moreW/);
  // Tab order follows what you see: More is declared before Leave
  assert.ok(island.indexOf('objectName: "callBarMore"') < island.indexOf("CallButton { // hang up"), "More must come before Leave in the focus chain");
  assert.match(island, /keep: \["call\.mic\.toggle", "call\.leave"\]/);
  assert.match(island, /Layout\.column: Math\.max\(0, brow\.moreCol\)/);
  assert.match(row, /property int foldCount: 0/);
  assert.match(main, /var r = root\.twoPaneSizes\(f, dockHost\.width, editModeObj\.open, dockHost\.unitPm\)/);
  assert.match(main, /if \(!editModeObj\.open && root\.twoPaneKey\.length > 0\) v\.sizes = JSON\.parse\(root\.twoPaneKey\)/);
});
