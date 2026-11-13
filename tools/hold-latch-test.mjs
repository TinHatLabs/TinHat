// hold-latch-test.mjs — a hold button always sends its release (security review round 4, UI-1 round 2): runs the pure
// holdStep of crates/app/qml/ActionButton.qml (between its BEGIN hold-latch pure / END hold-latch pure markers, the code
// the app runs) on the shared vectors testvectors/hold-latch/vectors.json (Tin Hat Web replays the same file:
// src/actions/ui/holdLatch.test.ts), and checks that ActionButton, ActionsHost and Main wire it.
//   node --test tools/hold-latch-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const button = file("../crates/app/qml/ActionButton.qml");
const host = file("../crates/app/qml/ActionsHost.qml");
const main = file("../crates/app/qml/Main.qml");
const V = JSON.parse(file("../testvectors/hold-latch/vectors.json"));

test("holdStep: the shared vectors", () => {
  const body = button.split("// BEGIN hold-latch pure")[1]?.split("// END hold-latch pure")[0];
  assert.ok(body, "ActionButton.qml has no BEGIN hold-latch pure / END hold-latch pure block");
  const ctx = vm.createContext({});
  vm.runInContext(`${body}\nvar hs = holdStep;`, ctx);
  assert.ok(V.cases.length >= 12);
  for (const c of V.cases) {
    let st = { held: false, disposed: false };
    const out = [];
    for (const op of c.ops) {
      const r = ctx.hs(st, op);
      st = r.st;
      if (r.emit) out.push(r.emit);
    }
    assert.deepEqual(out, c.out, c.name);
  }
});

test("ActionButton: the press and every way a hold can end go through the latch", () => {
  assert.match(button, /onPressedChanged: if \(pressed\) \{ if \(ab\.holdable\) ab\._hold\("down", this\) \} else ab\._hold\("up"\)/);
  assert.match(button, /onCanceled: ab\._hold\("cancel"\)/);
  assert.match(button, /onHoldableChanged: if \(!holdable\) ab\._hold\("disable"\)/);
  assert.match(button, /onVisibleChanged: if \(!visible\) ab\._hold\("hide"\)/);
  assert.match(button, /onParentChanged: ab\._hold\("reparent"\)/);
  assert.match(button, /Component\.onDestruction: ab\._hold\("unmount"\)/);
  // a lifecycle end also goes straight to the run path (the host's handler may be gone with the button)
  assert.match(button, /if \(op !== "up" && op !== "cancel" && id\.length > 0 && ab\.app && ab\.app\.actions\)\n\s+ab\.app\.actions\.run\(id, \{ source: "button", anchor: null, hold: "up" \}\)/);
  // the old release, gated on the state at release time, is gone
  assert.doesNotMatch(button, /ab\.hold\(this, pressed\)/);
});

test("ActionsHost: a hold's release always runs; its list = actions.json's hold actions", () => {
  assert.match(host, /if \(inv\.hold === "up" && host\.holdActions\.indexOf\(id\) >= 0\)\n\s+return host\._exec\(/);
  const listed = JSON.parse(host.match(/readonly property var holdActions: (\[[^\]]*\])/)?.[1] ?? "null");
  const table = JSON.parse(file("../crates/actions/actions.json"));
  const all = (table.actions ?? table).filter((a) => a && a.hold === true).map((a) => a.id).sort();
  assert.ok(all.length > 0, "actions.json lists a hold action");
  assert.deepEqual([...(listed ?? [])].sort(), all);
});

test("Main: the push-to-talk key's hold ends when the window loses focus", () => {
  assert.match(main, /onActiveChanged: if \(!active && callctl && callctl\.pttSpeaking\) callctl\.pttHold\(false\)/);
});

test("ActionButton._hold (the app's own code, a fake button): every end releases once; a lifecycle end also runs the release", () => {
  const pure = button.split("// BEGIN hold-latch pure")[1].split("// END hold-latch pure")[0];
  const fn = button.match(/ {4}function _hold\(op, anchor\) \{[\s\S]*?\n {4}\}\n/)?.[0];
  assert.ok(fn, "ActionButton.qml lost _hold");
  const mk = () => {
    const ctx = vm.createContext({ console });
    vm.runInContext(`${pure}\nvar log = []; var ab = { holdStep: holdStep, _latch: { held: false, disposed: false }, _holdAnchor: null, _holdAction: "",
      item: { a: "call.pushToTalk.hold" }, hold: function (a, d) { log.push(d ? "signal:down" : "signal:up") },
      app: { actions: { run: function (id, inv) { log.push("run:" + id + ":" + inv.hold) } } } };
      ${fn.replace("function _hold", "ab._hold = function")}`, ctx);
    return ctx;
  };
  for (const [op, direct] of [["up", false], ["cancel", false], ["disable", true], ["hide", true], ["reparent", true], ["unmount", true]]) {
    const c = mk();
    c.ab._hold("down", {});
    c.ab._hold(op);
    c.ab._hold(op);
    c.ab._hold("up");
    const want = ["signal:down", "signal:up", ...(direct ? ["run:call.pushToTalk.hold:up"] : [])];
    assert.deepEqual([...c.log], want, op);
  }
  // a host handler that throws (its delegate is being destroyed) can't stop the release
  const c = mk();
  c.ab._hold("down", {});
  c.ab.hold = function (a, d) { if (!d) throw new Error("delegate gone"); };
  c.ab._hold("unmount");
  assert.deepEqual([...c.log], ["signal:down", "run:call.pushToTalk.hold:up"]);
  c.ab._hold("down", {});
  assert.equal(c.log.length, 2, "no press after unmount");
});

