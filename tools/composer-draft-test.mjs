// composer-draft-test.mjs — L1 "don't lose what I typed": runs the composer's draft rules from
// crates/app/qml/ConversationView.qml (between its BEGIN composer-draft pure / END composer-draft pure markers, the code
// the app runs) on the shared vectors testvectors/composer-draft/vectors.json (Tin Hat Web replays the same file:
// src/features/chat/lib/composerDraft.test.ts), then drives the view's own start/cancel/send functions on a fake view.
//   A09.0 the bar's ✕ = Escape · A09.1 an edit sets the draft aside and gives it back · A09.4 unchanged / empty edits
//   A20.0 a refused edit comes back into the box · A20.1 a GIF that didn't go says so
//   node --test tools/composer-draft-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const qml = file("../crates/app/qml/ConversationView.qml");
const V = JSON.parse(file("../testvectors/composer-draft/vectors.json"));
const pure = qml.split("// BEGIN composer-draft pure")[1]?.split("// END composer-draft pure")[0];

test("draftStep / draftToKeep / failText: the shared vectors", () => {
  assert.ok(pure, "ConversationView.qml has no BEGIN composer-draft pure / END composer-draft pure block");
  const ctx = vm.createContext({});
  vm.runInContext(pure, ctx);
  assert.ok(V.cases.length >= 20);
  for (const c of V.cases) {
    let st = c.start;
    const fx = [];
    for (const op of c.ops) {
      const r = ctx.draftStep(st, op);
      st = r.st;
      fx.push(r.fx);
    }
    assert.deepEqual(fx, c.fx, c.name);
    assert.deepEqual(JSON.parse(JSON.stringify(st)), c.end, c.name);
  }
  for (const c of V.keep) assert.equal(ctx.draftToKeep(c.state), c.draft, c.name);
  for (const c of V.fail) assert.equal(ctx.failText(c.kind, c.class, c.putBack), c.text, `${c.kind} ${c.class}`);
});

