// now-playing-test.mjs — one player at a time (QOL A00.1): runs the pure npStep of crates/app/qml/NowPlaying.qml
// (between its BEGIN now-playing pure / END now-playing pure markers, the code the app runs) on the shared vectors
// crates/app/src/testvectors/now-playing.json (Tin Hat Web replays the same file: src/features/chat/lib/nowPlaying.test.ts),
// and checks that every inline player (voice note / audio clip, video, the draft preview) and the recorder use it.
//   node --test tools/now-playing-test.mjs
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const path = (p) => fileURLToPath(new URL(p, import.meta.url));
const file = (p) => (existsSync(path(p)) ? readFileSync(path(p), "utf8") : "");
const np = file("../crates/app/qml/NowPlaying.qml");
const bubble = file("../crates/app/qml/ChatBubble.qml");
const rec = file("../crates/app/qml/VoiceRecorderRow.qml");
const conv = file("../crates/app/qml/ConversationView.qml");
const build = file("../crates/app/build.rs");
const embed = file("../crates/app/qml/ThEmbedCard.qml");
const V = JSON.parse(file("../crates/app/src/testvectors/now-playing.json"));

test("npStep: the shared vectors", () => {
  const body = np.split("// BEGIN now-playing pure")[1]?.split("// END now-playing pure")[0]?.replace(/^[^\n]*\n/, "");
  assert.ok(body, "NowPlaying.qml has no BEGIN now-playing pure / END now-playing pure block");
  const ctx = vm.createContext({});
  vm.runInContext(`${body}\nvar step = npStep;`, ctx);
  assert.ok(V.cases.length >= 10);
  for (const c of V.cases) {
    let st = { cur: null };
    const out = [];
    for (const op of c.ops) {
      const r = ctx.step(st, op);
      st = r.st;
      if (r.pause) out.push(r.pause);
    }
    assert.deepEqual(out, c.out, c.name);
    assert.equal(st.cur, c.cur, c.name + " (cur)");
  }
});

test("NowPlaying is a registered singleton", () => {
  assert.match(build, /QmlFile::from\("qml\/NowPlaying\.qml"\)\.singleton\(true\)/);
  assert.match(np, /^pragma Singleton/m);
});

// each player: tells NowPlaying when it starts / stops and pauses itself when asked
function wired(src, player, label) {
  assert.match(src, new RegExp(`NowPlaying\\.playing\\(\\s*${label}`), `${label}: reports play`);
  assert.match(src, new RegExp(`NowPlaying\\.stopped\\(\\s*${label}`), `${label}: reports stop`);
  if (player) assert.match(src, new RegExp(`onPauseRequested[\\s\\S]{0,200}${label}[\\s\\S]{0,80}${player}\\.pause\\(\\)`), `${label}: pauses when asked`);
  assert.match(src, new RegExp(`${label}\\s*=\\s*NowPlaying\\.newId\\(`), `${label}: gets its id once`);
}

test("every inline player takes part", () => {
  wired(bubble, "audioPlayer", "bubble\\._npAudio");
  // the video sits in a lazily made surface: the bubble relays the request (pauseVideo) to it
  wired(bubble, null, "bubble\\._npVideo");
  assert.match(bubble, /bubble\._npVideo\) bubble\.pauseVideo\(\)/);
  assert.match(bubble, /function onPauseVideo\(\)[^\n]*vplayer\.pause\(\)/);
  wired(rec, "player", "row\\._npDraft");
  wired(embed, "player", "card\\._npEmbed");
});

test("starting a recording pauses what plays", () => {
  const start = conv.split("function vnStart()")[1]?.split("\n    }")[0] ?? "";
  assert.match(start, /NowPlaying\.recording\(\)[\s\S]*voiceCtl\.start/, "vnStart pauses playback before the mic opens");
});
