// palette-choice-test.mjs — what Enter on a Ctrl+K row does (P6 "Find people named …" → New DM, 2026-11-14): runs
// QuickSwitcher.qml's `choice` (between its BEGIN choice / END choice markers, the exact code the app runs) on
// crates/app/testvectors/palette-choice/vectors.json, a byte copy of the web's src/features/nav/testvectors/
// palette-choice.json (the web's paletteChoice.test.ts replays it too), and checks the find row reaches New DM with
// its text in the people picker.
//   node --test tools/palette-choice-test.mjs     (scripts/run-gates.sh runs it with the unit tests)
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const here = (p) => fileURLToPath(new URL(p, import.meta.url));
const qml = readFileSync(here("../crates/app/qml/QuickSwitcher.qml"), "utf8");
const main = readFileSync(here("../crates/app/qml/Main.qml"), "utf8");
const body = qml.split("// BEGIN choice")[1]?.split("// END choice")[0];
assert.ok(body, "QuickSwitcher.qml lost its BEGIN choice / END choice markers");
const ctx = vm.createContext({});
vm.runInContext(`${body}\nvar out = { choice: choice };`, ctx);
const { choice } = ctx.out;
const RAW = readFileSync(here("../crates/app/testvectors/palette-choice/vectors.json"));
const V = JSON.parse(RAW.toString("utf8"));
// the web's copy pins the same hash (src/features/nav/paletteChoice.test.ts): change both files together
const SHA = "da3d7a921ef259845f4b64bd59b08f6a1e42d3aed335e93f1a9938c8a1c82617";

test("palette-choice shared vectors", () => {
  assert.equal(V.format, "tinhat-palette-choice-vectors");
  for (const c of V.cases) assert.deepEqual(JSON.parse(JSON.stringify(choice(c.it ?? undefined, c.picking))), c.want, c.name);
});

test("the vectors are the web's byte copy", () => {
  assert.equal(createHash("sha256").update(RAW).digest("hex"), SHA);
});

test("the find row opens New DM with its text in the people picker", () => {
  assert.match(qml, /var c = qs\.choice\(it, !!p\)/);
  assert.match(qml, /c\.do === "newDm"[\s\S]{0,160}qs\.findPeople\(c\.text\)/);
  assert.match(main, /onFindPeople: \(text\) => root\.openNewDm\(text\)/);
  assert.match(main, /function openNewDm\(text\) \{ newDmDialog\.prefill = String\(text \|\| ""\); newDmDialog\.open\(\) \}/);
  assert.match(main, /onOpened: \{ newDmPicker\.reset\(newDmDialog\.prefill\); newDmDialog\.prefill = "" \}/);
});
