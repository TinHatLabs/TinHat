#!/usr/bin/env python3
"""reserved_scan.py QMLDIR — ECMAScript's old "future reserved words" used as JS names in QML (parameters, var/let/
const, function names, property declarations, signal parameters). Qt 6.11's parser accepts them; Qt 6.8's qmlcachegen
(the Windows build) refuses them.
11.0 (S7-J 2026-09-28): `readonly property bool short` in EditLayer.qml / Island.qml was a Qt 6.8 syntax error ("Expected
token `identifier'") this scan did not look for — property declarations and signal parameters are checked now."""
import re, sys, os
WORDS = ["abstract", "boolean", "byte", "char", "double", "final", "float", "goto", "int", "long", "native", "short",
         "synchronized", "throws", "transient", "volatile", "enum", "implements", "interface", "package", "private",
         "protected", "public", "static", "yield", "await"]
W = "|".join(WORDS)
pats = [
    re.compile(r"\bfunction\s+\w*\s*\(([^)]*)\)"),          # parameters of functions
    re.compile(r"\(([^()]*)\)\s*=>"),                          # arrow-function parameters
    re.compile(r"\b(?:var|let|const)\s+(%s)\b" % W),           # declarations
    re.compile(r"\bfunction\s+(%s)\s*\(" % W),                 # function names
    re.compile(r"\bproperty\s+[\w.<>]+\s+(%s)\b" % W),         # property declarations (11.0: `property bool short`)
]
SIGNAL = re.compile(r"\bsignal\s+\w+\s*\(([^)]*)\)")                # signal parameters: `type name` pairs
bad = 0
for root, _, files in os.walk(sys.argv[1]):
    for fn in files:
        if not fn.endswith(".qml") and not fn.endswith(".js"):
            continue
        p = os.path.join(root, fn)
        for i, line in enumerate(open(p, encoding="utf-8"), 1):
            code = re.sub(r"//.*$", "", line)
            code = re.sub(r'"(?:[^"\\]|\\.)*"|\'(?:[^\'\\]|\\.)*\'', '""', code)
            found = []
            for m in SIGNAL.finditer(code):
                found += [a.strip().split()[-1] for a in m.group(1).split(",") if a.strip()]
            for n in found:
                if n in WORDS:
                    bad += 1
                    print(f"{p}:{i}: `{n}`: {line.strip()[:110]}")
            for k, pat in enumerate(pats):
                for m in pat.finditer(code):
                    names = re.split(r"[,\s]+", m.group(1)) if k < 2 else [m.group(1)]
                    for n in names:
                        n = n.split("=")[0].strip()
                        if n in WORDS:
                            bad += 1
                            print(f"{p}:{i}: `{n}`: {line.strip()[:110]}")
print(f"reserved-word names: {bad}")
sys.exit(1 if bad else 0)
