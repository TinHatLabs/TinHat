// catching-up-test.mjs — catching up in a busy room (12.1.16 lane L2): runs the pure functions of
// crates/app/qml/ConversationView.qml (between its BEGIN catching-up pure / END catching-up pure markers, the exact code
// the app runs) on the shared vectors testvectors/catching-up/vectors.json (Tin Hat Web replays a byte copy:
// src/features/chat/lib/catchup.test.ts), and checks that the timeline wires them.
//   node --test tools/catching-up-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const cv = file("../crates/app/qml/ConversationView.qml");
const body = cv.split("// BEGIN catching-up pure")[1]?.split("// END catching-up pure")[0];
assert.ok(body, "ConversationView.qml lost its BEGIN catching-up pure / END catching-up pure markers");
const ctx = vm.createContext({});
vm.runInContext(`${body}\nvar cu = { newLineStart: newLineStart, newPillShown: newPillShown, dayNeedsYear: typeof dayNeedsYear === "function" ? dayNeedsYear : null, newLineHint: typeof newLineHint === "function" ? newLineHint : null };`, ctx);
const { newLineStart, newPillShown, dayNeedsYear, newLineHint } = ctx.cu;
const V = JSON.parse(file("../testvectors/catching-up/vectors.json"));

test("newLineStart: the shared vectors (A35.1)", () => {
  assert.ok(V.newLineStart.length >= 8);
  for (const c of V.newLineStart) assert.equal(JSON.stringify(newLineStart(c.len, c.hint, c.canBack)), JSON.stringify(c.out), c.name);
});

test("newLineHint: the shared vectors (fix 1-7: a thread's view has no NEW line from the room's count), and the view uses it", () => {
  assert.equal(typeof newLineHint, "function", "the pure block has no newLineHint");
  assert.ok(V.newLineHint.length >= 4);
  for (const c of V.newLineHint) assert.equal(newLineHint(c.threadRoot, c.roomUnread), c.out, c.name);
  assert.match(cv, /conv\._unreadHint = conv\.newLineHint\(conv\.threadRoot, rooms\[i\]\.unread \|\| 0\)/);
  assert.doesNotMatch(cv, /conv\._unreadHint = rooms\[i\]\.unread/, "the room's count straight into the hint again");
});

test("newPillShown: the shared vectors (P2 a)", () => {
  for (const c of V.newPill) assert.equal(newPillShown(c.hasLine, c.waiting, c.seen), c.out, c.name);
});

test("dayNeedsYear: the shared vectors (A05.2), and the day separator uses it", () => {
  assert.equal(typeof dayNeedsYear, "function", "the pure block has no dayNeedsYear");
  for (const c of V.dayNeedsYear) assert.equal(dayNeedsYear(c.ts, c.now), c.out, c.name);
  assert.match(cv, /return Qt\.formatDate\(d, conv\.dayNeedsYear\(ts, now\.getTime\(\)\) \? "ddd, MMM d, yyyy" : "ddd, MMM d"\)/);
});

