#!/usr/bin/env python3
"""qml-final-check.py QML_DIR [FILES…] — find QML property declarations that redeclare a FINAL property of the
object's type (Qt 6.11 refuses such a component: "Cannot override FINAL property"), and functions / signals named
like a signal of the type (incl. <property>Changed: "invalid override" — a call may emit the signal instead). Types: Qt's .qmltypes (prototype
chains, isFinal) + our own components (a file's root type, recursively)."""
import glob, os, re, sys

QMLTYPES = glob.glob("/usr/lib/qt6/qml/**/*.qmltypes", recursive=True)
comps = {}        # C++ class name -> {proto, props:{name: final}}
exports = {}      # short QML name -> [class names]
for f in QMLTYPES:
    txt = open(f, errors="replace").read()
    # split into Component blocks (top level inside Module)
    for m in re.finditer(r"\n    Component \{(.*?)\n    \}", txt, re.S):
        body = m.group(1)
        nm = re.search(r'\n        name: "([^"]+)"', body)
        if not nm:
            continue
        name = nm.group(1)
        proto = re.search(r'\n        prototype: "([^"]+)"', body)
        props = {}
        sigs = set(re.findall(r'Signal \{[^}]*?name: "([^"]+)"', body))
        for pm in re.finditer(r"Property \{([^}]*)\}", body):
            pb = pm.group(1)
            pn = re.search(r'name: "([^"]+)"', pb)
            if pn:
                props[pn.group(1)] = "isFinal: true" in pb
        c = comps.setdefault(name, {"proto": None, "props": {}, "sigs": set()})
        if proto:
            c["proto"] = proto.group(1)
        c["props"].update(props)
        c["sigs"] |= sigs | {p + "Changed" for p in props}
        ex = re.search(r"exports: \[([^\]]*)\]", body)
        if ex:
            for e in re.findall(r'"([^"]+)"', ex.group(1)):
                short = e.split("/")[-1].split(" ")[0]
                exports.setdefault(short, [])
                if name not in exports[short]:
                    exports[short].append(name)

def props_of_class(cls, seen=None):
    out = set()
    seen = seen or set()
    while cls and cls in comps and cls not in seen:
        seen.add(cls)
        out |= set(comps[cls]["props"])
        cls = comps[cls]["proto"]
    return out

def props_of_type(tname, depth=0):
    short = tname.split(".")[-1]
    if depth > 8:
        return set()
    own = os.path.join(qml_dir, short + ".qml")
    if os.path.exists(own) and "." not in tname:
        rt = root_type(own)
        return props_of_type(rt, depth + 1) if rt else set()
    out = set()
    for cls in exports.get(short, []):
        out |= props_of_class(cls)
    return out

def finals_of_class(cls, seen=None):
    out = set()
    seen = seen or set()
    while cls and cls in comps and cls not in seen:
        seen.add(cls)
        out |= {p for p, fin in comps[cls]["props"].items() if fin}
        cls = comps[cls]["proto"]
    return out

def signals_of_class(cls, seen=None):
    out = set()
    seen = seen or set()
    while cls and cls in comps and cls not in seen:
        seen.add(cls)
        out |= comps[cls]["sigs"]
        cls = comps[cls]["proto"]
    return out

qml_dir = sys.argv[1]
own_root = {}
def root_type(path):
    txt = strip(open(path).read())
    m = re.search(r"^\s*([A-Z][\w.]*)\s*\{", txt, re.M)
    return m.group(1) if m else None

def strip(txt):
    out, i, n = [], 0, len(txt)
    while i < n:
        c = txt[i]
        if c in "\"'`":
            q = c; out.append(c); i += 1
            while i < n and txt[i] != q:
                if txt[i] == "\\": out.append(txt[i]); i += 1
                if i < n: out.append(txt[i] if txt[i] == "\n" else " "); i += 1
            if i < n: out.append(q); i += 1
        elif txt.startswith("//", i):
            j = txt.find("\n", i); i = n if j < 0 else j
        elif txt.startswith("/*", i):
            j = txt.find("*/", i + 2); seg = txt[i:(n if j < 0 else j + 2)]
            out.append("\n" * seg.count("\n")); i = n if j < 0 else j + 2
        else:
            out.append(c); i += 1
    return "".join(out)

def finals_of_type(tname, depth=0):
    short = tname.split(".")[-1]
    if depth > 8:
        return set()
    own = os.path.join(qml_dir, short + ".qml")
    if os.path.exists(own) and "." not in tname:
        rt = root_type(own)
        return finals_of_type(rt, depth + 1) if rt else set()
    out = set()
    for cls in exports.get(short, []):
        out |= finals_of_class(cls)
    return out

