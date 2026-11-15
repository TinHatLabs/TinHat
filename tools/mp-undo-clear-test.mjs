// mp-undo-clear-test.mjs — Media Party: Undo after Clear (QOL A16.2, product call P11 (a)): the window's Clear goes
// through clearWithUndo (a 5 s "Queue cleared · Undo" bar), Undo hands the cleared list to watchCtl.restoreQueue
// (watchparty.rs queue_undo_clear; its unit test undoclear_vectors replays crates/app/testvectors/mediaparty/undoclear.json,
// byte-identical with Tin Hat Web's testvectors/mediaparty/undoclear.json).
//   node --test tools/mp-undo-clear-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const win = file("../crates/app/qml/WatchPartyWindow.qml");
const main = file("../crates/app/qml/Main.qml");
const rs = file("../crates/app/src/watchparty.rs");

test("Clear goes through the undo bar", () => {
  assert.match(win, /text: "Clear"[\s\S]{0,400}onClicked: sheet\.clearWithUndo\(\)/);
  const fn = win.split("function clearWithUndo()")[1]?.split("\n    }")[0] ?? "";
  assert.match(fn, /_undoJson = JSON\.stringify\(sheet\.queue\)[\s\S]*sheet\.clearQueue\(\)[\s\S]*undoT\.restart\(\)/);
  assert.match(win, /id: undoT; interval: 5000/);
  assert.match(win, /text: "Queue cleared"/);
  assert.match(win, /text: "Undo"[\s\S]{0,120}onClicked: sheet\.undoClear\(\)/);
  assert.match(win, /function undoClear\(\)[\s\S]{0,300}sheet\._partyRoom === sheet\._undoRoom\)\s*sheet\.restoreQueue\(sheet\._undoRoom, sheet\._undoJson\)/);
  assert.match(win, /on_PartyRoomChanged: sheet\._dropUndo\(\)/, "another party drops the Undo");
});

test("Undo reaches the controller", () => {
  assert.match(main, /onRestoreQueue: \(rid, json\) => watchCtl\.restoreQueue\(rid, json\)/);
  assert.match(rs, /#\[cxx_name = "restoreQueue"\]\s*fn restore_queue\(self: Pin<&mut WatchPartyController>, room_id: &QString/);
  assert.match(rs, /return; \/\/ the Clear was in another party/);
  assert.match(rs, /queue_undo_clear\(q, &cleared\);\s*true/, "a restore always writes (the base may be the pre-Clear copy)");
  assert.match(rs, /fn queue_writes\(\)/, "queue writes run one at a time");
  // one handler per signal (QML refuses a second onInPartyChanged in the same object)
  assert.equal((win.match(/^    onInPartyChanged:/gm) || []).length, 1);
});
