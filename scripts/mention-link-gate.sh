#!/bin/bash
# mention-link-gate.sh — runs ChatBubble.qml's OWN mentionTarget() (extracted from the file, so the
# test can't drift from the shipped code) in the real QML JS engine against the shared W2 vectors
# (Tin Hat Web html.ts: a percent-encoded mention is a mention; a stray '%' must not throw).
set -u
cd "$(dirname "$0")/.." || exit 2
QML=${QML:-/usr/lib/qt6/bin/qml}
[ -x "$QML" ] || QML=$(command -v qml6 || command -v qml)
SRC=crates/app/qml/ChatBubble.qml
FN=$(python3 - "$SRC" <<'PY'
import re, sys
t = open(sys.argv[1], encoding="utf-8").read()
m = re.search(r"\n(    function mentionTarget\(link\) \{\n.*?\n    \})\n", t, re.S)
print(m.group(1) if m else "")
PY
)
[ -n "$FN" ] || { echo "MENTION-GATE FAIL: mentionTarget() not found in $SRC"; exit 1; }
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
cat > "$T/gate.qml" <<QMLEOF
import QtQuick
Item {
$FN
    Component.onCompleted: {
        // SHARED W2 VECTORS — identical to tinhat-web src/features/chat/lib/mention.test.ts ("" = the web's null)
        var cases = [
            ["https://matrix.to/#/@tester1:example.com", "@tester1:example.com"],
            ["https://matrix.to/#/%40tester1%3Aexample.com", "@tester1:example.com"],
            ["https://matrix.to/#/%40bob%3aexample.com", "@bob:example.com"],
            ["https://matrix.to/#/@bob%3Aexample.com", "@bob:example.com"],
            ["https://matrix.to/#/@50%:example.com", "@50%:example.com"],
            ["https://matrix.to/#/@%E0%A4%A:example.com", "@%E0%A4%A:example.com"],
            ["https://matrix.to/#/%40bad%zz%3Aserver", "@bad%zz:server"],
            ["https://matrix.to/#/@bob:example.com/\$event?via=x", "@bob:example.com"],
            ["https://matrix.to/#/@bob:example.com?via=example.com", "@bob:example.com"],
            ["https://matrix.to/#/!room:example.com", ""],
            ["https://matrix.to/#/%23general%3Aexample.com", ""],
            ["https://matrix.to/#/@", ""],
            ["https://example.com/#/@bob:example.com", ""],
            ["http://matrix.to/#/@bob:example.com", ""],
            ["https://matrix.to/#/#lounge:example.com", ""],
            ["https://matrix.to/#/", ""]
        ]
        var fails = 0
        for (var i = 0; i < cases.length; i++) {
            var got
            try { got = mentionTarget(cases[i][0]) } catch (e) { got = "THREW " + e }
            var ok = got === cases[i][1]
            if (!ok) fails++
            console.log((ok ? "  ok   " : "  FAIL ") + cases[i][0] + " -> " + JSON.stringify(got))
        }
        console.log(fails === 0 ? "MENTION-GATE PASS" : ("MENTION-GATE FAIL: " + fails))
        Qt.exit(fails === 0 ? 0 : 1)
    }
}
QMLEOF
QT_QPA_PLATFORM=offscreen QT_ASSUME_STDERR_HAS_CONSOLE=1 QT_LOGGING_RULES="qml.debug=true;js.debug=true" "$QML" "$T/gate.qml" 2>&1 | sed 's/^qml: //'
exit ${PIPESTATUS[0]}
