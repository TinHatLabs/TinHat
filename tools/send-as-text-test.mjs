// send-as-text-test.mjs — 4-4 (L1 × L8 P21): "Send as text" sends exactly what the question showed. The question
// ("Unknown command — send as text?") takes a snapshot when it opens (_sendSnap); its confirm sends only when the room,
// text, reply/edit target, picked people and staged files still match (draftSame, the shared vectors
// testvectors/composer-draft "same"); otherwise nothing is sent and the box says so. The bug: a refused edit put back
// while the question was open made "Send as text" save THAT edit. Tin Hat Web: src/features/chat/sendAsText.test.ts.
//   node --test tools/send-as-text-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const qml = file("../crates/app/qml/ConversationView.qml");
const V = JSON.parse(file("../testvectors/composer-draft/vectors.json"));
const pure = qml.split("// BEGIN composer-draft pure")[1].split("// END composer-draft pure")[0];

function members(names) {
  return names
    .map((n) => {
      const m = qml.match(new RegExp(`\\n {4}function ${n}\\([^)]*\\) \\{[^\\n]*\\}\\n|\\n {4}function ${n}\\([^)]*\\) \\{[\\s\\S]*?\\n {4}\\}\\n`));
      assert.ok(m, `ConversationView.qml lost ${n}`);
      return m[0].replace(/\n {4}function (\w+)\(/, "\nconv.$1 = function (");
    })
    .join("\n");
}

test("draftSame: the shared vectors", () => {
  const ctx = vm.createContext({});
  vm.runInContext(pure, ctx);
  assert.equal(typeof ctx.draftSame, "function", "the pure block has no draftSame");
  assert.ok(V.same.length >= 8);
  for (const c of V.same) assert.equal(ctx.draftSame(c.asked, c.now), c.same, c.name);
});

test("4-4: a refused edit put back while the question is open: Send as text sends nothing", () => {
  const confirm = qml.match(/id: slashConfirm[\s\S]*?\n {8}onConfirmed: (\{[\s\S]*?\n {8}\}|[^\n]*)/)?.[1];
  assert.ok(confirm, "no slashConfirm onConfirmed");
  const ctx = vm.createContext({ console });
  vm.runInContext(
    `var log = [];
     var conv = { replyToId: "", replyToBody: "", replyToName: "", editToId: "", editToBody: "", staged: [], _draftStash: null,
                  primary: true, visible: true, _roomIdSeen: "!r", stopTyping: function () {},
                  room: { roomId: "!r", editMessageMentions: function (id, t) { log.push("edit:" + id + ":" + t) }, send: function (t) { log.push("send:" + t) },
                          sendReply: function () { log.push("reply") }, sendEmote: function () { log.push("emote") }, sendWithMentions: function (t) { log.push("send:" + t) },
                          sendReplyMentions: function () { log.push("reply") }, sendAttachment: function (u) { log.push("file:" + u) } },
                  app: { notify: function (t) { log.push("notify:" + t) }, playUiSound: function () {} } };
     var composer = { text: "", _mentions: [], cursorPosition: 0, forceActiveFocus: function () {} };
     var msgView = { kbHold: false, stickBottom: false, positionViewAtEnd: function () {} };
     var mentionPopup = { close: function () {} }, emojiPopup = { close: function () {} };
     var Qt = { callLater: function () {} };
     var slashConfirm = { cmd: "", asked: null, opened: 0, open: function () { this.opened++ } };
     ${pure.replace(/\n {4}function (\w+)\(/g, "\nconv.$1 = function (")}
     ${members(["_draftState", "_draftApply", "_draft", "_notify", "_opFailed", "_expandSlash", "_unknownSlash", "_sendSnap", "startEdit", "sendComposer"])}
     conv.confirmSlash = function () ${confirm.startsWith("{") ? confirm : "{ " + confirm + " }"};`,
    ctx,
  );
  const { conv, composer, log, slashConfirm } = ctx;
  // the draft, an edit saved meanwhile (the draft comes back), then the draft asks the question
  composer.text = "/unknown private draft";
  conv.startEdit("$m", "old words");
  composer.text = "fixed typo";
  conv.sendComposer();
  assert.deepEqual([...log], ["edit:$m:fixed typo"]);
  assert.equal(composer.text, "/unknown private draft");
  conv.sendComposer();
  assert.equal(slashConfirm.opened, 1, "the unknown command asks first");
  // the edit is refused while the question is open: it comes back into the box (A20.0)
  conv._opFailed({ seq: 1, room: "!r", kind: "edit", id: "$m", text: "fixed typo", cls: "network" });
  assert.equal(conv.editToId, "$m");
  conv.confirmSlash();
  const after = [...log].slice(1);
  assert.ok(!after.some((l) => l.startsWith("edit:")), `Send as text saved the put-back edit: ${after}`);
  assert.ok(!after.some((l) => l.startsWith("send:")), `Send as text sent something it didn't show: ${after}`);
  assert.ok(after.includes("notify:The message box changed, so nothing was sent. Check it and send again."), `${after}`);
  // unchanged: it does send exactly what it showed
  conv.cancelTarget = undefined;
  composer.text = "/unknown other";
  conv._draftApply({ text: "/unknown other", marks: [], target: "", id: "", body: "", stash: null });
  conv.sendComposer();
  conv.confirmSlash();
  assert.equal([...log].pop(), "send:/unknown other");
});