/** The view's member functions NAMES (4-space indent), rewritten as conv.NAME = function. */
function members(names) {
  return names
    .map((n) => {
      const m = qml.match(new RegExp(`\\n {4}function ${n}\\([^)]*\\) \\{[^\\n]*\\}\\n|\\n {4}function ${n}\\([^)]*\\) \\{[\\s\\S]*?\\n {4}\\}\\n`));
      assert.ok(m, `ConversationView.qml lost ${n}`);
      return m[0].replace(/\n {4}function (\w+)\(/, "\nconv.$1 = function (");
    })
    .join("\n");
}

function view() {
  const ctx = vm.createContext({ console });
  vm.runInContext(
    `var log = [];
     var conv = { replyToId: "", replyToBody: "", replyToName: "", editToId: "", editToBody: "", staged: [], _draftStash: null,
                  primary: true, visible: true, _roomIdSeen: "!r", stopTyping: function () {},
                  room: { roomId: "!r", editMessage: function (id, t) { log.push("edit:" + id + ":" + t) }, editMessageMentions: function (id, t) { log.push("edit:" + id + ":" + t) }, send: function (t) { log.push("send:" + t) },
                          sendReply: function (id, t) { log.push("reply:" + id + ":" + t) }, sendEmote: function (t) { log.push("emote:" + t) },
                          sendWithMentions: function (t) { log.push("mention:" + t) }, sendReplyMentions: function (id, t) { log.push("reply:" + id + ":" + t) },
                          sendAttachment: function (u) { log.push("file:" + u) } },
                  app: { notify: function (t) { log.push("notify:" + t) }, playUiSound: function () {} } };
     var composer = { text: "", _mentions: [], cursorPosition: 0, forceActiveFocus: function () {} };
     var msgView = { kbHold: false, stickBottom: false, positionViewAtEnd: function () {} };
     var mentionPopup = { close: function () {} }, emojiPopup = { close: function () {} };
     var Qt = { callLater: function () {} };
     ${pure.replace(/\n {4}function (\w+)\(/g, "\nconv.$1 = function (")}
     ${members(["_draftState", "_draftApply", "_draft", "_notify", "_failList", "_newestSeq", "_eachNew", "_opFailed", "_gifFailed", "_expandSlash", "startReply", "startEdit", "cancelTarget", "sendComposer"])}`,
    ctx,
  );
  return ctx;
}

test("the view: an edit sets the draft aside; cancel and save give it back (A09.1)", () => {
  const v = view();
  v.composer.text = "my half-written line";
  v.conv.startEdit("$m", "old words");
  assert.equal(v.composer.text, "old words");
  assert.equal(v.conv.editToId, "$m");
  v.conv.cancelTarget();
  assert.equal(v.composer.text, "my half-written line", "cancelling the edit must give the draft back");
  assert.equal(v.conv.editToId, "");
  v.conv.startEdit("$m", "old words");
  v.composer.text = "new words";
  v.conv.sendComposer();
  assert.deepEqual([...v.log], ["edit:$m:new words"]);
  assert.equal(v.composer.text, "my half-written line", "saving the edit must give the draft back");
});

test("the view: unchanged and empty edits (A09.4)", () => {
  const v = view();
  v.conv.startEdit("$m", "same");
  v.conv.sendComposer();
  assert.deepEqual([...v.log], [], "an unchanged edit is not sent");
  assert.equal(v.conv.editToId, "");
  v.conv.startEdit("$m", "same");
  v.composer.text = " ";
  v.conv.sendComposer();
  assert.match(v.log[0], /^notify:An edit can't be empty\./);
  assert.equal(v.conv.editToId, "$m", "the empty edit stays open");
});

test("the view: a refused edit comes back; a refused delete / GIF says so in plain words (A20.0 / A20.1)", () => {
  const v = view();
  v.composer.text = "next line";
  v.conv._opFailed({ seq: 1, room: "!r", kind: "edit", id: "$m", text: "fixed typo", cls: "network" });
  assert.equal(v.composer.text, "fixed typo");
  assert.equal(v.conv.editToId, "$m");
  assert.deepEqual([...v.log], ["notify:Your edit wasn't saved (no connection). It's back in the message box."]);
  v.conv.cancelTarget();
  assert.equal(v.composer.text, "next line", "the draft typed meanwhile is back");
  v.conv._opFailed({ seq: 2, room: "!other", kind: "edit", id: "$m", text: "x", cls: "other" });
  assert.equal(v.log.length, 1, "another room's failure is not this view's");
  v.conv._opFailed({ seq: 3, room: "!r", kind: "delete", id: "$m", text: "", cls: "forbidden" });
  v.conv._gifFailed({ seq: 1, room: "!r", cls: "other" });
  assert.deepEqual([...v.log].slice(1), ["notify:The message wasn't deleted (you don't have permission here).", "notify:Your GIF wasn't sent."]);
});

test("the view: the room remembers the set-aside draft, never an edit's text; failures are polled", () => {
  assert.match(qml, /draft: conv\.draftToKeep\(conv\._draftState\(\)\)/);
  assert.match(qml, /if \(\(conv\.room\.opFailedJson \|\| ""\) !== conv\._opFailRaw\) \{ conv\._opFailRaw = conv\.room\.opFailedJson \|\| ""; conv\._opFailSeq = conv\._eachNew\(conv\._opFailRaw, conv\._opFailSeq, conv\._opFailed\) \}/);
  assert.match(qml, /if \(conv\.gif && \(conv\.gif\.sendFailedJson \|\| ""\) !== conv\._gifFailRaw\)/);
  // two failures between polls are both handled, each once
  const v = view();
  const seen = [];
  const list = JSON.stringify([{ seq: 4, room: "!r" }, { seq: 5, room: "!r" }, { seq: 6, room: "!r" }]);
  const n = v.conv._eachNew(list, 4, (f) => seen.push(f.seq));
  assert.deepEqual(seen, [5, 6]);
  assert.equal(n, 6);
  assert.equal(v.conv._newestSeq(list), 6);
  assert.equal(v.conv._newestSeq("garbage"), 0);
  // a room switch takes the controller's last failure as seen (no stale notice) and drops the stash
  assert.match(qml, /conv\._draftStash = null[^\n]*\n\s+conv\._opFailRaw = conv\.room \? \(conv\.room\.opFailedJson \|\| ""\) : ""/);
  // the edit no longer overwrites the box from a property handler (the draft was lost there)
  assert.doesNotMatch(qml, /onEditToIdChanged: if \(conv\.editToId\.length > 0\) \{ composer\.text = conv\.editToBody/);
  const room = file("../crates/app/src/room.rs");
  assert.doesNotMatch(room, /format!\("Edit failed: \{e\}"\)|format!\("Delete failed: \{e\}"\)/, "no raw SDK error on screen");
  assert.match(file("../crates/app/src/gif.rs"), /set_send_failed_json/);
});
