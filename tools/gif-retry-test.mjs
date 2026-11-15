// gif-retry-test.mjs — the GIF picker's failed load gets Try again (QOL A08.5; Tin Hat Web's
// src/features/chat/gifRetry.test.ts checks its side): the button shows only on an error and runs the same search again.
//   node --test tools/gif-retry-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { test } from "node:test";

const cv = readFileSync(fileURLToPath(new URL("../crates/app/qml/ConversationView.qml", import.meta.url)), "utf8");

test("A08.5: Try again under Couldn't load GIFs", () => {
  const b = cv.split("id: gifRetry")[1]?.split("}")[0] ?? "";
  assert.match(b, /visible: bubbleGifPopup\.gifs\.length === 0 && !!conv\.gif && !conv\.gif\.loading && conv\.gif\.error\.length > 0/);
  assert.match(b, /text: "Try again"/);
  assert.match(b, /onClicked: conv\.gif\.search\(bubbleGifSearch\.text\)/);
});
