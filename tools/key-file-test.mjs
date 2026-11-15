// key-file-test.mjs — L1 A28.1: the desktop's key screen can save the Security Key as a file (Tin Hat Web's Download;
// the text itself is bridge.rs key_file_text, checked against testvectors/security-key-file by its key_file_tests), and
// a saved file ticks "I've saved this key" (the web's keySaved: copied or downloaded).
//   node --test tools/key-file-test.mjs
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { test } from "node:test";

const file = (p) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
const dlg = file("../crates/app/qml/VerifyDeviceDialog.qml");
const main = file("../crates/app/qml/Main.qml");
const V = JSON.parse(file("../testvectors/security-key-file/vectors.json"));

test("Save as file… on the key screen, named like the web's file", () => {
  assert.match(dlg, /text: "Save as file…"\n\s+icon\.name: "document-save-symbolic"\n\s+onClicked: \{ keyFileDialog\.selectedFile = keyFileDialog\.currentFolder \+ "\/(.+?)"; keyFileDialog\.open\(\) \}/);
  assert.equal(dlg.match(/keyFileDialog\.currentFolder \+ "\/(.+?)"/)[1], V.fileName);
  assert.match(dlg, /onAccepted: dlg\.saveKeyRequested\(decodeURIComponent\(String\(selectedFile\)\)\)/);
  assert.match(dlg, /function keySavedToFile\(\) \{ savedCheck\.checked = true \}/);
});

test("Main writes it through login.saveRecoveryKey and ticks the box on success", () => {
  assert.match(main, /onSaveKeyRequested: \(url\) => \{\n\s+var err = login\.saveRecoveryKey\(url, Qt\.formatDate\(new Date\(\), "yyyy-MM-dd"\)\)\n\s+if \(err\.length === 0\) \{ verifySheet\.keySavedToFile\(\);/);
  assert.match(file("../crates/app/src/bridge.rs"), /fn save_recovery_key\(self: &LoginController, file_url: &QString, local_date: &QString\) -> QString;/);
});
