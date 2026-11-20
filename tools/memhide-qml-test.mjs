// memhide-qml-test.mjs — 12.1.17 round 2 (memories-hide-chat, assembly-1217 review #2/#3): the QML wiring of hidden
// tails. A thread's panel always reads its thread; readTail only flags and the poll marks read after it applied the rows.
//   node --test tools/memhide-qml-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { test } from "node:test";

const cv = readFileSync(fileURLToPath(new URL("../crates/app/qml/ConversationView.qml", import.meta.url)), "utf8");

test("a thread's panel is always a reader of its thread (the one-reader rule is the main timeline's)", () => {
  assert.match(cv, /readonly property bool reader: conv\.primary \|\| conv\.threadRoot\.length > 0 \|\| !\(conv\.app && conv\.app\.conversationView/);
});

test("readTail only flags; the poll marks read after applying the rows, settled, at the view's end", () => {
  assert.match(cv, /function onReadTailChanged\(\) \{ if \(conv\.room && String\(conv\.room\.readTail \|\| ""\)\.length > 0\) conv\._readTailPending = true \}/);
  assert.match(cv, /if \(conv\._readTailPending && conv\._markerFrozen && !msgView\.settling\) \{\n\s+conv\._readTailPending = false\n\s+if \(conv\.reader && conv\.attending && msgView\.stickBottom\) conv\.room\.markRead\(\)/);
});

test("a jump to a hidden photo opens Memories through one helper line (the hunt stays in _jumpTo's reach)", () => {
  assert.match(cv, /if \(conv\._jumpToMemories\(eventId\)\) return/);
});
