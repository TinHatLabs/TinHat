// login-view-test.mjs — the sign-in card's first-run and keyboard rules (QoL sweep 2026-11-13), checked in
// crates/app/qml/LoginView.qml's source (Tin Hat Web's src/app/loginview.test.ts checks its LoginView.tsx for the same
// rules and words):
//   A01.1 the quiet links (Use <server>, Forgot your password?, New here? Create an account, both Back to sign in) are
//         reachable with Tab, act on Return/Enter/Space and read as buttons; Escape leaves the create / reset face;
//   A01.2 an empty Server field doesn't grey out Create account (the click says why: SERVER_FIELD_HINT);
//   A01.3 an empty Server field takes the caret first and says what a server is;
//   A14.3 an invite link pasted into the Server or Invite code field fills the Server field with the link's server.
//   node --test tools/login-view-test.mjs     (scripts/run-gates.sh runs it with the unit tests)
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { test } from "node:test";
import vm from "node:vm";

const qml = readFileSync(fileURLToPath(new URL("../crates/app/qml/LoginView.qml", import.meta.url)), "utf8");
const EXPLAINER = "Your server is where your account lives. Its address is in your invite, like yourname.tinhat.me.";

/** The Controls.Label block whose `text:` line contains `needle` (from its opening line to its closing brace). */
function labelBlock(needle) {
  const lines = qml.split("\n");
  const at = lines.findIndex((l) => /^\s*text: /.test(l) && l.includes(needle));
  assert.ok(at >= 0, `no label with text ${needle}`);
  let start = at;
  while (start > 0 && !/Controls\.Label \{\s*$/.test(lines[start])) start--;
  const indent = lines[start].match(/^\s*/)[0];
  let end = start + 1;
  while (end < lines.length && lines[end] !== `${indent}}`) end++;
  return lines.slice(start, end + 1).join("\n");
}

test("A01.1: every quiet link is a keyboard- and screen-reader-reachable button", () => {
  const links = ['"Use " + loginView.movedTo', '"Forgot your password?"', '"New here? Create an account"'];
  const blocks = links.map(labelBlock);
  // both "Back to sign in" links (the reset's last step has a real button of that name: not a link)
  let from = 0;
  let backs = 0;
  for (;;) {
    const i = qml.indexOf('text: "Back to sign in"', from);
    if (i < 0) break;
    from = i + 1;
    const before = qml.lastIndexOf("Controls.Label {", i);
    if (qml.lastIndexOf("GlowButton {", i) > before) continue;
    backs++;
    blocks.push(qml.slice(before, qml.indexOf("MouseArea {", i)));
  }
  assert.equal(backs, 2);
  for (const b of blocks) {
    assert.match(b, /activeFocusOnTab: \w+\.enabled/, b);
    assert.match(b, /Keys\.onReturnPressed: if \((\w+)\.enabled\) \1\.clicked\(null\)/, "a busy card ignores the key");
    assert.match(b, /Keys\.onEnterPressed: /, b);
    assert.match(b, /Keys\.onSpacePressed: /, b);
    assert.match(b, /Accessible\.role: Accessible\.Button/, b);
    assert.match(b, /Accessible\.name: text/, b);
    assert.match(b, /Accessible\.onPressAction: if \((\w+)\.enabled\) \1\.clicked\(null\)/, b);
    assert.match(b, /activeFocus\) \? Th\.loginLinkFgHover/, "the focused link shows like the hovered one");
  }
});

test("A01.1: Escape leaves the create and reset faces", () => {
  const esc = qml.split("Keys.onEscapePressed: (event) => {")[1]?.split("\n    }\n")[0] ?? "";
  assert.match(esc, /showCreate\) loginView\.leaveCreate\(\)/);
  assert.match(esc, /showReset\) loginView\.leaveReset\(\)/);
  assert.match(esc, /event\.accepted = false/, "on the sign-in face Escape is left to others");
});

test("A01.2: an empty Server field doesn't grey out Create account", () => {
  const ready = qml.split("readonly property bool createReady:")[1]?.split("\n    function")[0] ?? "";
  assert.ok(ready.length > 0);
  assert.ok(!/homeserver/.test(ready), `createReady still needs the server: ${ready}`);
});

test("A01.3: an empty Server field takes the caret first and says what goes there", () => {
  const ff = qml.split("function focusFirst() {")[1]?.split("\n    }\n")[0] ?? "";
  const code = ff.split("\n").map((l) => l.trim()).filter((l) => l && !l.startsWith("//"));
  assert.match(code[0], /showReset\) return/);
  assert.match(code[1], /homeserverField\.text\.trim\(\)\.length === 0\) homeserverField\.focusField\(\)/, code[1]);
  assert.ok(qml.includes(`text: "${EXPLAINER}"`), "the explainer line is missing");
  assert.match(labelBlock(EXPLAINER), /visible: homeserverField\.text\.trim\(\)\.length === 0/);
});

test("A14.3: a pasted invite link fills the Server field with its server", () => {
  assert.equal(qml.split("InviteLink.inviteFromPaste(").length - 1, 2, "both the Server and the Invite code field");
  assert.ok(!qml.includes("InviteLink.codeFromPaste("), "a code is never taken without its server");
  // the Server field keeps following the controller's server after a paste (its binding is put back)
  assert.equal(qml.split("loginView.homeserverEdited(host); homeserverField.text = Qt.binding(").length - 1, 2);
});

// D01.7: the live hint, run on LoginView.qml's own signupHint (between its BEGIN pure / END pure markers) with
// testvectors/signup-hint/vectors.json, byte-identical in Tin Hat Web (src/app/createForm.test.ts).
test("D01.7: the create form's hint names the wrong field (shared vectors)", () => {
  const body = qml.split("// BEGIN pure")[1]?.split("// END pure")[0]?.replace(/^[^\n]*\n/, "");
  assert.ok(body, "LoginView.qml lost its BEGIN pure / END pure markers");
  const ctx = vm.createContext({});
  vm.runInContext(`${body}\nvar fns = { signupHint: signupHint };`, ctx);
  const V = JSON.parse(readFileSync(fileURLToPath(new URL("../testvectors/signup-hint/vectors.json", import.meta.url)), "utf8"));
  assert.ok(V.cases.length >= 12);
  for (const c of V.cases) {
    const got = JSON.parse(JSON.stringify(ctx.fns.signupHint(c.username, c.password, c.password2)));
    assert.deepEqual(got, { text: c.text, error: c.error }, c.name);
  }
  assert.match(qml, /loginView\.signupHint\(newUserField\.text, newPwField\.text, newPw2Field\.text\)/);
});

test("D01.7: Create account counts password characters as the hint does", () => {
  const ready = qml.split("readonly property bool createReady:")[1]?.split("\n    function")[0] ?? "";
  assert.match(ready, /Array\.from\(newPwField\.text\)\.length >= 8/);
});