test("A05.5: the stamp's full date follows the app's clock setting", () => {
  const main = file("../crates/app/qml/Main.qml");
  const bubble = file("../crates/app/qml/ChatBubble.qml");
  assert.match(main, /function fmtFull\(ms\) \{/);
  assert.match(main, /if \(root\.clockMode === "12"\) return Qt\.formatDate\(d, "ddd, MMM d, yyyy"\) \+ " " \+ Qt\.formatTime\(d, "h:mm:ss AP"\)/);
  assert.match(main, /if \(root\.clockMode === "24"\) return Qt\.formatDate\(d, "ddd, MMM d, yyyy"\) \+ " " \+ Qt\.formatTime\(d, "HH:mm:ss"\)/);
  assert.match(bubble, /Controls\.ToolTip\.text: bubble\.theme\.fmtFull \? bubble\.theme\.fmtFull\(bubble\.ts\)/);
});

test("the timeline wires them: the divider waits for older history, the pill shows by the rule and jumps", () => {
  assert.match(cv, /s0 = conv\.newLineStart\(end, conv\._unreadHint, conv\.mHasMore\)/);
  assert.match(cv, /if \(s0 === "more"\) \{ conv\._newWaiting = true; return \}/);
  assert.doesNotMatch(cv, /start = Math\.max\(-1, arr\.length - 1 - conv\._unreadHint\)/);
  assert.match(cv, /visible: msgView\.count > 0 && conv\.newPillShown\(conv\.unreadDividerId\.length > 0, conv\._newWaiting, conv\._newSeen\)/);
  assert.match(cv, /onClicked: conv\.jumpToNew\(\)/);
  assert.match(cv, /text: "Jump to NEW"/);
  // a room switch starts the pill afresh
  assert.match(cv, /conv\._newWaiting = false; conv\._newSeen = false; conv\._newHunt = 0/);
});

test("the web keeps a byte copy of the vectors (when its checkout is known: TINHAT_WEB_DIR)", { skip: !process.env.TINHAT_WEB_DIR }, () => {
  const theirs = readFileSync(`${process.env.TINHAT_WEB_DIR}/testvectors/catching-up/vectors.json`, "utf8");
  assert.equal(theirs, file("../testvectors/catching-up/vectors.json"));
});

test("A33.2: notification clicks bring the window back un-minimised (bringToFront), never only raise it", () => {
  const main = file("../crates/app/qml/Main.qml");
  assert.match(main, /function bringToFront\(\) \{\n\s+if \(root\.visibility === Window\.Minimized \|\| root\.visibility === Window\.Hidden\) \{/);
  assert.match(main, /if \(root\._lastShownVisibility === Window\.Maximized\) root\.showMaximized\(\)/);
  for (const a of ["invite:", "open:", "compose:", "calendar:"]) {
    const at = main.indexOf(`if (action.indexOf("${a}") === 0) {`);
    assert.ok(at > 0, a);
    assert.match(main.slice(at, at + 400), /root\.bringToFront\(\)/, a);
  }
  assert.match(main, /if \(action\.indexOf\("join:"\) === 0\) \{ root\.bringToFront\(\); incomingToast\.join/);
  assert.doesNotMatch(main, /root\.visible = true; root\.raise\(\); root\.requestActivate\(\)\n/);
});

test("A35.4: Jump to latest / End takes the window back to the default depth", () => {
  assert.match(cv, /function jumpToLatest\(\) \{[\s\S]{0,800}if \(conv\.room && conv\.room\.trimToLatest\) conv\.room\.trimToLatest\(\)/);
  const rs = file("../crates/app/src/room.rs");
  assert.match(rs, /#\[qinvokable\]\n\s+fn trim_to_latest\(self: Pin<&mut RoomController>\);/);
  assert.match(rs, /limit\.store\(window_after_jump_to_latest\(limit\.load\(Ordering::Relaxed\)\), Ordering::Relaxed\);/);
});

test("D03.4: a room marked read (list, space, notification) marks its mentions seen", () => {
  const nav = file("../crates/app/src/nav.rs");
  assert.match(nav, /eprintln!\("\[nav\] mark read failed for \{id\}: \{e\}"\);\n\s+return false;\n\s+\}\n\s+\/\/ D03\.4[^\n]*\n\s+crate::mentions_ctl::room_seen_up_to\(id, cutoff\);/);
  assert.match(nav, /let cutoff = crate::mentions_ctl::now_ms\(\);[^\n]*\n\s+let mut opts = MessagesOptions::backward\(\);/);
});

test("A05.1 / A35.2: a jump to a message older than what is loaded pages back for it (12 × 50, as the web's 6 × 100)", () => {
  const at = cv.indexOf("function _jumpTo(eventId) {");
  const body = cv.slice(at, at + 1400);
  assert.match(body, /if \(conv\._jumpHunt !== eventId\) \{ conv\._jumpHunt = eventId; conv\._jumpPages = 12 \}/);
  assert.match(body, /if \(conv\._jumpPages > 0 && conv\.mHasMore && conv\.room\) \{\n\s+conv\._jumpPages--\n\s+if \(!conv\.room\.loadingOlder\) msgView\._huntOlder\(\)/);
  assert.match(cv, /function _huntStep\(\) \{\n\s+if \(conv\._jumpHunt\.length > 0\) conv\._jumpTo\(conv\._jumpHunt\)/);
  const main = file("../crates/app/qml/Main.qml");
  assert.match(main, /if \(here && \(cv\.hasEvent\(p\.id\) \|\| cv\._markerFrozen\)\) \{ root\._pendingJump = null; stop\(\); cv\.jumpTo\(p\.id\); return \}/);
});

test("A11.2: Mark all as read only with something unread, the label goes to Rust, and its note is shown", () => {
  const main = file("../crates/app/qml/Main.qml");
  const host = file("../crates/app/qml/ActionsHost.qml");
  assert.match(main, /text: "Mark all as read"\n[^\n]*\n[^\n]*\n\s+visible: !root\.ctxSpaceInvited && root\.ctxSpaceUnread > 0/);
  assert.match(main, /nav\.markSpaceRead\(root\.ctxRoomId, root\.ctxRoomName\)/);
  assert.match(main, /function onMarkedRead\(text\) \{ root\.showPassiveNotification\(LinkGate\.plain\(text\), "short"\) \}/);
  assert.match(host, /app\.nav\.markSpaceRead\(ctx\.subject, host\._nameOf\("space", ctx\.subject\)\)/);
  const nav = file("../crates/app/src/nav.rs");
  assert.match(nav, /if mark_read_owns\(&key, &r\) \{/);
  assert.match(nav, /\.buffer_unordered\(8\)/);
});

test("D03.3: the session room list (bridge rooms_json) puts an unread mention with the new DMs too", () => {
  const br = file("../crates/app/src/bridge.rs");
  assert.match(br, /unread_of\(v\) && \(v\["dm"\]\.as_bool\(\)\.unwrap_or\(false\) \|\| v\["highlight"\]\.as_bool\(\)\.unwrap_or\(false\)\)/);
});

test("A11.3: Home / Favourites / People / Rooms get a menu with Mark all as read (when something is unread)", () => {
  const ni = file("../crates/app/qml/NavIsland.qml");
  const main = file("../crates/app/qml/Main.qml");
  assert.equal((ni.match(/if \(mouse\.button === Qt\.RightButton\) island\.openMenu\(/g) || []).length, 2, "both meta rows (column + row)");
  assert.match(ni, /if \(sp && \(Number\(sp\.unread\) \|\| 0\) > 0 && island\.app && island\.app\.openMetaContext\) island\.app\.openMetaContext\(sp\.key, sp\.label\)/);
  assert.match(main, /onTriggered: \{ nav\.markSpaceRead\(root\.ctxMetaKey, root\.ctxMetaLabel\);/);
});

test("review: the hunts step on the poll (never on a loadingOlder edge), End cancels them, an empty marker waits too", () => {
  assert.match(cv, /if \(conv\._huntAsked && !conv\.room\.loadingOlder\n\s+&& \(\(msgModel\.count > 0 && msgModel\.get\(0\)\.eventId !== conv\._huntFirstId\) \|\| !conv\.mHasMore \|\| conv\.mStatus\.length > 0\)\) \{/);
  assert.match(cv, /olderAnchor = -1; olderAnchorOff = 0/);
  assert.match(cv, /if \(conv\._newWaiting && conv\._markerFrozen\) conv\._recomputeDivider\(\)\n\s+\/\/ A05\.1 \+ P2 a/);
  assert.match(cv, /s0 = conv\.newLineStart\(end, conv\._unreadHint, conv\.mHasMore\)/);
  assert.doesNotMatch(cv, /if \(!conv\.mLoadingOlder && conv\._newHunt > 0\)/);
  assert.match(cv, /conv\._newHunt = 0; conv\._jumpHunt = ""; conv\._jumpPages = 0; conv\._huntAsked = false/);
  const rs = file("../crates/app/src/room.rs");
  assert.match(rs, /self\.as_mut\(\)\.rust_mut\(\)\.trim_pending = true;/);
  assert.match(rs, /if std::mem::take\(&mut o\.as_mut\(\)\.rust_mut\(\)\.trim_pending\) \{/);
  const nt = file("../crates/app/src/notify.rs");
  assert.match(nt, /let gen = crate::session::generation\(\);\n\s+let _one_at_a_time = scan_gate\(\)\.lock\(\)\.await;\n\s+if crate::session::generation\(\) != gen \{/);
  assert.match(nt, /if a\.is_some_and\(\|\(g, x\)\| g == gen && x <= at && x > Instant::now\(\)\)/);
  // 12.1.16 fix 1-1: a thread reply's toast closes only when that thread is on screen (not merely the room)
  assert.match(nt, /filter\(\|o\| !o\.unread_on_server \|\| room_attended_for_thread\(&o\.id, &posted_thread\(&o\.id\)\)\)/);
});
