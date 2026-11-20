#!/bin/bash
# qml-lint.sh — static check for QML that qml-smoke.sh CANNOT reach.
#
# qml-smoke launches the app and asserts a window appears, which only exercises the
# BOOT path. A component that is instantiated later (RightPanel when you open a room,
# a popup when you click a menu item) can be broken and still smoke-PASS — you find out
# when a user clicks it. This lints every QML file up front.
#
# Fails ONLY on the categories that actually break loading:
#   • syntax errors
#   • "is not a type" (missing/unregistered component — the 0.3.5 class)
# Layout-positioning warnings are REPORTED but not fatal: the tree has pre-existing ones
# and a gate that cries wolf gets ignored, which is worse than no gate.
#
# Unqualified-access warnings are ignored by design: context properties (settings, room,
# soundboardCtl, …) are injected from Rust and always read as "unqualified" here.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
# Default: the qml/ of the repo this script lives in (a worktree lints its own tree).
QML_DIR=${1:-$(cd "$(dirname "$0")/.." && pwd)/crates/app/qml}
LINT=${QMLLINT:-/usr/lib/qt6/bin/qmllint}

[ -x "$LINT" ] || { echo "QMLLINT FAIL: no qmllint at $LINT"; exit 2; }
cd "$QML_DIR" || { echo "QMLLINT FAIL: no such dir $QML_DIR"; exit 2; }

