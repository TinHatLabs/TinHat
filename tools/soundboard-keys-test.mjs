// soundboard-keys-test.mjs — the soundboard's keys (QOL A16.0 / product call P12 (a), A16.1): runs the pure sbKeyAction of
// crates/app/qml/SoundboardPopover.qml (between its BEGIN soundboard-keys pure / END soundboard-keys pure markers) on the
// shared vectors crates/app/src/testvectors/soundboard-keys.json (Tin Hat Web replays the same file:
// src/features/media/soundboard/keys.test.ts), and checks that the board and the search box both go through it.
//   node --test tools/soundboard-keys-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const pop = file("../crates/app/qml/SoundboardPopover.qml");
const V = JSON.parse(file("../crates/app/src/testvectors/soundboard-keys.json"));

test("sbKeyAction: the shared vectors", () => {
  const body = pop.split("// BEGIN soundboard-keys pure")[1]?.split("// END soundboard-keys pure")[0]?.replace(/^[^\n]*\n/, "");
  assert.ok(body, "SoundboardPopover.qml has no BEGIN soundboard-keys pure / END soundboard-keys pure block");
  const ctx = vm.createContext({});
  vm.runInContext(`${body}\nvar act = sbKeyAction;`, ctx);
  assert.ok(V.cases.length >= 15);
  for (const c of V.cases) assert.equal(ctx.act(c.key, c.mods, c.inSearch, c.query, c.counts), c.expect, c.name);
});

test("the board and the search box both go through it", () => {
  assert.match(pop, /!searchField\.activeFocus && sheet\.runKey\(ev, false\)/, "the board (focus outside the box)");
  const box = pop.split("id: searchField")[1]?.split("Controls.ToolButton")[0] ?? "";
  assert.match(box, /sheet\.runKey\(ev, true\)/, "the search box");
  assert.doesNotMatch(box, /playHotkey\(1\)/, "Enter no longer plays favourite 1 only");
  assert.match(pop, /function runKey\(ev, inSearch\)[\s\S]{0,200}sheet\.sbKeyAction\(/);
});
