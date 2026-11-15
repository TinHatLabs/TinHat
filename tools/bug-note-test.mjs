// bug-note-test.mjs — L1 A34.0: "Report a problem" keeps the typed note through Cancel, Escape and a click outside; only a
// SENT report clears it (every open used to clear it unless the last send had failed). Tin Hat Web:
// src/features/settings/bugNote.test.ts.
//   node --test tools/bug-note-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const qml = ["Main.qml", "ActionsHost.qml", "BugReportDialog.qml", "AppCore.qml"].map((f) => [f, file(`../crates/app/qml/${f}`)]);

test("no way into the dialog clears the note", () => {
  for (const [f, src] of qml)
    for (const line of src.split("\n"))
      if (/clearForm\(\)/.test(line) && /bugReport/i.test(line) && !/^\s*\/\//.test(line))
        assert.match(line, /Report sent/, `${f}: only a sent report may clear the note: ${line.trim()}`);
  // and only its own note (a newer note typed while it was sending stays)
  const main = qml.find(([f]) => f === "Main.qml")[1];
  assert.match(main, /if \(settings\.bugReportStatus\.indexOf\("Report sent"\) === 0\) bugReportDialog\.sendDone\(true\)/);
  assert.match(main, /else if \(settings\.bugReportStatus\.indexOf\("Report failed"\) === 0\) bugReportDialog\.sendDone\(false\)/);
  const dlg = qml.find(([f]) => f === "BugReportDialog.qml")[1];
  assert.match(dlg, /onAccepted: if \(noteArea\.text\.trim\(\)\.length > 0\) \{ dlg\.sentText = noteArea\.text; dlg\.pending \+= 1; dlg\.submitted\(noteArea\.text\) \}/);
  // the app's own sendDone on a fake dialog: send A, reopen, send B, A's answer must not clear B
  const fn = dlg.match(/ {4}function sendDone\(sent\) \{[\s\S]*?\n {4}\}\n/)[0];
  const run = (steps) => {
    const d = { pending: 0, sentText: "" }, noteArea = { text: "" };
    const sendDone = new Function("dlg", "noteArea", `${fn}; return sendDone;`)(d, noteArea);
    for (const s of steps) {
      if (s.type !== undefined) noteArea.text = s.type;
      if (s.send) { d.sentText = noteArea.text; d.pending += 1; }
      if (s.done !== undefined) sendDone(s.done);
    }
    return noteArea.text;
  };
  assert.equal(run([{ type: "A" }, { send: true }, { done: true }]), "", "a sent note goes");
  assert.equal(run([{ type: "A" }, { send: true }, { type: "B" }, { done: true }]), "B", "a newer note stays");
  assert.equal(run([{ type: "A" }, { send: true }, { type: "B" }, { send: true }, { done: true }]), "B", "A's answer doesn't clear B");
  assert.equal(run([{ type: "A" }, { send: true }, { type: "B" }, { send: true }, { done: true }, { done: true }]), "", "B's answer does");
  assert.equal(run([{ type: "A" }, { send: true }, { done: false }]), "A", "a failed send keeps it");
});

test("closing the dialog doesn't clear it either", () => {
  const dlg = qml.find(([f]) => f === "BugReportDialog.qml")[1];
  assert.doesNotMatch(dlg, /on(Closed|Rejected)\s*:[^\n]*clearForm/);
  assert.doesNotMatch(dlg, /on(Closed|Rejected)\s*:[^\n]*noteArea\.text\s*=/);
  const main = qml.find(([f]) => f === "Main.qml")[1];
  assert.match(main, /if \(settings\.bugReportStatus\.indexOf\("Report sent"\) === 0\) bugReportDialog\.sendDone\(true\)/);
});
