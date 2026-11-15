// reset-ask-test.mjs — L1 A17.1: "Reset layout on this device" with an edit session that has unsaved changes asks first
// (they used to be thrown away without a word); with none it resets at once, as before. Tin Hat Web:
// src/features/editmode/resetAsk.test.ts (the same words, editmode/store.ts RESET_ASK).
//   node --test tools/reset-ask-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const main = readFileSync(fileURLToPath(new URL("../crates/app/qml/Main.qml", import.meta.url)), "utf8");

test("unsaved changes: ask; none: reset at once", () => {
  const fn = main.match(/\n {4}function resetLayoutHere\(\) \{[\s\S]*?\n {4}\}\n/)?.[0];
  assert.ok(fn);
  const run = (open, dirty) => {
    const ctx = vm.createContext({});
    vm.runInContext(`var log = []; var editModeObj = { open: ${open}, dirty: ${dirty}, cancel: function () { log.push("cancel") } };
      var resetAsk = { open: function () { log.push("ask") } };
      var layoutStoreObj = { setOverride: function () { log.push("override") } };
      var root = { settingsToast: function () {}, resetLayoutNow: function () { if (editModeObj.open) editModeObj.cancel(); layoutStoreObj.setOverride() } };
      ${fn} resetLayoutHere();`, ctx);
    return [...ctx.log];
  };
  assert.deepEqual(run(true, true), ["ask"], "unsaved layout changes must not go without a word");
  assert.deepEqual(run(true, false), ["cancel", "override"]);
  assert.deepEqual(run(false, false), ["override"]);
});

test("the question: the web's words; Reset resets", () => {
  const ask = main.match(/ConfirmDialog \{\n\s+id: resetAsk\n([\s\S]*?)\n {4}\}/)?.[1] ?? "";
  assert.match(ask, /dialogTitle: "Reset the layout on this device\?"/);
  assert.match(ask, /message: "Your unsaved layout changes will be lost too\."/);
  assert.match(ask, /confirmText: "Reset"/);
  assert.match(ask, /cancelText: "Keep editing"/);
  assert.match(ask, /onConfirmed: root\.resetLayoutNow\(\)/);
});
