#!/usr/bin/env python3
"""Gate: every controller call in QML must resolve to a real Rust invokable, in scope.

Why this exists: on 2026-08-12 fourteen shipped features were dead in the running app
while `cargo build`, qml-lint, qml-smoke and every Rust behavioural harness passed. The
cause was `#[cxx_name = "..."]` written *without* `#[qinvokable]` — which compiles,
links, and is simply invisible to QML. The first such call sat at the top of
Component.onCompleted; a TypeError there aborts the whole handler, so `login.restore()`
never ran and every user was signed out on each launch.

Nothing in the old gate suite crossed the QML->Rust boundary — the harnesses call Rust
directly. This crosses it statically, and checks two failure modes:

  MISSING   qml calls ctl.foo(), Rust has no #[qinvokable] foo
  OUTSCOPE  qml calls ctl.foo() in a document where `ctl` was never declared. QML ids
            are scoped to their own document, so this is a ReferenceError at runtime
            even though the method exists.
"""
import re
import sys
import glob
import os

# The repo this script lives in (worktrees gate their own tree), or QML_BOUNDARY_ROOT.
ROOT = os.environ.get("QML_BOUNDARY_ROOT") or os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "crates/app/src")
QML = os.path.join(ROOT, "crates/app/qml")


def qml_files():
    """Every QML document in the tree."""
    return glob.glob(os.path.join(QML, "**", "*.qml"), recursive=True)


# Some documents reach the controllers through the shell object they were handed
# (`core.callctl`, `Demo.core.room`, `sh.settings`, `app.calCtl`). Those are real uses of the real
# type, so strip the hop and let the MISSING check see them.
INDIRECT = re.compile(r"\b(?:Demo\.core|Demo\.shell|core|shell|sh|app|root)\.([a-z]\w*)\.(\w+)")


def snake_to_camel(name):
    head, *rest = name.split("_")
    return head + "".join(w[:1].upper() + w[1:] for w in rest)


def parse_rust():
    """QObject type name -> set of names reachable from QML."""
    types, last = {}, None
    for path in sorted(glob.glob(os.path.join(SRC, "*.rs"))):
        text = open(path, encoding="utf-8").read()
        for block in re.split(r'extern\s+"RustQt"', text)[1:]:
            decl = re.search(r'type\s+(\w+)\s*=\s*super::', block)
            if decl:
                last = decl.group(1)
            if last is None:
                continue
            names = types.setdefault(last, set())

            # A qproperty gives QML a reader and a setX writer.
            for prop in re.findall(r'#\[qproperty\([^,]+,\s*(\w+)', block):
                camel = snake_to_camel(prop)
                names.add(camel)
                names.add("set" + camel[:1].upper() + camel[1:])

            attrs = []
            for line in block.splitlines():
                s = line.strip()
                if s.startswith("#["):
                    attrs.append(s)
                    continue
                m = re.match(r'(?:pub )?(?:unsafe )?fn (\w+)', s)
                if m:
                    joined = " ".join(attrs)
                    if "qinvokable" in joined or "qsignal" in joined:
                        cxx = re.search(r'cxx_name = "(\w+)"', joined)
                        names.add(cxx.group(1) if cxx else snake_to_camel(m.group(1)))
                    attrs = []
                    continue
                if s and not s.startswith("//"):
                    attrs = []
    return types


CTRL_DECL = re.compile(r'\b([A-Z]\w*Controller)\s*\{\s*id:\s*(\w+)')


def check_duplicated_contracts():
    """The timeline stick-to-bottom contract exists in TWO files.

    Main.qml carries the main window's timeline and ConversationView.qml the pop-out one.
    On 2026-08-12 a scroll fix was applied to ConversationView only, so the main
    window — the one people actually use — stayed broken and shipped as fixed.
    Any file declaring `stickBottom` must carry the whole contract.
    """
    bad = []
    for path in sorted(qml_files()):
        text = open(path, encoding="utf-8").read()
        if "property bool stickBottom" not in text:
            continue
        base = os.path.relpath(path, QML)
        for needle, why in (
            ("function calcNearBottom", "no calcNearBottom() — bottom test not shared"),
            # 0.8.21: Main.qml guards the recompute with a post-room-switch
            # settle window; the recompute-on-every-scroll contract still holds outside it.
            (("onContentYChanged: stickBottom = calcNearBottom()",
              "onContentYChanged: if (!settling) stickBottom = calcNearBottom()"),
             "stickBottom not recomputed on every scroll (the 2026-08-12 regression)"),
        ):
            alts = needle if isinstance(needle, tuple) else (needle,)
            if not any(n in text for n in alts):
                bad.append((base, why))
    return bad


