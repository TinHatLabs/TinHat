// reactions-test.mjs — reactions (QOL A07.2 / A31.1), the QML half: runs ChatBubble.qml's pure reactNorm (between its
// BEGIN react-norm pure / END react-norm pure markers) on the shared vectors crates/app/src/testvectors/reactions.json (the
// grouping itself is reactions.rs, whose unit tests replay the same file; Tin Hat Web: src/features/chat/lib/reactions.test.ts),
// and checks the quick bar matches by it and that a chip is a named, keyboard-reachable toggle.
//   node --test tools/reactions-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const bubble = file("../crates/app/qml/ChatBubble.qml");
const room = file("../crates/app/src/room.rs");
const V = JSON.parse(file("../crates/app/src/testvectors/reactions.json"));

test("reactNorm: the shared vectors", () => {
  const body = bubble.split("// BEGIN react-norm pure")[1]?.split("// END react-norm pure")[0]?.replace(/^[^\n]*\n/, "");
  assert.ok(body, "ChatBubble.qml has no BEGIN react-norm pure / END react-norm pure block");
  const ctx = vm.createContext({});
  vm.runInContext(`${body}\nvar norm = reactNorm;`, ctx);
  for (const c of V.norm) assert.equal(ctx.norm(c.in), c.out, JSON.stringify(c.in));
});

test("the quick bar and the optimistic chip match by the grouping form; the model groups through reactions.rs", () => {
  assert.match(bubble, /bubble\.reactNorm\(r\[i\]\.key\) === bubble\.reactNorm\(modelData\)/);
  assert.match(room, /crate::reactions::group_chips\(&oldest_first, &my_id\)/);
  assert.match(room, /map\(crate::reactions::react_key_norm\)/);
});

test("a chip is a named toggle for the keyboard", () => {
  const chip = bubble.split("Reactions row slot")[1] ?? "";
  assert.match(chip, /activeFocusOnTab: true/);
  assert.match(chip, /Accessible\.role: Accessible\.Button/);
  assert.match(chip, /Accessible\.checked: modelData\.mine === true/);
  assert.match(chip, /Accessible\.name: \(modelData\.key[^\n]*bubble\.reactWho\(modelData\)/);
  assert.match(chip, /Keys\.onSpacePressed: toggle\(\)/);
});

test("taking a merged chip back redacts every one of mine (❤ and ❤️ both sent)", () => {
  assert.match(bubble, /function unreactChip\(chip\) \{[\s\S]{0,200}chip\.reactionIds[\s\S]{0,200}bubble\.unreact\(String\(ids\[i\]\)\)/);
  assert.equal((bubble.match(/bubble\.unreactChip\(/g) || []).length, 2, "the chip and the quick bar");
});