fatal=0
layout=0
# Walk subdirectories: the Mint shell lives in mint/ and was never linted.
for f in $(find . -name "*.qml" | sed "s|^\./||" | sort); do
    out=$("$LINT" "$f" 2>&1)
    syn=$(echo "$out" | grep -cE "Syntax error|Expected token|Unexpected token")
    nt=$(echo "$out" | grep -c "is not a type")
    # A DIRECT assignment to a property the type does not have. Fatal: the document fails to load
    # and everything that instantiates it fails with "Type <X> unavailable". Deliberately NOT the
    # "Member ... not found on type" form, which is qmllint failing to type an expression.
    nop=$(echo "$out" | grep -c "Could not find property")
    lay=$(echo "$out" | grep -c "layout-positioning")
    layout=$((layout + lay))
    # Duplicate `id:` inside one document = "id is not unique" at load time — the whole
    # component (and everything that instantiates it) fails to load. qmllint is silent
    # about it (found the hard way on 0.8.16).
    # 0.8.22: also catch one-line forms (`Item { id: foo; ... }`) — a duplicate slipped past the
    # line-anchored pattern and killed the whole Main.qml load (id is not unique).
    # A QML id declaration ENDS its statement; `({ id: modelData, kind: … })` is a JS object key
    # and must not count (CallView has two such literals — they read as a duplicate id).
    dup=$(python3 -c '
import re, sys, collections
text = open(sys.argv[1], encoding="utf-8").read()
text = re.sub(r"^[ \t]*//.*$", "", text, flags=re.M)
names = re.findall(r"(?:^|[\s{])id:[ \t]*([A-Za-z_]\w*)[ \t]*(?=[;}\n])", text)
print(" ".join(n for n, c in collections.Counter(names).items() if c > 1))
' "$f")
    if [ "$syn" != "0" ] || [ "$nt" != "0" ] || [ "$nop" != "0" ] || [ -n "$dup" ]; then
        echo "QMLLINT FAIL: $f (syntax=$syn not-a-type=$nt no-such-property=$nop duplicate-ids='$dup')"
        echo "$out" | grep -E "Syntax error|Expected token|Unexpected token|is not a type|Could not find property" | head -5
        fatal=$((fatal + 1))
    fi
done

# A property that redeclares a FINAL one of its object's type (Item.activeFocusOnTab, Dialog.result, …): Qt 6.11
# refuses the whole component ("Cannot override FINAL property") and every file that uses it — qmllint says nothing.
# (10.7 candidate s1: ThemeCard's `activeFocusOnTab` alias kept Main.qml from loading at all.)
fin=$(python3 "$HERE/qml-final-check.py" "$QML_DIR" $(find "$QML_DIR" -name "*.qml" | sort) 2>&1)
if [ $? -ne 0 ]; then
    echo "$fin" | grep -v "^qml-final-check:" | sed "s/^/QMLLINT FAIL: /"
    n=$(echo "$fin" | grep -c "redeclares a FINAL")
    fatal=$((fatal + (n > 0 ? n : 1)))
fi

# An alias to a member our own component does not declare ("Invalid alias target location"): refuses the component at
# run time and qmllint does not resolve it (11.0: AppCore's renamed _navPartTick left Main.qml's alias → no window).
al=$(python3 "$HERE/qml-alias-check.py" "$QML_DIR" 2>&1)
if [ $? -ne 0 ]; then
    echo "$al" | grep -v "^qml-alias-check:" | sed "s/^/QMLLINT FAIL: /"
    fatal=$((fatal + 1))
fi

# ECMAScript's old future-reserved words as JS names (short, int, final, ...): Qt 6.11 accepts them, Qt 6.8's
# qmlcachegen (the Windows build) refuses the whole file (10.8: UpNextWidget's dayWord(ms, short) stopped the Windows
# build).
rs=$(python3 "$HERE/qml-reserved-check.py" "$QML_DIR" 2>&1)
if [ $? -ne 0 ]; then
    echo "$rs" | grep -v "^reserved-word names:" | sed "s/^/QMLLINT FAIL: /"
    fatal=$((fatal + 1))
fi

# Security review 2026-10-26 (M7, H2): bound label text declares its textFormat (other people's text is never markup),
# style-drawn controls show bound text through LinkGate.plain, and only LinkGate.qml calls Qt.openUrlExternally.
tf=$(python3 "$HERE/qml-textformat-lint.py" "$QML_DIR" 2>&1)
if [ $? -ne 0 ]; then
    echo "$tf" | grep -v "PENDING" | sed "s/^/QMLLINT FAIL: /"
    fatal=$((fatal + 1))
fi

# 12.1.8 (docs/KEYS-IN-WINDOWS.md): every window Shortcut is gated on ITS window having the keyboard (the main window and
# its transient children count as active together: a hidden window's ungated Escape took the key from the main window).
wk=$(python3 "$HERE/qml-window-keys-check.py" --selftest 2>&1 && python3 "$HERE/qml-window-keys-check.py" "$QML_DIR" 2>&1)
if [ $? -ne 0 ]; then
    echo "$wk" | grep -v "^qml-window-keys-check:" | sed "s/^/QMLLINT FAIL: /"
    fatal=$((fatal + 1))
fi

# 12.1.11 (assembly-047): AppCore.qml's call identity rules (tileMxid / isParticipantOf / sameParticipant) run by the QML
# engine against the shared testvectors/identity.json (identity.rs + the web's identity.test.ts run the same groups).
QMLRUN=${QML6:-$(command -v qml6 || echo /usr/lib/qt6/bin/qml)}
if [ -x "$QMLRUN" ]; then
    idq=$(QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/qml-identity-test.qml" -- "$(pwd)" 2>&1)
    if [ $? -ne 0 ]; then
        echo "$idq" | grep -E "FAIL" | head -10 | sed "s/^/QMLLINT FAIL: identity: /"
        fatal=$((fatal + 1))
    fi
else
    echo "QMLLINT FAIL: no qml runtime for qml-identity-test.qml (set QML6)"
    fatal=$((fatal + 1))
fi

# 12.1.15: the share picker's "Custom…" panel. ShareQuick.qml's rules run by the QML engine against the shared
# testvectors/share-custom-quick (the web's shareQuick.test.ts runs the same file), AppCore.qml's lists against the
# vectors' lists, and the panel's words and wiring in StreamPopover.qml.
if [ -x "$QMLRUN" ]; then
    cq=$(QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/share-custom-quick-test.qml" -- "$(cd "$HERE/.." && pwd)" 2>&1)
    if [ $? -ne 0 ] || ! echo "$cq" | grep -q "VERDICT: PASS"; then
        echo "$cq" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: share-custom-quick: /"
        fatal=$((fatal + 1))
    fi
fi

# 12.1.16 (L2, P2 a): the "Jump to NEW" pill's "seen" rule on a real ListView (scripts/new-pill-test.qml).
if [ -x "$QMLRUN" ]; then
    np=$(QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/new-pill-test.qml" -- "$(cd "$HERE/.." && pwd)" 2>&1)
    if [ $? -ne 0 ] || ! echo "$np" | grep -q "VERDICT: PASS"; then
        echo "$np" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: new-pill: /"
        fatal=$((fatal + 1))
    fi
fi

# 12.1.15 (review A3-5): a restart that waits for the call treats a reconnecting or still-hanging-up call as a call
# (Main.qml's uiScaleCallUp, the after-call Timer, the updater's callUp, read from the file and evaluated).
if [ -x "$QMLRUN" ]; then
    rq=$(QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/restart-after-call-test.qml" -- "$(cd "$HERE/.." && pwd)" 2>&1)
    if [ $? -ne 0 ] || ! echo "$rq" | grep -q "VERDICT: PASS"; then
        echo "$rq" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: restart-after-call: /"
        fatal=$((fatal + 1))
    fi
fi

# 12.1.16 (L6: P9): double-click a call tile + the tile menu's Pin / Full screen rows: AppCore.qml's rules against the shared
# testvectors/tile-gestures (the web's tileGestures.test.ts runs the same file) and their wiring.
if [ -x "$QMLRUN" ]; then
    tg=$(QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/tile-gestures-test.qml" -- "$(cd "$HERE/.." && pwd)" 2>&1)
    if [ $? -ne 0 ] || ! echo "$tg" | grep -q "VERDICT: PASS"; then
        echo "$tg" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: tile-gestures: /"
        fatal=$((fatal + 1))
    fi
fi

# 12.1.16 (lane sun-qol-parked): the QOL sweep leftovers' rules against the vectors Tin Hat Web runs too, and their wiring
if [ -x "$QMLRUN" ]; then
    qp=$(QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/qol-parked-test.qml" -- "$(cd "$HERE/.." && pwd)" 2>&1)
    if [ $? -ne 0 ] || ! echo "$qp" | grep -q "VERDICT: PASS"; then
        echo "$qp" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: qol-parked: /"
        fatal=$((fatal + 1))
    fi
fi

# A33.3 / A33.4 (12.1.16): a remembered window spot on an unplugged screen is not restored; pop-outs step down-right
# (ScreenFit.qml's rules + the wiring in WindowMemory / AppCore / WatchPartyWindow).
if [ -x "$QMLRUN" ]; then
    sq=$(QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/screen-fit-test.qml" -- "$(cd "$HERE/.." && pwd)" 2>&1)
    if [ $? -ne 0 ] || ! echo "$sq" | grep -q "VERDICT: PASS"; then
        echo "$sq" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: screen-fit: /"
        fatal=$((fatal + 1))
    fi
fi

# 12.1.16 (QOL A21.1/A21.3/A21.5/D06.6): the Pad's words against the shared testvectors/pad-words (the web's
# src/features/pad/words.test.ts runs the same file): PadWindow.qml's computer words, "Leave that call" next to the
# greyed Watch, the mic's visible reason, and Settings → Calls → Pad's description.
if [ -x "$QMLRUN" ]; then
    pw=$(QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/pad-words-test.qml" -- "$(cd "$HERE/.." && pwd)" 2>&1)
    if [ $? -ne 0 ] || ! echo "$pw" | grep -q "VERDICT: PASS"; then
        echo "$pw" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: pad-words: /"
        fatal=$((fatal + 1))
    fi
fi

# A31.6 (12.1.16): ThToolButton's screen-reader name leaves the tip's shortcut to the description (the real rule, evaluated).
if [ -x "$QMLRUN" ]; then
    aq=$(QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/a11y-tip-test.qml" -- "$(cd "$HERE/.." && pwd)" 2>&1)
    if [ $? -ne 0 ] || ! echo "$aq" | grep -q "VERDICT: PASS"; then
        echo "$aq" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: a11y-tip: /"
        fatal=$((fatal + 1))
    fi
fi

# 12.1.16 (L6: A06.1 / A06.0 / A06.5): the share popover loaded for real (qmltestrunner, offscreen, stand-in app + call
# controller): it fits a short window with its action row on screen, the Linux source button keeps the picked audio, and
# the pills work from the keyboard.
spt=$(bash "$HERE/share-popover-test.sh" "$(pwd)" 2>&1)
if [ $? -ne 0 ] || ! echo "$spt" | grep -q "VERDICT: PASS"; then
    echo "$spt" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: share-popover: /"
    fatal=$((fatal + 1))
fi

# 12.1.17: the Chat background sheet loaded for real (qmltestrunner, offscreen, stand-in app + room): people who can't
# change the background see the other tiles greyed and their clicks change nothing; people who can see them clear.
bgs=$(bash "$HERE/background-sheet-test.sh" "$(pwd)" 2>&1)
if [ $? -ne 0 ] || ! echo "$bgs" | grep -q "VERDICT: PASS"; then
    echo "$bgs" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: background-sheet: /"
    fatal=$((fatal + 1))
fi

# 12.2.0 respin (#27): a game invite card after the call ENDED, GameChatCard.qml loaded for real (qmltestrunner, offscreen,
# stand-in app + PlayController): mCallRoomId outlives the call, so the card is the full one with "Join the call".
gcec=$(bash "$HERE/game-card-ended-call-test.sh" "$(pwd)" 2>&1)
if [ $? -ne 0 ] || ! echo "$gcec" | grep -q "VERDICT: PASS"; then
    echo "$gcec" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: game-card-ended-call: /"
    fatal=$((fatal + 1))
fi

# 12.1.17 (P22): Th.qml's row()/gap() — the person's text size + density as QML draws them — against the shared vectors
# testvectors/personal/metrics.json (crates/theme tests/personal.rs and the web's personal.test.ts run the same file).
if [ -x "$QMLRUN" ]; then
    pq=$(QT_QPA_PLATFORM=offscreen QML_XHR_ALLOW_FILE_READ=1 QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/personal-row-test.qml" -- "$(cd "$HERE/.." && pwd)" 2>&1)
    if [ $? -ne 0 ] || ! echo "$pq" | grep -q "VERDICT: PASS"; then
        echo "$pq" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: personal-row: /"
        fatal=$((fatal + 1))
    fi
fi

# 12.1.17 (P22): ClipProbe.qml (the text size gate's probe) reports planted clipped text and leaves intended cases alone
if [ -x "$QMLRUN" ]; then
    cpq=$(QT_QPA_PLATFORM=offscreen QT_ASSUME_STDERR_HAS_CONSOLE=1 \
        QT_LOGGING_RULES="qml.debug=true;js.debug=true" timeout 60 "$QMLRUN" "$HERE/clip-probe-test.qml" -- "$(cd "$HERE/.." && pwd)" 2>&1)
    if [ $? -ne 0 ] || ! echo "$cpq" | grep -q "VERDICT: PASS"; then
        echo "$cpq" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: clip-probe: /"
        fatal=$((fatal + 1))
    fi
fi

# 12.1.17 (A31.4): every QML text size follows the person's text size (scripts/text-scale-lint.py)
if ! tsl=$(python3 "$HERE/text-scale-lint.py" "$(cd "$HERE/.." && pwd)" 2>&1); then
    echo "$tsl" | grep -E "TEXT-SCALE-LINT|FAIL" | head -10 | sed "s/^/QMLLINT FAIL: text-scale: /"
    fatal=$((fatal + 1))
fi

# 12.1.17 (P22): the per-window font scope (ThWindow.qml) loaded for real: a secondary window's unsized controls take the
# person's text size, that window only, never app-wide (a plain Window, the control, keeps the platform size).
wfs=$(bash "$HERE/window-font-scope-test.sh" "$(cd "$HERE/.." && pwd)/crates/app/qml" 2>&1)
if [ $? -ne 0 ] || ! echo "$wfs" | grep -q "VERDICT: PASS"; then
    echo "$wfs" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: window-font-scope: /"
    fatal=$((fatal + 1))
fi

# 12.1.18 review #2: the Chat background sheet at 150 %: every tile's caption inside its tile; 112 x 92 at 100 % as before
bstt=$(bash "$HERE/background-sheet-textsize-test.sh" "$(cd "$HERE/.." && pwd)/crates/app/qml" 2>&1)
if [ $? -ne 0 ] || ! echo "$bstt" | grep -q "VERDICT: PASS"; then
    echo "$bstt" | grep -E "FAIL|rror" | head -10 | sed "s/^/QMLLINT FAIL: background-sheet-textsize: /"
    fatal=$((fatal + 1))
fi

if [ "$fatal" -eq 0 ]; then
    echo "QMLLINT PASS: no syntax / not-a-type / FINAL-override / alias-target / reserved-name / textFormat / window-keys / identity-rule / share-custom-quick / share-popover / background-sheet / game-card-ended-call / tile-gestures / pad-words / screen-fit / a11y-tip / qol-parked / personal-row / clip-probe / text-scale / window-font-scope errors (${layout} layout-positioning warnings, non-fatal)"
    exit 0
fi
echo "QMLLINT FAIL: $fatal file(s) would fail to load"
exit 1
