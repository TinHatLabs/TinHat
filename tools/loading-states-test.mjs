// loading-states-test.mjs — the conversation's loading states (QOL A29.0 / A29.3; Tin Hat Web's
// src/features/chat/loadingStates.test.ts checks the same words): opening a room says "Loading messages…" (ThBusyIndicator's
// "Still …" after 10 s), the history mark has a name, and "Couldn't load this conversation" has a Try again button
// that reloads the room.
//   node --test tools/loading-states-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const cv = file("../crates/app/qml/ConversationView.qml");
const es = file("../crates/app/qml/EmptyState.qml");
const bi = file("../crates/app/qml/ThBusyIndicator.qml");

test("opening a room says what loads", () => {
  assert.match(cv, /loadingLabel: "Loading messages…"/);
  assert.match(es, /shownTitle: empty\.loading && empty\.loadingLabel\.length > 0 \? busyMark\.shownLabel : empty\.title/);
  assert.match(es, /id: busyMark[\s\S]{0,200}label: empty\.loadingLabel/);
  const fn = bi.split("function stillLabel(l) {")[1]?.split("\n    }")[0];
  const ctx = vm.createContext({});
  vm.runInContext(`function stillLabel(l) {${fn}\n}`, ctx);
  assert.equal(ctx.stillLabel("Loading messages…"), "Still loading messages…");
});

test("the history mark has a name", () => {
  assert.match(cv, /label: "Loading older messages"[\s\S]{0,80}running: conv\.mLoadingOlder/);
  assert.match(bi, /Accessible\.name: mark\.label\.length > 0 \? mark\.shownLabel : "Loading"/);
});

test("a failed load has Try again", () => {
  assert.match(cv, /actionText: !_loading && conv\.mStatus\.length > 0 \? "Try again" : ""/);
  assert.match(cv, /onActionClicked: \{ conv\.room\.refresh\(\) \}/);
  assert.match(es, /ThButton \{[\s\S]{0,200}text: empty\.actionText[\s\S]{0,60}onClicked: empty\.actionClicked\(\)/);
});
