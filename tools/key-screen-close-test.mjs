// key-screen-close-test.mjs — L1 A28.0: the one-time Security Key screen (VerifyDeviceDialog, sasState "recovery-done")
// can't be thrown away by one Escape. Until "I've saved this key" is ticked, neither Escape nor a click outside closes
// it; Escape asks "Close without saving your key?" (Tin Hat Web's words, src/features/selfhost/firstrun.ts KEY_SKIP_*).
// Closing it is not a cancel (it used to log "You cancelled" over a key that was made). Tin Hat Web checks its dialog
// the same way (src/features/settings/keyScreenClose.test.ts).
//   node --test tools/key-screen-close-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const qml = readFileSync(fileURLToPath(new URL("../crates/app/qml/VerifyDeviceDialog.qml", import.meta.url)), "utf8");

test("unsaved key: no auto-close; Escape asks first", () => {
  const cp = qml.match(/\n {4}closePolicy: ([^\n]*)/);
  assert.ok(cp, "the dialog sets no closePolicy (Escape / a click outside close the key screen)");
  const keyUnsaved = qml.match(/readonly property bool keyUnsaved: ([^\n]*)/)?.[1];
  assert.ok(keyUnsaved);
  const policy = (state, checked) => {
    const ctx = vm.createContext({ dlg: { sasState: state }, savedCheck: { checked }, Controls: { Popup: { NoAutoClose: 0, CloseOnEscape: 1, CloseOnPressOutside: 2 } } });
    ctx.dlg.keyUnsaved = vm.runInContext(keyUnsaved, ctx);
    return vm.runInContext(cp[1], ctx);
  };
  assert.equal(policy("recovery-done", false), 0, "the unsaved key screen must not close by itself");
  assert.equal(policy("recovery-done", true), 3);
  assert.equal(policy("emoji", false), 3);
  const sc = qml.match(/Shortcut \{\n\s+sequences: \[StandardKey\.Cancel\]\n\s+enabled: ([^\n]*)\n\s+onActivated: (\w+)\.open\(\)/);
  assert.ok(sc, "Escape on the unsaved key screen opens the question");
  assert.match(sc[1], /dlg\.keysHere && dlg\.keyUnsaved/);
  const ask = qml.match(new RegExp(`ConfirmDialog \\{\\n\\s+id: ${sc[2]}\\n([\\s\\S]*?)\\n {4}\\}`))?.[1] ?? "";
  assert.match(ask, /dialogTitle: "Close without saving your key\?"/);
  assert.match(ask, /message: "You won't see it again\. Without it, you can't read your messages if you sign in on a new device\."/);
  assert.match(ask, /confirmText: "Close anyway"/);
  assert.match(ask, /cancelText: "Go back"/);
  assert.match(ask, /onConfirmed: dlg\.reject\(\)/);
});

test("closing the key screen is not a cancel", () => {
  const m = qml.match(/\n {4}onRejected: (\{[\s\S]*?\n {4}\}|[^\n]*)/);
  assert.ok(m, "no onRejected");
  const run = (state) => {
    const log = [];
    const ctx = vm.createContext({ dlg: { sasState: state, sasCancelled: () => log.push("cancelled"), sasResetRequested: () => log.push("reset") } });
    vm.runInContext(m[1], ctx);
    return log;
  };
  assert.deepEqual(run("recovery-done"), ["reset"], "closing the key screen must not log a cancel");
  assert.deepEqual(run("emoji"), ["cancelled"], "closing a live emoji check still cancels it");
  assert.deepEqual(run("waiting"), ["cancelled"]);
  assert.deepEqual(run(""), []);
  assert.deepEqual(run("done"), []);
});
