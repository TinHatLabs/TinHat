// room-filter-test.mjs — the room list's filter (ACTIONS.md §10.8; QOL L3, 2026-11-14), the desktop mirror of the web's
// src/features/nav/roomFilter.test.ts. The matcher itself is the shared engine's roomFilter (crates/actions), proven byte
// for byte by palette/roomfilter.json in crates/actions/tests/vectors.rs. This file checks the QML around it: the row
// mapping (ThRoomList.qml _filterRow, between its BEGIN filter-row / END filter-row markers: the exact code the app
// runs), that every place that asks "does the filter show this room" asks the shared filter (no private lower-casing
// copies left: the list rows, the empty state's count, the keyboard walk — a11y round 2's kbMatches), and P1: typing
// never opens a room (no auto-accept), Enter acts on the highlighted first match as a click on it does.
//   node --test tools/room-filter-test.mjs     (scripts/run-gates.sh runs it with the unit tests)
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const here = (p) => fileURLToPath(new URL(p, import.meta.url));
const list = readFileSync(here("../crates/app/qml/ThRoomList.qml"), "utf8");
const island = readFileSync(here("../crates/app/qml/RoomsIsland.qml"), "utf8");
const body = list.split("// BEGIN filter-row")[1]?.split("// END filter-row")[0];
assert.ok(body, "ThRoomList.qml lost its BEGIN filter-row / END filter-row markers");
const ctx = vm.createContext({});
vm.runInContext(`${body}\nvar out = { _filterRow: _filterRow };`, ctx);
const { _filterRow } = ctx.out;
const plain = (v) => JSON.parse(JSON.stringify(v));

test("a row as the shared matcher sees it (the web's filterRowOf)", () => {
  assert.deepEqual(plain(_filterRow({ id: "!a", name: "Alex", alias: "", peer: "@alexw:example.org", membership: "join", unread: 2, highlight: true })),
    { name: "Alex", aliases: ["@alexw:example.org"], membership: "join", unread: true, mention: true });
  assert.deepEqual(plain(_filterRow({ id: "!b", name: "Café", alias: "#cafe:example.org", peer: "", membership: "none", unread: 0, highlight: false })),
    { name: "Café", aliases: ["#cafe:example.org"], membership: "none", unread: false, mention: false });
  // a row without the fields: an empty name, joined, nothing unread
  assert.deepEqual(plain(_filterRow({ id: "!c" })), { name: "", aliases: [], membership: "join", unread: false, mention: false });
});

test("every filter question goes to the shared filter (no private matcher copies)", () => {
  for (const [name, src] of [["ThRoomList.qml", list], ["RoomsIsland.qml", island]]) {
    assert.doesNotMatch(src, /toLowerCase\(\)\.indexOf\(q\)/, `${name} still lower-cases the query itself`);
  }
  assert.match(list, /actionsCtl\.call\("roomFilter", JSON\.stringify\(\{ query: q, rooms: rows \}\)\)/);
  assert.match(list, /readonly property int _matchCount:[\s\S]{0,200}panel\.roomShown\(roomModel\.get\(i\)\.room\)/);
  assert.match(island, /function kbMatches\(i\) \{[^\n]*\n\s*return island\.panelRef\.roomShown\(island\.roomModelRef\.get\(i\)\.room\)/);
  assert.match(island, /readonly property bool matches: island\.panelRef\.roomShown\(room\)/);
});

test("typing never opens a room (P1): no auto-accept; Enter = a click on the highlighted first match", () => {
  const field = island.slice(island.indexOf("id: filterField"), island.indexOf("Keys.onDownPressed", island.indexOf("id: filterField")));
  assert.match(field, /autoAccept: false/, "the filter auto-accepts again");
  assert.match(field, /focusSequences: \[\]/, "Kirigami's own Ctrl+F would make the table's key ambiguous");
  assert.match(field, /onAccepted: island\.acceptFilter\(\)/);
  assert.doesNotMatch(list + island, /openFirstMatch/, "the old auto-open path is back");
  const acc = island.slice(island.indexOf("function acceptFilter()"), island.indexOf("// Ctrl+K on a pending invite"));
  assert.match(acc, /island\.panelRef\.filterFirstId/);
  assert.match(acc, /island\.rowGesture\(\{ gesture: "click", membership: String\(r\.membership \|\| ""\) \}\)/);
  assert.match(acc, /if \(g === "askJoin"\) island\.panelRef\.joinRequested\(r\.id, r\.name\)\s*else if \(g === "open"\) island\.panelRef\.roomActivated\(r\.id\)/);
  // the highlight: the first match wears the hover wash
  assert.match(island, /readonly property bool filterFirst: island\.panelRef\.filterFirstId\.length > 0 && room\.id === island\.panelRef\.filterFirstId/);
  assert.match(island, /rowMa\.containsMouse \|\| row\.kbCurrent \|\| row\.filterFirst\) \? Th\.roomsItemBgHover/);
});

test("no match is no dead end (A10.3): Search everywhere opens Ctrl+K with the text typed", () => {
  const main = readFileSync(here("../crates/app/qml/Main.qml"), "utf8");
  assert.match(island, /visible: island\.panelRef\.query\.trim\(\)\.length > 0[\s\S]{0,200}text: "Search everywhere"\s*onClicked: \{ island\.disarm\(\); island\.app\.searchEverywhere\(island\.panelRef\.query\) \}/);
  assert.match(main, /function searchEverywhere\(text\) \{[\s\S]{0,120}quickSwitcher\.open\(\)\s*Qt\.callLater\(function \(\) \{ quickSwitcher\.setQuery\(t\) \}\)/);
});
