// linkify-test.mjs — runs RightPanel.qml's linkify (between its BEGIN linkify / END linkify markers, the exact code the
// app runs) on the cases Tin Hat Web's src/features/spaces/linkify.test.ts checks for the same walk (assembly verify C,
// sec-night 2026-10-27). The desktop's href is the escaped address as typed (LinkGate checks it at the click); the web's
// is the normalised address, so only the visible text and the trailing punctuation are compared with the web's cases.
//   node --test tools/linkify-test.mjs     (scripts/run-gates.sh runs it with the unit tests)
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const qml = readFileSync(fileURLToPath(new URL("../crates/app/qml/RightPanel.qml", import.meta.url)), "utf8");
const body = qml.split("// BEGIN linkify")[1]?.split("// END linkify")[0]?.replace(/^[^\n]*\n/, "");
assert.ok(body, "RightPanel.qml lost its BEGIN linkify / END linkify markers");
const ctx = vm.createContext({});
vm.runInContext(`${body}\nvar out = { linkify: linkify };`, ctx);
const { linkify } = ctx.out;

test("links, trailing punctuation outside, newlines", () => {
  assert.equal(linkify("see https://example.org/a."), 'see <a href="https://example.org/a">https://example.org/a</a>.');
  assert.equal(linkify("a\nb"), "a<br>b");
  assert.equal(linkify("x https://example.org/?q=1&r=2"), 'x <a href="https://example.org/?q=1&amp;r=2">https://example.org/?q=1&amp;r=2</a>');
  assert.equal(linkify(""), "");
  assert.equal(linkify(null), "");
});

test("markup stays text; a quote can't leave the href", () => {
  assert.equal(linkify("<b>x</b>"), "&lt;b&gt;x&lt;/b&gt;");
  const out = linkify('https://example.org/"onmouseover="x');
  assert.ok(!out.includes('onmouseover="'), out);
});

test("trimmed in the raw text, then escaped (assembly verify C)", () => {
  assert.equal(linkify('see https://example.org/?q="x";'), 'see <a href="https://example.org/?q=&quot;x&quot;">https://example.org/?q=&quot;x&quot;</a>;');
  assert.equal(linkify("https://example.org/a&b."), '<a href="https://example.org/a&amp;b">https://example.org/a&amp;b</a>.');
  assert.equal(linkify("a https://example.org/x\nb"), 'a <a href="https://example.org/x">https://example.org/x</a><br>b');
  assert.equal(linkify("(https://example.org/x)."), '(<a href="https://example.org/x">https://example.org/x</a>).');
  assert.equal(linkify("http://"), "http://");
});