test("RowModel: a live hold keeps its delegates when only the snapshot changed; a shape or hold-item change publishes", () => {
  const src = file("../crates/app/qml/RowModel.qml");
  const parse = src.match(/ {4}function _parse\(\) \{[\s\S]*?\n {4}\}\n/)?.[0];
  const key = src.match(/ {4}function _genericsKey\(gens\) \{[\s\S]*?\n {4}\}\n/)?.[0];
  assert.ok(parse && key, "RowModel lost _parse / _genericsKey");
  const ctx = vm.createContext({});
  vm.runInContext(`var rm = { resultJson: "", firstColumn: 0, foldCount: 0, keep: [], bespoke: [], _holding: false, _gensKey: "", generics: [],
    app: { foldKeeps: function () { return true } }, _rowKindDefaults: function () { return { look: "icon", scale: "S" } } };
    ${key.replace("function _genericsKey", "rm._genericsKey = function")}
    ${parse.replace("function _parse", "rm._parse = function")}`, ctx);
  const row = (mic, extra) => JSON.stringify({ look: "icon", scale: "S", items: [{ a: "call.mic.toggle", state: "shown", known: true, target: null, active: mic }, { a: "call.pushToTalk.hold", state: "shown", known: true, target: null, hold: true }, ...(extra ?? [])] });
  const set = (j) => { ctx.resultJson = j; ctx.rm._parse(); }; // (_parse reads the bare QML property)
  set(row(false));
  const first = ctx.rm.generics;
  assert.equal(first.length, 2);
  ctx.rm._holding = true;
  set(row(true));
  assert.equal(ctx.rm.generics, first, "the press flipped the mic: same model, the held button lives");
  assert.equal(ctx.rm._holding, true);
  set(row(true, [{ a: "call.camera.toggle", state: "shown", known: true, target: null }]));
  assert.notEqual(ctx.rm.generics, first, "the row's shape changed: published (the button goes, its release runs)");
  assert.equal(ctx.rm._holding, false);
  const second = ctx.rm.generics;
  ctx.rm._holding = true;
  set(row(true, [{ a: "call.camera.toggle", state: "shown", known: true, target: null }]).replace('"hold":true', '"hold":true,"tip":"changed"'));
  assert.notEqual(ctx.rm.generics, second, "the held item itself changed: published");
  ctx.rm._holding = false;
  const third = ctx.rm.generics;
  set(row(false, [{ a: "call.camera.toggle", state: "shown", known: true, target: null }]));
  assert.notEqual(ctx.rm.generics, third, "no hold: every change publishes as before");
});

test("RowModel.run + ButtonRow keep the hold flag; Main ends a key hold when a transient window takes the keyboard", () => {
  const row = file("../crates/app/qml/RowModel.qml"), br = file("../crates/app/qml/ButtonRow.qml");
  assert.match(row, /if \(inv\.hold === "down"\) rm\._holding = true\n\s+app\.actions\.run\(it\.a, inv\)\n\s+if \(inv\.hold === "up"\) \{ rm\._holding = false; rm\._parse\(\) \}/);
  assert.match(br, /if \(br\._holding && k === br\._itemsKey\) return/);
  assert.match(br, /if \(down\) br\._holding = true[\s\S]{0,260}if \(!down\) \{ br\._holding = false; br\._parse\(\) \}/);
  assert.match(main, /onKeysHereChanged: if \(!keysHere && callctl && callctl\.pttSpeaking\) callctl\.pttHold\(false\)/);
});

test("the lab paddrive steps are fenced: refused without labDriveHooks; the fence names the switch", () => {
  assert.match(main, /\(s\.do === "row" \|\| s\.do === "find-action" \|\| s\.do === "ptt-state"\) && !login\.labDriveHooks\(\)\) \{ r = "refused" \}/);
  const at = main.indexOf('!login.labDriveHooks()');
  assert.ok(at > 0 && at < main.indexOf('s.do === "row") { r = root.layoutStore'), "the refusal comes before the steps");
  const bridge = file("../crates/app/src/bridge.rs");
  assert.match(bridge, /#\[cfg\(tinhat_env_selftests\)\]\s+\{\s+std::env::var_os\("TINHAT_LABDRIVE_PTT"\)\.is_some\(\)/);
  assert.match(bridge, /#\[cfg\(not\(tinhat_env_selftests\)\)\]\s+\{\s+false/);
  assert.match(file("../scripts/release-binary-check.sh"), /for h in [^;]*TINHAT_LABDRIVE_PTT[^;]*; do/);
});
