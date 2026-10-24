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

if [ "$fatal" -eq 0 ]; then
    echo "QMLLINT PASS: no syntax / not-a-type / FINAL-override / alias-target / reserved-name errors (${layout} layout-positioning warnings, non-fatal)"
    exit 0
fi
echo "QMLLINT FAIL: $fatal file(s) would fail to load"
exit 1
