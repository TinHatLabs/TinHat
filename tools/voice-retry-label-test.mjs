// voice-retry-label-test.mjs — L1 A00.3: after a failed voice send the desktop's Send button says "Retry" (label, screen
// reader name and tooltip), as Tin Hat Web's VoiceRecorder.tsx SendButton does; it used to stay a bare icon named "Send".
//   node --test tools/voice-retry-label-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const qml = readFileSync(fileURLToPath(new URL("../crates/app/qml/VoiceRecorderRow.qml", import.meta.url)), "utf8");

test("Send turns into Retry after a failure", () => {
  const btn = qml.match(/GlowButton \{\n\s+id: sendBtn\n([\s\S]*?)\n\s+onClicked:/)?.[1];
  assert.ok(btn, "no voice Send button");
  const prop = (name) => btn.match(new RegExp(`\\n\\s+(?:readonly property bool )?${name.replace(".", "\\.")}: ([^\\n]+)`))?.[1];
  const ev = (expr, error, width) => {
    const ctx = vm.createContext({ row: { vs: { error }, conv: { width } } });
    ctx.again = vm.runInContext(prop("again") ?? "false", ctx);
    return vm.runInContext(expr, ctx);
  };
  assert.ok(prop("again"), "the button doesn't know about a failed send");
  assert.equal(ev(prop("text"), "Couldn't send", 800), "Retry");
  assert.equal(ev(prop("text"), "", 800), "Send");
  assert.equal(ev(prop("text"), "Couldn't send", 300), "", "narrow: icon only, like the text Send");
  assert.equal(ev(prop("Accessible.name"), "Couldn't send", 800), "Send voice message again");
  assert.equal(ev(prop("Accessible.name"), "", 800), "Send voice message");
  assert.equal(ev(prop("Controls.ToolTip.text"), "x", 800), "Try sending again (Ctrl+Enter)");
});
