#!/usr/bin/env python3
"""actions-coverage.py — the desktop's id → command map covers the actions table exactly (ACTIONS.md §14.1; the web
checks src/features/actions/exec.ts the same way in exec.test.ts): every placeable action whose apps include "desktop"
(not pending) has one `case "<id>":` in ActionsHost.qml's exec(), and every case names such an action.

Exit 0 = covered. Usage: scripts/actions-coverage.py
"""
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
table = json.load(open(os.path.join(ROOT, "crates/actions/actions.json"), encoding="utf-8"))
qml = open(os.path.join(ROOT, "crates/app/qml/ActionsHost.qml"), encoding="utf-8").read()
body = qml[qml.index("function exec(id, ctx)"):]
cases = re.findall(r'^\s*case "([a-z][a-zA-Z0-9.]*)":', body, re.M)
want = sorted(a["id"] for a in table["actions"] if "desktop" in a["apps"] and not a.get("contexts") and "pending" not in a)
fails = 0


def check(name, ok, detail=""):
    global fails
    print(("ok   " if ok else "FAIL ") + name + ("" if ok or not detail else " — " + detail))
    if not ok:
        fails += 1


dupes = sorted({c for c in cases if cases.count(c) > 1})
check("no case twice", not dupes, ", ".join(dupes))
missing = [i for i in want if i not in cases]
check("every placeable desktop action has a command", not missing, ", ".join(missing))
extra = [c for c in cases if c not in want]
check("every command is a placeable desktop action", not extra, ", ".join(extra))
check(f"{len(set(cases))} commands for {len(want)} actions", len(set(cases)) == len(want))
sys.exit(1 if fails else 0)
