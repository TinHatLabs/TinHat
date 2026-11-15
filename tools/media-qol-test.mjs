// media-qol-test.mjs — small media QOL (A00.5 / A08.6 / A08.7; Tin Hat Web's src/features/chat/mediaQol.test.ts checks
// the same on its side): the timeline play button has a name and a tip, the viewer toggles 2× on a double-click, an
// inline image opens from the keyboard, and a staged file shows its size.
//   node --test tools/media-qol-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const bubble = file("../crates/app/qml/ChatBubble.qml");
const lb = file("../crates/app/qml/Lightbox.qml");
const cv = file("../crates/app/qml/ConversationView.qml");
const room = file("../crates/app/src/room.rs");

test("A00.5: the play button has a name and a tip", () => {
  assert.match(bubble, /Accessible\.name: audioPlayer\.playbackState === MediaPlayer\.PlayingState \? "Pause" : "Play"/);
  assert.match(bubble, /Controls\.ToolTip\.text: Accessible\.name/);
});

test("A08.6: a double-click toggles 2×", () => {
  assert.match(lb, /onDoubleTapped: \{ var s = img\.scale > 1\.0 \? 1\.0 : 2\.0; img\.scale = s;/);
});

test("A08.7: inline images open from the keyboard; a staged file shows its size", () => {
  for (const id of ["inlineImgOpen", "inlineGifOpen"]) {
    const ma = bubble.split(`id: ${id}`)[1]?.split("Rectangle {")[0] ?? "";
    assert.match(ma, /activeFocusOnTab: true/, id);
    assert.match(ma, /Accessible\.name: "Open image"/, id);
    assert.match(ma, /Keys\.onReturnPressed: bubble\.imageClicked\(/, id);
  }
  assert.match(cv, /size: size \}\)/);
  assert.match(cv, /text: conv\.fmtSize\(modelData\.size\)/);
  assert.match(room, /#\[cxx_name = "localFileSize"\]/);
});

// QOL A29.5: the busy words carry the size (Tin Hat Web's MediaBlocks busyWithSize: the same cases in mediaQol.test.ts)
const fnSrc = (name) => {
  const start = bubble.indexOf(`    function ${name}(`);
  const end = bubble.indexOf("\n    }\n", start);
  return bubble.slice(start, end + 6);
};
test("A29.5: Downloading / Opening say the size", async () => {
  const vm = await import("node:vm");
  const ctx = vm.createContext({});
  vm.runInContext(`var bubble = {}; ${fnSrc("fmtSize")}\n${fnSrc("busyWithSize")}\nbubble.fmtSize = fmtSize;`, ctx);
  assert.equal(ctx.busyWithSize("Downloading", 4718592), "Downloading 4.5 MB…");
  assert.equal(ctx.busyWithSize("Opening", 2048), "Opening 2.0 KB…");
  assert.equal(ctx.busyWithSize("Downloading", 0), "Downloading…");
  assert.match(bubble, /\? bubble\.busyWithSize\("Downloading", bubble\.fileSize\)/);
  assert.match(bubble, /bubble\.opening \? bubble\.busyWithSize\("Opening", bubble\.fileSize\)/);
});
