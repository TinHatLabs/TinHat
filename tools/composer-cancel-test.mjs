// composer-cancel-test.mjs — L1 A09.0: the reply/edit bar's ✕ (tooltip "Cancel (Esc)") does what Escape does: it drops
// the reply/edit target and keeps the typed text and the staged files (it used to wipe both on one click). Runs the
// app's own cancel function from crates/app/qml/ConversationView.qml on a fake view. Tin Hat Web checks its Composer the
// same way (src/features/chat/composerCancel.test.ts).
//   node --test tools/composer-cancel-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const qml = readFileSync(fileURLToPath(new URL("../crates/app/qml/ConversationView.qml", import.meta.url)), "utf8");

test("the bar's ✕ calls Escape's cancel", () => {
  const btn = qml.match(/Controls\.ToolTip\.text: "Cancel \(Esc\)"[\s\S]*?onClicked: ([^\n]*)/);
  assert.ok(btn, "the reply/edit bar's ✕ is missing");
  assert.match(btn[1], /conv\.cancelTarget\(\)/, "the ✕ must call conv.cancelTarget (Escape's cancel)");
  assert.match(qml, /else if \(conv\.composing\) conv\.cancelTarget\(\)/, "Escape cancels through cancelTarget");
});

test("the ✕'s function keeps the typed text and the staged files", () => {
  const call = qml.match(/Controls\.ToolTip\.text: "Cancel \(Esc\)"[\s\S]*?onClicked: [^\n]*?conv\.(\w+)\(\)/)[1];
  const fn = qml.match(new RegExp(` {4}function ${call}\\(\\) \\{[\\s\\S]*?\\n {4}\\}\\n`))?.[0];
  assert.ok(fn, `ConversationView.qml lost ${call}`);
  // the view's draft helpers it may go through (L1 composer-draft: the pure block + _draftState/_draftApply/_draft), as conv.*
  const pure = qml.split("// BEGIN composer-draft pure")[1]?.split("// END composer-draft pure")[0] ?? "";
  const helpers = ["_draftState", "_draftApply", "_draft"]
    .map((n) => qml.match(new RegExp(` {4}function ${n}\\([^)]*\\) \\{[\\s\\S]*?\\n {4}\\}\\n| {4}function ${n}\\([^)]*\\) \\{[^\\n]*\\}\\n`))?.[0] ?? "")
    .join("\n");
  const asConv = (code) => code.replace(/\n {4}function (\w+)\(/g, "\nconv.$1 = function (").replace(/^ {4}function (\w+)\(/, "conv.$1 = function (");
  const run = (kind) => {
    const ctx = vm.createContext({});
    vm.runInContext(`var closed = 0;
      var conv = { replyToId: "", replyToBody: "", replyToName: "", editToId: "", editToBody: "", staged: [{ url: "file:///a.png" }],
                   stopTyping: function () {} };
      var composer = { text: "half a thought", _mentions: [{ name: "b" }] };
      var mentionPopup = { close: function () { closed++ } }, emojiPopup = { close: function () { closed++ } };
      if (${JSON.stringify(kind)} === "reply") { conv.replyToId = "$p"; conv.replyToBody = "parent"; conv.replyToName = "Bo" }
      else { conv.editToId = "$m"; conv.editToBody = "old"; composer.text = "old" }
      ${asConv(pure)}
      ${asConv(helpers)}
      ${fn.replace(`function ${call}`, "conv.fn = function")}
      conv.fn();`, ctx);
    return ctx;
  };
  const r = run("reply");
  assert.equal(r.composer.text, "half a thought", "a reply's typed text is kept");
  assert.equal(r.composer._mentions.length, 1, "its picked mentions too");
  assert.equal(r.conv.staged.length, 1, "the staged file is kept");
  assert.equal(r.conv.replyToId, "", "the reply target is dropped");
  const e = run("edit");
  assert.equal(e.conv.editToId, "", "the edit target is dropped");
  assert.equal(e.conv.staged.length, 1, "the staged file is kept when leaving an edit");
});
