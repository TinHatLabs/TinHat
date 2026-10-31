// room-row-gesture-test.mjs — 12.1.5 (double-click a room → its call): runs RoomsIsland.qml's rowGesture (between its
// BEGIN pure / END pure markers, the exact code the app runs) on crates/app/testvectors/room-row-gesture/vectors.json, a
// byte copy of the web's src/features/nav/testvectors/room-row-gesture.json (the web's rowGesture.test.ts replays it too),
// and checks the row's MouseArea routes a click and a double-click through it.
//   node --test tools/room-row-gesture-test.mjs     (scripts/run-gates.sh runs it with the unit tests)
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const here = (p) => fileURLToPath(new URL(p, import.meta.url));
const qml = readFileSync(here("../crates/app/qml/RoomsIsland.qml"), "utf8");
const body = qml.split("// BEGIN pure")[1]?.split("// END pure")[0]?.replace(/^[^\n]*\n/, "");
assert.ok(body, "RoomsIsland.qml lost its BEGIN pure / END pure markers");
const ctx = vm.createContext({});
vm.runInContext(`${body}\nvar island = { rowGesture: rowGesture };`, ctx);
const { rowGesture } = ctx.island;
const RAW = readFileSync(here("../crates/app/testvectors/room-row-gesture/vectors.json"));
const V = JSON.parse(RAW.toString("utf8"));
// the web's copy pins the same hash (src/features/nav/rowGesture.test.ts): change both files together
const SHA = "f23ef1e5bf62925aaa81d7eca9c0888e727851505c3edf33d47b47a818ecad85";

test("room-row-gesture shared vectors", () => {
  assert.equal(V.format, "tinhat-room-row-gesture-vectors");
  assert.ok(V.cases.length >= 10);
  for (const c of V.cases) assert.equal(rowGesture(c), c.want, JSON.stringify(c));
});

test("the vectors are the web's byte copy", () => {
  assert.equal(createHash("sha256").update(RAW).digest("hex"), SHA);
});

test("click opens, double-click joins the call, never on an invite or a room you are not in", () => {
  assert.equal(rowGesture({ gesture: "click", membership: "join" }), "open");
  assert.equal(rowGesture({ gesture: "dblclick", membership: "join", touch: false, armed: true }), "call");
  assert.equal(rowGesture({ gesture: "dblclick", membership: "join", touch: false }), "nothing"); // not armed: it started elsewhere
  assert.equal(rowGesture({ gesture: "dblclick", membership: "invite", touch: false, armed: true }), "nothing");
  assert.equal(rowGesture({ gesture: "dblclick", membership: "none", touch: false }), "nothing");
  assert.equal(rowGesture({ gesture: "click", membership: "none" }), "askJoin");
});

// The wiring: the row's MouseArea sends a click and a double-click through rowGesture, the double-click only on the
// left button, and "call" goes up as roomCallRequested → Main.qml runs the call.join action (the call button's path).
test("the row and Main.qml route the double-click to call.join", () => {
  const ma = qml.slice(qml.indexOf("id: rowMa"), qml.indexOf("Controls.ToolTip.text", qml.indexOf("id: rowMa")));
  assert.match(ma, /function rowClick\(m, mayArm\)[\s\S]*rowGesture\(\{ gesture: "click"/);
  assert.match(ma, /onClicked:[\s\S]*rowMa\.rowClick\(m, true\)/);
  // 12.1.5 review: the second PRESS (doubleClicked) only marks what its release does; the release over the row joins
  // (armed) or runs the ordinary click (never arming); a cancel drops it
  assert.match(ma, /onDoubleClicked:[\s\S]*island\.disarm\(\)\s*island\.clearPending\(\)\s*if \(m\.button !== Qt\.LeftButton\) return[\s\S]*rowGesture\(\{ gesture: "dblclick"[\s\S]*armed: armed \}\) === "call" \? "call" : "click"\s*island\.pendingRoom = row\.room\.id/);
  assert.doesNotMatch(ma.slice(ma.indexOf("onDoubleClicked:"), ma.indexOf("onReleased:")), /roomCallRequested/); // never on the press
  assert.match(ma, /onReleased:[\s\S]*island\.pendingRoom !== row\.room\.id[\s\S]*island\.clearPending\(\)[\s\S]*!rowMa\.containsMouse\) return[\s\S]*if \(what === "call"\) island\.panelRef\.roomCallRequested\(row\.room\.id\)\s*else rowMa\.rowClick\(m, false\)/);
  assert.match(ma, /onCanceled: if \(island\.pendingRoom === row\.room\.id\) island\.clearPending\(\)/);
  // the pending action belongs to the press that set it (a leftover never fires on a later release)
  assert.match(ma, /onPressed: island\.pressSeq\+\+/);
  assert.match(ma, /island\.pendingSeq = island\.pressSeq/);
  assert.match(ma, /onReleased:[\s\S]*island\.pendingSeq !== island\.pressSeq\) \{ island\.clearPending\(\); return \}/);
  assert.match(qml, /function disarm\(\) \{ island\.armedRoom = ""; island\.armedAt = 0; island\.clearPending\(\) \}/);
  // ONE arm for the list: set only by a joined row's own left click, checked against this row, a non-negative age within
  // the platform interval, consumed by the double-click; a right click disarms; another room on screen disarms
  assert.match(ma, /if \(mayArm && row\.membershipKey === "join" && !row\.fromTouch\(m\)\) \{ island\.armedRoom = row\.room\.id; island\.armedAt = Date\.now\(\) \}\s*else island\.disarm\(\)/);
  assert.match(ma, /island\.armedRoom === row\.room\.id && island\.armedAt > 0 && age >= 0 && age <= Qt\.styleHints\.mouseDoubleClickInterval/);
  assert.match(ma, /RightButton\) \{ island\.disarm\(\)/);
  assert.match(qml, /function onCurrentRoomIdChanged\(\) \{ if \(island\.panelRef\.currentRoomId !== island\.armedRoom\) island\.disarm\(\); island\.clearPending\(\) \}/);
});

test("every control in the room list disarms on press", () => {
  // the header + and gear, the filter, Accept, Decline, the now-playing pencil, editor and clear
  assert.equal((qml.match(/onPressed: island\.disarm\(\)/g) || []).length, 8);
  assert.doesNotMatch(qml, /row\.armedAt/); // no per-row arm left
  const nav = readFileSync(here("../crates/app/qml/ThNav.qml"), "utf8");
  assert.match(nav, /onRoomCallRequested: \(roomId\) => nav\.roomCallRequested\(roomId\)/);
  const main = readFileSync(here("../crates/app/qml/Main.qml"), "utf8");
  const h = main.slice(main.indexOf("onRoomCallRequested:"), main.indexOf("myMxid: login.userId", main.indexOf("onRoomCallRequested:")));
  assert.match(h, /root\.actions\.run\("call\.join", \{ source: "host", target: \{ room: roomId \} \}\)/);
  // no other path: the handler never calls the call controller directly
  assert.doesNotMatch(h, /callctl\./);
});
