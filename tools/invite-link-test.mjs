// invite-link-test.mjs — selfhost/firstrun §4 (the invite link): runs the pure functions of crates/app/qml/InviteLink.qml
// (between its BEGIN pure / END pure markers, the exact code the app runs) with the cases of the web's
// src/app/invite.test.ts, plus what pasting a whole link into the sign-in screen does.
//   node --test tools/invite-link-test.mjs     (scripts/run-gates.sh runs it with the unit tests)
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const qml = readFileSync(fileURLToPath(new URL("../crates/app/qml/InviteLink.qml", import.meta.url)), "utf8");
const body = qml.split("// BEGIN pure")[1]?.split("// END pure")[0]?.replace(/^[^\n]*\n/, "");
assert.ok(body, "InviteLink.qml lost its BEGIN pure / END pure markers");
const ctx = vm.createContext({});
vm.runInContext(`${body}
var links = { _queryValue: _queryValue, parseHash: parseHash, inviteFieldValue: inviteFieldValue, codeFromPaste: codeFromPaste };`, ctx);
const { parseHash, inviteFieldValue, codeFromPaste } = ctx.links;
const plain = (o) => JSON.parse(JSON.stringify(o)); // objects from the vm context compare by value

const field = (hash) => {
  const r = parseHash(hash);
  return r.name === "register" ? inviteFieldValue(r.token) : null;
};

test("the dashboard's link and its variants fill the code", () => {
  assert.deepEqual(plain(parseHash("#/register?token=TH-AB12-CD34")), { name: "register", token: "TH-AB12-CD34" });
  const cases = [
    ["#/register?token=TH-AB12-CD34", "AB12-CD34"],
    ["#/register?token=th-ab12-cd34", "ab12-cd34"],
    ["#/register?invite=TH-AB12-CD34", "AB12-CD34"],
    ["#register?token=TH-AB12-CD34", "AB12-CD34"],
    ["/register?token=TH-AB12-CD34", "AB12-CD34"],
    ["#/register?token=TH%2DAB12%2DCD34", "AB12-CD34"],
    ["#/register?token=%20TH-AB12-CD34%20", "AB12-CD34"],
    ["#/register?token=AB12CD34", "AB12CD34"],
    ["#/register?token=TH-AB12-CD34&invite=TH-ZZZZ-ZZZZ", "AB12-CD34"],
    ["#/register?via=x&token=TH-AB12-CD34", "AB12-CD34"],
    ["#/register", ""],
    ["#/register?token=", ""],
  ];
  for (const [hash, want] of cases) assert.equal(field(hash), want, hash);
});

test("other routes carry no invite", () => {
  for (const h of ["#/login", "#/home", "", "#/forgot", "#/room/!r:x?token=TH-AB12-CD34"]) assert.equal(field(h), null, h);
});

test("inviteFieldValue", () => {
  assert.equal(inviteFieldValue(undefined), "");
  assert.equal(inviteFieldValue(null), "");
  assert.equal(inviteFieldValue("  TH-AB12-CD34 "), "AB12-CD34");
  assert.equal(inviteFieldValue("th-x"), "x");
  assert.equal(inviteFieldValue("THAB12CD34"), "THAB12CD34", "no hyphen: left to the field's own rule");
});

test("pasting a whole link into the sign-in screen (desktop)", () => {
  const cases = [
    ["https://maplehouse.tinhat.me/#/register?token=TH-AB12-CD34", "AB12-CD34"],
    ["  https://maplehouse.tinhat.me/#/register?token=TH-AB12-CD34\n", "AB12-CD34"],
    ["http://maplehouse.local/#register?invite=TH-AB12-CD34", "AB12-CD34"],
    ["https://chat.example.com:8443/#/register?token=TH%2DAB12%2DCD34", "AB12-CD34"],
    ["#/register?token=TH-AB12-CD34", "AB12-CD34"],
    ["https://maplehouse.tinhat.me/#/register", ""],
  ];
  for (const [text, want] of cases) assert.equal(codeFromPaste(text), want, text);
  // not a link: the field keeps what was typed
  for (const text of ["AB12-CD34", "TH-AB12-CD34", "maplehouse.tinhat.me", "https://maplehouse.tinhat.me/",
                      "https://maplehouse.tinhat.me/#/login", "https://a b/#/register?token=X", "", null, undefined])
    assert.equal(codeFromPaste(text), null, String(text));
});
