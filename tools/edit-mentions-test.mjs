// edit-mentions-test.mjs — L1 A09.3: the composer sends the people picked while editing with the edit
// (editMessageMentions); Rust keeps the original's links that are still in the text (room.rs edit_mentions, checked
// against testvectors/edit-mentions by edit_content_tests). Tin Hat Web: src/features/chat/lib/editMentions.test.ts.
//   node --test tools/edit-mentions-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");

test("the edit carries the picks; Rust reads the original's links", () => {
  assert.match(file("../crates/app/qml/ConversationView.qml"), /conv\.room\.editMessageMentions\(st0\.id, t, JSON\.stringify\(composer\._mentions\)\)/);
  const rs = file("../crates/app/src/room.rs");
  assert.match(rs, /fn edit_message_mentions\(self: Pin<&mut RoomController>, event_id: &QString, text: &QString, mentions_json: &QString\);/);
  assert.match(rs, /let formatted = row_str\(&cur, &eid, "formatted"\)\.unwrap_or_default\(\);\n\s+let mentions = edit_mentions\(&formatted, &text, &picks\);\n\s+let notify = edit_notify_ids\(&formatted, &mentions\);/);
  assert.match(rs, /include_str!\("\.\.\/\.\.\/\.\.\/testvectors\/edit-mentions\/vectors\.json"\)/);
});