def signals_of_type(tname, depth=0):
    short = tname.split(".")[-1]
    if depth > 8:
        return set()
    own = os.path.join(qml_dir, short + ".qml")
    if os.path.exists(own) and "." not in tname:
        rt = root_type(own)
        return signals_of_type(rt, depth + 1) if rt else set()
    out = set()
    for cls in exports.get(short, []):
        out |= signals_of_class(cls)
    return out

def own_type(tname):
    short = tname.split(".")[-1]
    return "." not in tname and (os.path.exists(os.path.join(qml_dir, short + ".qml")) or short in inline_components)

POSITIONERS = {"Row", "Column", "Grid", "Flow"}
inline_components = {}   # name -> base type, from `component Name: Base {` in every file
for path in glob.glob(os.path.join(qml_dir, "*.qml")):
    for cm in re.finditer(r"component\s+(\w+)\s*:\s*([A-Z][\w.]*)\s*\{", strip(open(path).read())):
        inline_components.setdefault(cm.group(1), cm.group(2))

def base_positioner(tname, depth=0):
    short = tname.split(".")[-1]
    if depth > 8:
        return False
    if short in POSITIONERS and (tname == short or tname.startswith("QtQuick.")):
        return True
    if short in inline_components:
        return base_positioner(inline_components[short], depth + 1)
    own = os.path.join(qml_dir, short + ".qml")
    if os.path.exists(own) and "." not in tname:
        rt = root_type(own)
        return base_positioner(rt, depth + 1) if rt else False
    return False

files = sys.argv[2:] or sorted(glob.glob(os.path.join(qml_dir, "*.qml")))
bad = 0
for path in files:
    txt = strip(open(path).read())
    stack = []  # (type or None, brace depth)
    lines = txt.split("\n")
    for ln, line in enumerate(lines, 1):
        # property declarations on this line belong to the innermost object on the stack
        for pm in re.finditer(r"(?:^|[;{\s])(?:(?:readonly|required|default)\s+)*property\s+(?:alias|[\w<>.]+)\s+(\w+)", line):
            owner = next((t for t, _ in reversed(stack) if t), None)
            if owner and pm.group(1) in finals_of_type(owner):
                print(f"{os.path.relpath(path)}:{ln}: `{pm.group(1)}` redeclares a FINAL property of {owner}")
                bad += 1
            elif owner and not own_type(owner) and pm.group(1) in props_of_type(owner):
                # a Qt type's own (non-final) property: Qt 6.11 warns "overrides a member of the base object" — the
                # next Qt may refuse it like a FINAL one. (Our own components' properties may be overridden on purpose.)
                print(f"{os.path.relpath(path)}:{ln}: `{pm.group(1)}` overrides a property of {owner} (rename it)")
                bad += 1
        for fm in re.finditer(r"(?:^|[;{\s])(function|signal)\s+(\w+)\s*\(", line):
            owner = next((t for t, _ in reversed(stack) if t), None)
            if owner and fm.group(2) in signals_of_type(owner):
                print(f"{os.path.relpath(path)}:{ln}: {fm.group(1)} `{fm.group(2)}` has the name of a signal of {owner} (invalid override)")
                bad += 1
        # Qt 6.11: a positioner (Row / Column / Grid / Flow) computes its implicit size — assigning it is refused
        # ("Invalid property assignment: … is a read-only property") and takes the component down with it
        for am in re.finditer(r"(?:^\s*|[;{]\s*)(implicitWidth|implicitHeight)\s*:", line):
            owner = next((t for t, _ in reversed(stack) if t), None)
            if owner and base_positioner(owner):
                print(f"{os.path.relpath(path)}:{ln}: `{am.group(1)}` assigned on a positioner ({owner}): read-only in Qt 6.11")
                bad += 1
        # walk the braces of this line: `Type {` opens an object, other `{` open blocks (JS / grouped props)
        for tok in re.finditer(r"([A-Z][\w.]*)\s*\{|\{|\}", line):
            if tok.group(0) == "}":
                if stack: stack.pop()
            elif tok.group(1):
                stack.append((tok.group(1), len(stack)))
            else:
                stack.append((None, len(stack)))
print(f"qml-final-check: {bad} problem(s) in {len(files)} file(s); {len(comps)} Qt types read")
sys.exit(1 if bad else 0)
