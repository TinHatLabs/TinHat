#!/bin/bash
# qml-errors-check.sh BINARY [SECONDS] — "0 QML errors at an empty-profile start" (12.1.16 r6). Starts BINARY on a
# brand-new profile (its own HOME / XDG dirs, deleted afterwards) under Xvfb, lets it sit at the sign-in screen for
# SECONDS (default 25), then stops it and scans its output AND its log file for QML errors: a line naming a
# `<file>.qml:<line>` with a script error or a failed write (TypeError, ReferenceError, JSON.parse SyntaxError, "Unable
# to assign [undefined] to bool", …). Binding-loop and other QML warnings are listed but don't fail.
# Exit 0 = none; 1 = QML errors (listed); 2 = the check itself couldn't run (no binary, no Xvfb, the app never
# started): never "clean" by accident. Positive control: QML_ERRORS_PLANT=1 adds one deliberate error line to the
# scanned text, which must make the check fail.
set -u
B=${1:?usage: qml-errors-check.sh BINARY [SECONDS]}
S=${2:-25}
[ -x "$B" ] || { echo "qml-errors-check: no executable $B" >&2; exit 2; }
command -v xvfb-run >/dev/null || { echo "qml-errors-check: xvfb-run missing" >&2; exit 2; }
H=$(mktemp -d /tmp/qml-errors-check.XXXXXX)
trap 'rm -rf "$H"' EXIT
mkdir -p "$H/run" && chmod 700 "$H/run"
HOME=$H XDG_CONFIG_HOME=$H/.config XDG_DATA_HOME=$H/.local/share XDG_STATE_HOME=$H/.local/state \
  XDG_CACHE_HOME=$H/.cache XDG_RUNTIME_DIR=$H/run TINHAT_NO_SINGLE_INSTANCE=1 QT_QPA_PLATFORM=xcb \
  timeout -s INT -k 10 "$S" xvfb-run -a -s "-screen 0 1600x1000x24" "$B" > "$H/out.log" 2>&1
# the app must have started (its own log, or output) — an empty run proves nothing
logs=$(find "$H" -type f -name '*.log' ! -name out.log ! -name all.log 2>/dev/null)
cat "$H/out.log" $logs > "$H/all.log" 2>/dev/null
[ -s "$H/all.log" ] || { echo "qml-errors-check: the app wrote nothing (did it start?)" >&2; exit 2; }
[ "${QML_ERRORS_PLANT:-0}" = 1 ] && echo "qrc:/qt/qml/dev/tinhat/app/qml/Plant.qml:1: TypeError: planted" >> "$H/all.log"
# ERRORS (fail): script errors and failed property writes at a .qml line. Other .qml lines (binding loops, Shortcut
# notices) are WARNINGS: listed, not failed (several predate 12.1.16).
ERR='(TypeError|ReferenceError|SyntaxError|RangeError|URIError|EvalError|Unable to assign|Cannot assign|is not a function|is not defined|is not a type|Type .* unavailable)'
qml=$(grep -E '\.qml:[0-9]+(:[0-9]+)?:? ' "$H/all.log" | sed -E 's/^.*(qrc:|file:)/\1/')
errs=$(printf '%s\n' "$qml" | grep -E "$ERR" | sort | uniq -c | sort -rn)
warns=$(printf '%s\n' "$qml" | grep -vE "$ERR" | grep -v '^$' | sort | uniq -c | sort -rn)
lines=$(wc -l < "$H/all.log")
[ -n "$warns" ] && { echo "qml-errors-check: QML warnings (not failed):"; echo "$warns"; }
if [ -n "$errs" ]; then
  echo "qml-errors-check: QML ERRORS at an empty-profile start ($lines log lines scanned):"
  echo "$errs"
  exit 1
fi
echo "qml-errors-check: OK, 0 QML errors at an empty-profile start ($lines log lines scanned, ${S}s)"
