// seeds-test.mjs — selfhost/firstrun §6 (one colour per person): runs the pure functions of crates/app/qml/Seeds.qml
// (between its BEGIN pure / END pure markers, the exact code the app runs) with the cases of the web's
// src/ui/avatarSeed.test.ts, so the desktop and the web give every person and room the same colour.
//   node tools/seeds-test.mjs            (scripts/run-gates.sh runs it with the unit tests)
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const qml = readFileSync(fileURLToPath(new URL("../crates/app/qml/Seeds.qml", import.meta.url)), "utf8");
// the lines after the BEGIN marker's line, up to the END marker
const body = qml.split("// BEGIN pure")[1]?.split("// END pure")[0]?.replace(/^[^\n]*\n/, "");
assert.ok(body, "Seeds.qml lost its BEGIN pure / END pure markers");
// The QML object's methods as plain functions; `seeds.x` (the object's id) resolves to the same functions.
const ctx = vm.createContext({});
vm.runInContext(`${body}\nvar seeds = { personSeed: personSeed, avatarSeed: avatarSeed };`, ctx);
const { personSeed, avatarSeed } = ctx.seeds;

// Avatar.qml's baseHue hash (the web's hash31): the same seed is the same colour.
const hash31 = (s) => { let h = 0; for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0; return h; };

test("personSeed: a LiveKit identity is the person", () => {
  const cases = [
    ["@alex:maplehouse.tinhat.me:QWERTYUIOP", "@alex:maplehouse.tinhat.me"],
    ["@tester1:example.com:SGQMTQBAXL", "@tester1:example.com"],
    ["@alex:maplehouse.tinhat.me", "@alex:maplehouse.tinhat.me"],
    ["@alex:host:8448", "@alex:host:8448"],
    ["@alex:host:8448:ABCDEFGHIJ", "@alex:host:8448"],
    ["@alex:[::1]", "@alex:[::1]"],
    ["@alex:[::1]:8448", "@alex:[::1]:8448"],
    ["@alex:[::1]:8448:DEV", "@alex:[::1]:8448"],
    ["  @alex:x:DEV ", "@alex:x"],
    ["!room:server", "!room:server"],
    ["", ""],
  ];
  for (const [i, want] of cases) assert.equal(personSeed(i), want, i);
});

test("avatarSeed: a DM is its person, a room its id", () => {
  assert.equal(avatarSeed("!dm:server", "@morgan:server"), "@morgan:server");
  assert.equal(avatarSeed("!dm:server", "@morgan:server:DEVICEX"), "@morgan:server");
  assert.equal(avatarSeed("!room:server", ""), "!room:server");
  assert.equal(avatarSeed("!room:server", null), "!room:server");
  assert.equal(avatarSeed("!room:server"), "!room:server");
  assert.equal(avatarSeed(null), "");
  assert.equal(avatarSeed("@sam:server:ABCDEFGHIJ"), "@sam:server");
});

test("the same person is the same colour in chat, on the call tile, in the DM list and header", () => {
  const me = "@morgan:maplehouse.tinhat.me";
  const chat = avatarSeed(me); // ChatBubble: the sender
  const tile = avatarSeed(`${me}:LKDEVICEID`); // call tile: the LiveKit identity
  const dmRow = avatarSeed("!dm:maplehouse.tinhat.me", me); // room list / DM strip
  const dmHeader = avatarSeed("!dm:maplehouse.tinhat.me", me); // the DM's header
  assert.equal(new Set([chat, tile, dmRow, dmHeader].map(hash31)).size, 1);
  assert.equal(hash31(avatarSeed("!general:maplehouse.tinhat.me")), hash31(avatarSeed("!general:maplehouse.tinhat.me", "")));
});
