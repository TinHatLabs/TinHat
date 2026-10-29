// link-gate-test.mjs — security review 2026-10-26 (H2): runs the pure functions of crates/app/qml/LinkGate.qml (between
// its BEGIN pure / END pure markers, the exact code the app runs) on the shared vectors
// crates/app/src/testvectors/member-content.json (section `links`, the same cases richtext.rs link_verdict and Tin Hat
// Web replay), and checks that the attachment extensions it opens are viewer.rs's OPEN_EXTS.
//   node --test tools/link-gate-test.mjs     (scripts/run-gates.sh runs it with the unit tests)
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const qml = file("../crates/app/qml/LinkGate.qml");
const body = qml.split("// BEGIN pure")[1]?.split("// END pure")[0]?.replace(/^[^\n]*\n/, "");
assert.ok(body, "LinkGate.qml lost its BEGIN pure / END pure markers");
const ctx = vm.createContext({});
vm.runInContext(`${body}
var gate = { verdict: verdict, describe: describe, openExtList: openExtList, fileOk: fileOk, plain: plain, imageSrc: imageSrc };`, ctx);
const { verdict, describe, openExtList, fileOk, plain, imageSrc } = ctx.gate;
const V = JSON.parse(file("../crates/app/src/testvectors/member-content.json"));

test("links Qt would read differently from a browser are refused (desktop only)", () => {
  for (const [u, why] of [["https:evil.example", "no-host"], ["https:///evil.example", "no-host"], ["https:\\\\evil.example", "bad-char"],
                          ["https://evil.example\\@good.example/", "bad-char"]]) {
    const v = verdict(u);
    assert.equal(v.open, false, u);
    assert.equal(v.why, why, u);
  }
});

test("the shared link vectors", () => {
  assert.ok(V.links.length >= 30);
  for (const c of V.links) {
    const v = verdict(c.url);
    assert.equal(v.open, c.open, JSON.stringify(c.url));
    if (!c.open && c.why) assert.equal(v.why, c.why, JSON.stringify(c.url));
    if (c.open && c.as !== undefined) assert.equal(v.url, c.as, JSON.stringify(c.url));
    if (c.open) assert.equal(v.ask, c.ask, JSON.stringify(c.url));
  }
});

test("what an allowed link opens is the cleaned link", () => {
  assert.equal(verdict("  https://example.org/x \n").url, "https://example.org/x");
  assert.equal(verdict("https://example.org/a b").url, "https://example.org/a b");
  assert.equal(verdict(null).open, false);
  assert.equal(verdict(undefined).open, false);
});

test("the hover hint names the real host", () => {
  assert.equal(describe("https://bank.example@evil.example/x"), "", "a user name before the host: refused");
  assert.equal(describe("https://Example.ORG:8448/x"), "example.org:8448");
  assert.equal(describe("mailto:ana@example.org?subject=x"), "ana@example.org");
  assert.equal(describe("file:///etc/passwd"), "");
});

test("an attachment opens once: one onMediaOpenedChanged handler in the whole UI (the old double Open)", () => {
  const dir = fileURLToPath(new URL("../crates/app/qml/", import.meta.url));
  const hits = readdirSync(dir).filter((f) => f.endsWith(".qml")).flatMap((f) => (readFileSync(dir + f, "utf8").match(/function onMediaOpenedChanged\b/g) ?? []).map(() => f));
  assert.deepEqual(hits, ["ConversationView.qml"]);
});

test("attachments: viewer.rs's extensions, file:// only", () => {
  const rs = file("../crates/app/src/viewer.rs");
  const m = /pub const OPEN_EXTS: &\[&str\] = &\[([^\]]*)\]/.exec(rs);
  assert.ok(m, "viewer.rs OPEN_EXTS not found");
  const theirs = [...m[1].matchAll(/"([a-z0-9]+)"/g)].map((x) => x[1]);
  assert.deepEqual([...openExtList()], theirs);
  const exts = openExtList();
  for (const ok of ["file:///home/a/.cache/TinHat/media/_abc.png", "file:///C:/Users/a b/AppData/Local/TinHat/media/x.PDF", "FILE:///tmp/x.pdf"])
    assert.equal(fileOk(ok, exts), true, ok);
  for (const no of ["file:///tmp/x.img", "file:///tmp/x.txt", "file:///tmp/x.svg", "file:///tmp/x.html", "file:///tmp/x.exe", "file:///tmp/x.desktop",
                    "file:///tmp/x", "https://example.org/x.png", "smb://host/x.png", "file:///tmp/x.png?y", "file:///tmp/x.png#y",
                    "file:///tmp\\x.png", "file://host/share/x.pdf", "file:///x.pdf\u0000.exe", "", null])
    assert.equal(fileOk(no, exts), false, String(no));
  assert.equal(fileOk("file:///home/a/tinhat-diagnostics.txt", ["txt"]), true);
  assert.equal(fileOk("file:///home/a/tinhat-diagnostics.zip", ["txt"]), false);
});

test("plain(): someone's text on a style-drawn control can't read as markup", () => {
  assert.equal(plain('<img src="https://evil.example/p">Ana'), '\u2039img src="https://evil.example/p">Ana');
  assert.equal(plain("Tom & Jerry <3"), "Tom & Jerry \u20393");
  assert.equal(plain("plain"), "plain");
  assert.equal(plain(null), "");
  assert.equal(plain(undefined), "");
  assert.equal(plain(42), "42");
  assert.ok(!plain("<b>x</b><i>y").includes("<"));
});

test("imageSrc(): Media Party art is drawn only from https:// (launch-gate's review #5, desktop only)", () => {
  for (const u of ["https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg", "HTTPS://i.ytimg.com/a.jpg", " https://x.example/a.png "]) {
    assert.equal(imageSrc(u), u.trim(), u);
  }
  for (const u of ["file://attacker.example/share/a.png", "file:///C:/Users/a.png", "\\\\attacker.example\\share\\a.png",
                   "//attacker.example/a.png", "http://x.example/a.png", "qrc:/qt/qml/a.png", "image://theme/a", "data:image/png;base64,AAAA",
                   "smb://attacker.example/a.png", "https:///a.png", "https:\\\\x.example\\a.png", "https://x.example/a b.png",
                   "https://x.example\\@y.example/a.png", "", null, undefined, "https://" + "a".repeat(5000)]) {
    assert.equal(imageSrc(u), "", String(u));
  }
});