def main():
    types = parse_rust()

    # Where each controller id is declared, and what type it is.
    declared, owner = {}, {}
    for path in qml_files():
        for t, i in CTRL_DECL.findall(open(path, encoding="utf-8").read()):
            declared.setdefault(os.path.relpath(path, QML), {})[i] = t
            owner[i] = t
    if not owner:
        print("BOUNDARY FAIL: no controller instantiations found — parser is broken")
        return 1

    missing, outscope = [], []
    for path in sorted(qml_files()):
        base = os.path.relpath(path, QML)
        text = open(path, encoding="utf-8").read()
        local = dict(declared.get(base, {}))
        # 9.0 hoist: a shell re-exposes AppCore's controllers as `readonly property var X: core.c_X`
        # — treat those exactly like a local declaration so the MISSING check still covers it.
        for i in re.findall(r'readonly property var (\w+): core\.c_\1\b', text):
            if i in owner:
                local[i] = owner[i]
        # Ids/properties legitimately in scope in this document.
        own = set(local) | set(re.findall(r'^\s*id:\s*(\w+)', text, re.M))
        own |= set(re.findall(r'property\s+\w+\s+(\w+)', text))
        own |= set(re.findall(r'required property\s+\w+\s+(\w+)', text))
        # A JS local or parameter of the same name shadows the controller id, exactly as a
        # property does: `rooms.some(function (room) { return room.title … })` is not the
        # RoomController. Without this, every such delegate reads as out-of-scope.
        own |= set(re.findall(r'\b(?:var|let|const)\s+(\w+)', text))
        for params in re.findall(r'function\s*\w*\s*\(([^)]*)\)', text):
            own |= {p.strip() for p in params.split(',') if p.strip()}
        for params in re.findall(r'\(([^)]*)\)\s*=>', text):
            own |= {p.strip() for p in params.split(',') if p.strip()}

        for n, line in enumerate(text.splitlines(), 1):
            line = re.sub(r'//.*$', '', line)
            # A string literal is data, not a call: "room.joined" (an actions-table key) is not RoomController.joined.
            # (Stripping can only remove findings, never hide a real call written as code.)
            line = re.sub(r'"(?:[^"\\]|\\.)*"', '""', line)
            line = re.sub(r"'(?:[^'\\]|\\.)*'", "''", line)
            if not line.strip():
                continue
            # Documents that reach the controllers through the shell they were handed rather than declaring
            # them (`core.callctl.x`, `Demo.core.room.y`, `sh.settings.z`). Those are real calls on
            # the real type, so check them even though the id is declared in another document.
            for hop in INDIRECT.finditer(line):
                ident, method = hop.group(1), hop.group(2)
                if ident not in owner:
                    continue
                known = types.get(owner[ident], set())
                if known and method not in known:
                    missing.append((base, n, ident, owner[ident], method))
            # Checked above with the type in hand — take the hop out of the line so the ordinary
            # id-scope scan below does not report the same occurrence as out-of-scope.
            line = INDIRECT.sub(lambda m: "_." + m.group(2), line)
            for mt in re.finditer(r'\b(\w+)\.(\w+)', line):
                ident, method = mt.group(1), mt.group(2)
                # `settings.foo: bar` declares a GROUPED PROPERTY on the enclosing type
                # (WebEngineView.settings), it does not read our controller.
                if re.match(r'\s*:(?!:)', line[mt.end():]):
                    continue
                if ident not in owner:
                    continue
                if ident in local:
                    known = types.get(local[ident], set())
                    if known and method not in known:
                        missing.append((base, n, ident, local[ident], method))
                elif ident not in own:
                    outscope.append((base, n, ident, method))

    for f, n, i, t, m in missing:
        print(f"MISSING  {f}:{n}  {i}.{m}  -> {t} has no #[qinvokable]/#[qproperty] {m}")
    contracts = check_duplicated_contracts()
    for f, why in contracts:
        print(f"CONTRACT {f}  {why}")
    for f, n, i, m in outscope:
        print(f"OUTSCOPE {f}:{n}  {i}.{m}  -> `{i}` is declared in another document")

    if missing or outscope or contracts:
        print(f"\nBOUNDARY FAIL: {len(missing)} missing, {len(outscope)} out-of-scope, "
              f"{len(contracts)} broken contract(s).")
        print("Missing is usually #[cxx_name] written without #[qinvokable].")
        return 1
    print(f"BOUNDARY PASS: all QML calls across {len(owner)} controllers resolve, in scope; "
          "duplicated timeline contracts agree")
    return 0


if __name__ == "__main__":
    sys.exit(main())
