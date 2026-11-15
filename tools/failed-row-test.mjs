// failed-row-test.mjs — L1 A31.0 / D05.6 / A38.0: a failed message can be retried or deleted without a mouse. Its Retry /
// Delete are named buttons for a screen reader (Accessible.Button + a name + the press action), and the message menu
// (the keyboard's way in: the list is one Tab stop) offers Retry and Delete for a failed send, as the web's menu does.
// Tin Hat Web: src/features/chat/failedRowButtons.test.ts.
//   node --test tools/failed-row-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const bubble = file("../crates/app/qml/ChatBubble.qml");
const conv = file("../crates/app/qml/ConversationView.qml");

test("the failed row's Retry / Delete are named buttons a screen reader can press", () => {
  for (const [text, name, sig] of [["Retry", "Retry sending this message", "retryRequested"], ["Delete", "Delete this unsent message", "discardRequested"]]) {
    const lbl = bubble.match(new RegExp(`Controls\\.Label \\{\\n\\s+text: "${text}"\\n([\\s\\S]*?)\\n\\s+TapHandler \\{ onTapped: bubble\\.${sig}\\(bubble\\.txn\\) \\}`))?.[1];
    assert.ok(lbl, `no ${text} label`);
    assert.match(lbl, /Accessible\.role: Accessible\.Button/, `${text}: not a button to a screen reader`);
    assert.match(lbl, new RegExp(`Accessible\\.name: "${name}"`));
    assert.match(lbl, new RegExp(`Accessible\\.onPressAction: bubble\\.${sig}\\(bubble\\.txn\\)`));
  }
});

test("the message menu offers Retry and Delete for a failed send", () => {
  const items = [...conv.matchAll(/Controls\.MenuItem \{\n\s+text: "(Retry|Delete)"\n\s+icon\.name: "[^"]+"\n\s+visible: ([^\n]+)\n\s+height: [^\n]+\n\s+onTriggered: ([^\n]+)/g)];
  const retry = items.find((m) => m[1] === "Retry");
  const del = items.find((m) => m[1] === "Delete" && /discardEcho/.test(m[3]));
  assert.ok(retry, "the menu has no Retry");
  assert.ok(del, "the menu has no Delete for a failed send");
  assert.equal(retry[3], "conv.room.retrySend(conv.ctxMsgTxn)");
  assert.equal(del[3], "conv.room.discardEcho(conv.ctxMsgTxn)");
  const open = conv.match(/function _openMenuFor\(md, mine, at\) \{[\s\S]*?\n {4}\}\n/)[0];
  const ctx = vm.createContext({});
  const vis = (md) => {
    vm.runInContext(`var conv = { _canThread: () => false }; ${open.replace(/if \(conv\.ctxMsgId\.length === 0\) return[\s\S]*$/, "}")}; _openMenuFor(${JSON.stringify(md)}, true, null)`, ctx);
    return vm.runInContext(retry[2], ctx);
  };
  assert.equal(vis({ id: "~t1", txn: "t1", echo: true, failed: true, body: "x" }), true);
  assert.equal(vis({ id: "~t1", txn: "t1", echo: true, failed: false, body: "x" }), false, "not on a pending echo");
  assert.equal(vis({ id: "$e", body: "x" }), false, "not on a sent message");
});
