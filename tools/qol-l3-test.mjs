// qol-l3-test.mjs — QOL lane L3 (2026-11-14), the desktop's small room-list / menu items, each mirrored on the web:
//   A04.3  the room menu's call item: Main.qml roomMenuCall (between BEGIN / END room-menu-call, the code the app runs)
//          on the web's roomMenuCall.test.ts cases, and the item runs call.join on that room
//   A04.1  the room menu by membership: joined-only items hide on an invite / a listed room you are not in, Leave reads
//          "Decline invite" / hides, the Notifications submenu is taken out (the web's menus.tsx)
//   A39.5  Copy text says "Text copied", as Copy link says "Message link copied" (the web's ConversationView.tsx)
//   A23.1 / A23.5  Ctrl+/ and Ctrl+F reach their actions (the table's keys; the engine side is unit.rs example 15)
//   node --test tools/qol-l3-test.mjs     (scripts/run-gates.sh runs it with the unit tests)
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const here = (p) => fileURLToPath(new URL(p, import.meta.url));
const main = readFileSync(here("../crates/app/qml/Main.qml"), "utf8");
const conv = readFileSync(here("../crates/app/qml/ConversationView.qml"), "utf8");
const menu = main.slice(main.indexOf("id: roomMenu\n"), main.indexOf("id: recordConfirm"));

test("A04.3: the call item — joined rooms only, never the call you are in (the web's cases)", () => {
  const body = main.split("// BEGIN room-menu-call")[1]?.split("// END room-menu-call")[0];
  assert.ok(body, "Main.qml lost its BEGIN / END room-menu-call markers");
  const ctx = vm.createContext({});
  vm.runInContext(`${body}\nvar out = { roomMenuCall: roomMenuCall };`, ctx);
  const CASES = [
    ["join", true, false, "Join the active call"],
    ["join", false, false, "Start a call"],
    ["join", true, true, ""],
    ["invite", true, false, ""],
    ["none", true, false, ""],
    ["", false, false, ""],
  ];
  for (const [m, live, inThis, want] of CASES) assert.equal(ctx.out.roomMenuCall(m, live, inThis), want, JSON.stringify([m, live, inThis]));
  assert.match(main, /readonly property string ctxRoomCallLabel: root\.roomMenuCall\(root\.ctxRoomMembership, root\.ctxRoomCall,\s*root\.mInCall === true && root\.mCallRoomId === root\.ctxRoomId\)/);
  assert.match(menu, /text: LinkGate\.plain\(root\.ctxRoomCallLabel\)[\s\S]{0,200}visible: root\.ctxRoomCallLabel\.length > 0[\s\S]{0,300}root\.actions\.run\("call\.join", \{ source: "host", target: \{ room: id \} \}\)/);
});

test("A04.1: the room menu follows the row's membership", () => {
  assert.match(main, /root\.ctxRoomMembership = String\(root\.mNavRooms\[i\]\.membership \|\| "join"\)\s*root\.ctxRoomCall = root\.mNavRooms\[i\]\.call === true\s*root\._syncRoomNotifyMenu\(\)\s*roomMenu\.popup\(\)/);
  for (const label of ['root.ctxRoomFav ? "Unfavourite" : "Favourite"', '"Invite people…"', '"Pop out chat"']) {
    const at = menu.indexOf(`text: ${label}`);
    assert.ok(at > 0, label);
    assert.match(menu.slice(at, at + 400), /visible: root\.ctxRoomMembership === "join"/, `${label} hides unless joined`);
  }
  assert.match(menu, /text: root\.ctxRoomMembership === "invite" \? "Decline invite" : "Leave room"[\s\S]{0,80}visible: root\.ctxRoomMembership !== "none"/);
  assert.match(main, /var want = root\.ctxRoomMembership === "join"[\s\S]{0,200}roomMenu\.insertMenu\(root\._roomNotifyIndex, roomNotifyMenu\)\s*else if \(!want && at >= 0\) roomMenu\.takeMenu\(at\)/);
});

test("A39.5: Copy text confirms with a toast", () => {
  assert.match(conv, /text: "Copy text"[\s\S]{0,200}onTriggered: \{ conv\.copyText\(conv\.ctxMsgBody\); conv\._toast\("Text copied", "short"\) \}/);
});

test("A23.1 / A23.5: Ctrl+/ opens the shortcuts, Ctrl+F the room filter — bound through the table's keys", () => {
  const host = readFileSync(here("../crates/app/qml/ActionsHost.qml"), "utf8");
  assert.match(main, /Shortcut \{ sequences: actionsHost\.keysFor\("app\.shortcuts\.open"\); context: Qt\.ApplicationShortcut; onActivated: shortcutsDialog\.open\(\) \}/);
  assert.match(main, /Shortcut \{ sequences: actionsHost\.keysFor\("nav\.roomList\.filter"\); enabled: root\.keysHere && root\.mLoggedIn === true; onActivated: actionsHost\.run\("nav\.roomList\.filter", \{ source: "key" \}\) \}/);
  assert.match(host, /"app\.zoom\.reset",\s*"app\.shortcuts\.open", "nav\.roomList\.filter"\]/, "both have their own Shortcut (no duplicate user one)");
  assert.match(host, /case "nav\.roomList\.filter":[^\n]*filterField\.forceActiveFocus\(\); ui\.roomsIsl\.filterField\.selectAll\(\)/);
  assert.match(main, /helpKey\("nav\.roomList\.filter", "Ctrl\+F"\)[^\n]*"Filter rooms"/);
  assert.match(main, /helpKey\("app\.shortcuts\.open", "Ctrl\+\/"\)[^\n]*"Keyboard shortcuts"/);
});
