#!/usr/bin/env python3
"""qml-textformat-lint.py — security review 2026-10-26 (M7, H2): text from other people never renders as markup.

Qt's Text / Label default to Text.AutoText: a string that looks like markup is drawn as StyledText, so a display name,
room name, topic, poll question, file name, game name … with `<img src=https://…>` in it made every reader's client
fetch that image (their IP, also in encrypted rooms) and `<font>` / `<b>` could restyle the UI. This lint fails on:

  1. a label (Text, Controls.Label, Kirigami.Heading, … and our own components whose root is one, unless that root
     declares a textFormat) whose `text:` is not a plain literal and that does not declare `textFormat:` — bound text
     must say what it is: Text.PlainText (anything from a model, a member, a server), or StyledText / RichText for markup
     the app builds itself with every inserted string escaped;
  2. a style-drawn control (Controls.Button, MenuItem, CheckBox, …: the style's own label is AutoText) whose `text:` is
     not a plain literal and not wrapped in `LinkGate.plain(…)` (which makes a string unable to read as markup);
  3. `Qt.openUrlExternally` anywhere but LinkGate.qml (every link / file the OS opens goes through its allow-list);
  4. a `ToolTip { … }` element (the shared attached tooltip is made PlainText at start by AppCore._plainTooltips; an
     element of its own is not);
  5. a toast (`showPassiveNotification(…)`: Kirigami draws it AutoText) whose message is not a literal and not wrapped in
     LinkGate.plain(…), and a dialog title (Controls.Dialog, Kirigami.Dialog / PromptDialog …) likewise. Kirigami's InlineMessage / PlaceholderMessage / Action count as style-drawn controls (2).

Files of other lanes that are not migrated yet are listed in PENDING: reported, not fatal.
Usage: python3 scripts/qml-textformat-lint.py [qml-dir]   (scripts/qml-lint.sh runs it)
"""
import os
import re
import sys

LABEL_TYPES = {"Text", "Label", "Controls.Label", "QQC2.Label", "T.Label", "Kirigami.Heading", "Kirigami.Label", "Kirigami.Title"}
STYLED_CONTROLS = {
    "Controls.Button", "Controls.ToolButton", "Controls.MenuItem", "Controls.ItemDelegate", "Controls.CheckBox",
    "Controls.RadioButton", "Controls.Switch", "Controls.TabButton", "Controls.CheckDelegate", "Controls.RadioDelegate",
    "Controls.SwitchDelegate", "Controls.MenuBarItem", "Controls.RoundButton", "Controls.DelayButton",
    "Controls.SwipeDelegate", "Controls.Action", "Kirigami.Chip", "Kirigami.LinkButton", "Kirigami.UrlButton",
    "Kirigami.Action", "Kirigami.InlineMessage", "Kirigami.PlaceholderMessage",
}
# a dialog's title is drawn by the style's / Kirigami's own header label (AutoText)
DIALOGS = {"Controls.Dialog", "Kirigami.Dialog", "Kirigami.PromptDialog", "Kirigami.MenuDialog", "Kirigami.OverlaySheet"}
# sec/trust-1026 owns these files (Media Party, soundboard): reported until that lane migrates them
PENDING = set()   # 2026-10-26: WatchPartyWindow.qml and SoundboardPopover.qml migrated at the sec/desktop merge
SAFE_IDENTS = {"qsTr", "qsTrId", "true", "false", "null", "undefined"}


def mask(src):
    """The source with comments and the insides of string / template / regex literals blanked (same length, same
    newlines), so braces and keywords in them don't count."""
    out = list(src)
    i, n = 0, len(src)
    prev_sig = "\n"  # last significant (non-space) character, for telling a regex from a division

    def blank(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = src[i]
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
            continue
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            j = src.find("*/", i + 2)
            j = n if j < 0 else j + 2
            blank(i, j)
            i = j
            continue
        if c in "\"'`":
            j = i + 1
            while j < n and src[j] != c:
                if src[j] == "\\":
                    j += 1
                elif src[j] == "\n" and c != "`":
                    break
                j += 1
            blank(i + 1, min(j, n))
            i = j + 1
            prev_sig = c
            continue
        if c == "/" and prev_sig in "(,=:[!&|?{};+-*%<>~^\n":
            # a regex literal
            j = i + 1
            in_class = False
            while j < n and src[j] != "\n":
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == "[":
                    in_class = True
                elif src[j] == "]":
                    in_class = False
                elif src[j] == "/" and not in_class:
                    break
                j += 1
            if j < n and src[j] == "/":
                blank(i + 1, j)
                i = j + 1
                prev_sig = "/"
                continue
        if not c.isspace():
            prev_sig = c
        i += 1
    return "".join(out)


ELEM_RE = re.compile(r"(?<![\w.])([A-Z]\w*(?:\.[A-Z]\w*)?)\s*\{")


def elements(masked):
    """(type, open brace index, close brace index) of every `Type {` block."""
    res = []
    for m in ELEM_RE.finditer(masked):
        start = m.end() - 1
        depth = 0
        for k in range(start, len(masked)):
            ch = masked[k]
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    res.append((m.group(1), start, k, m.start()))
                    break
    return res


def own_props(masked, a, b):
    """{name: (value start, value end)} of the block's own `name: value` properties (not of nested blocks)."""
    props = {}
    depth = 0
    k = a + 1
    stmt_start = True
    while k < b:
        ch = masked[k]
        if ch in "{([":
            depth += 1
        elif ch in "})]":
            depth -= 1
        elif depth == 0 and ch in ";\n":
            stmt_start = True
            k += 1
            continue
        if depth == 0 and stmt_start and not ch.isspace():
            stmt_start = False
            m = re.match(r"([A-Za-z_][\w.]*)\s*:(?!:)", masked[k:b])
            if m and not re.match(r"(property|signal|function|readonly|required|default)\b", masked[k:b]):
                name = m.group(1)
                vs = k + m.end()
                ve = value_end(masked, vs, b)
                props[name] = (vs, ve)
                k = ve
                stmt_start = True
                continue
        k += 1
    return props


def value_end(masked, vs, b):
    depth = 0
    k = vs
    while k < b:
        ch = masked[k]
        if ch in "{([":
            depth += 1
        elif ch in "})]":
            if depth == 0:
                return k
            depth -= 1
        elif depth == 0 and ch == ";":
            return k
        elif depth == 0 and ch == "\n":
            before = masked[vs:k].rstrip()
            after = masked[k:b].lstrip()
            if before.endswith(("+", "?", ":", "&&", "||", ",", "(", "-", "*", "/", "=")) or after.startswith(("+", "?", ":", ".", "&&", "||", "-")):
                k += 1
                continue
            return k
        k += 1
    return b


def arg_end(masked, a):
    """End of the call argument that starts at `a` (the first `,` or `)` at depth 0)."""
    depth = 0
    for k in range(a, len(masked)):
        ch = masked[k]
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            if depth == 0:
                return k
            depth -= 1
        elif ch == "," and depth == 0:
            return k
    return len(masked)


def is_static(masked_value):
    """A plain literal: no names left once the string contents are blanked (qsTr and true/false allowed); a ternary is
    static when both its branches are (`muted ? "Unmute" : "Mute"` — the condition picks between the app's words)."""
    v = masked_value.strip()
    while v.startswith("(") and closing(v, 0) == len(v) - 1:
        v = v[1:-1].strip()
    q = top_level(v, "?")
    if q >= 0:
        c = top_level(v, ":", q + 1, ternary=True)
        if c >= 0:
            return is_static(v[q + 1:c]) and is_static(v[c + 1:])
    names = set(re.findall(r"[A-Za-z_$][\w$]*", v)) - SAFE_IDENTS
    return not names


def closing(v, i):
    depth = 0
    for k in range(i, len(v)):
        if v[k] in "([{":
            depth += 1
        elif v[k] in ")]}":
            depth -= 1
            if depth == 0:
                return k
    return -1


def top_level(v, ch, start=0, ternary=False):
    """Index of `ch` at bracket depth 0 (for ':' of a ternary: the one that closes it, nested ternaries skipped)."""
    depth, inner = 0, 0
    for k in range(start, len(v)):
        x = v[k]
        if x in "([{":
            depth += 1
        elif x in ")]}":
            depth -= 1
        elif depth == 0:
            if ternary and x == "?":
                inner += 1
            elif x == ch:
                if ternary and inner > 0:
                    inner -= 1
                    continue
                return k
    return -1


def label_components(qml_dir):
    """Our own components whose root is a label: {name: declares_textFormat}."""
    comps = {}
    for root, _, files in os.walk(qml_dir):
        for f in files:
            if not f.endswith(".qml"):
                continue
            src = open(os.path.join(root, f), encoding="utf-8").read()
            masked = mask(src)
            els = elements(masked)
            if not els:
                continue
            t, a, b, _ = min(els, key=lambda e: e[3])
            if t in LABEL_TYPES:
                comps[f[:-4]] = "textFormat" in own_props(masked, a, b)
    return comps


def lint(qml_dir):
    comps = label_components(qml_dir)
    labelish = set(LABEL_TYPES) | {c for c, has in comps.items() if not has}
    fails, pending = [], []
    for root, _, files in os.walk(qml_dir):
        for f in sorted(files):
            if not f.endswith(".qml"):
                continue
            path = os.path.join(root, f)
            rel = os.path.relpath(path, qml_dir)
            src = open(path, encoding="utf-8").read()
            masked = mask(src)
            out = pending if f in PENDING else fails
            line = lambda pos: src.count("\n", 0, pos) + 1
            for t, a, b, s in elements(masked):
                props = own_props(masked, a, b)
                if t in DIALOGS and "title" in props:
                    vs, ve = props["title"]
                    val = src[vs:ve].strip()
                    if not is_static(masked[vs:ve]) and not val.startswith("LinkGate.plain("):
                        out.append(f"{rel}:{line(vs)}: {t} title bound to `{' '.join(val.split())[:60]}` — its header draws it AutoText: wrap it in LinkGate.plain(…)")
                if "text" not in props:
                    if t.endswith("ToolTip") and f != "AppCore.qml":
                        out.append(f"{rel}:{line(s)}: a {t} element of its own (its label is AutoText) — use the attached Controls.ToolTip.text")
                    continue
                vs, ve = props["text"]
                val, mval = src[vs:ve].strip(), masked[vs:ve]
                short = " ".join(val.split())[:70]
                if is_static(mval):
                    continue
                if t in labelish and "textFormat" not in props:
                    out.append(f"{rel}:{line(vs)}: {t} text bound to `{short}` without textFormat (Text.PlainText for anything not built as escaped markup)")
                elif t in STYLED_CONTROLS and not re.match(r"LinkGate\.plain\(", val):
                    out.append(f"{rel}:{line(vs)}: {t} text bound to `{short}` — the style draws it AutoText: wrap it in LinkGate.plain(…)")
            # a toast (Kirigami's passive notification draws it AutoText): its message is a literal or goes through
            # LinkGate.plain (the helpers that forward a message wrap it once)
            for m in re.finditer(r"(?<!function )\bshowPassiveNotification\s*\(", masked):
                a0 = m.end()
                e0 = arg_end(masked, a0)
                arg, marg = src[a0:e0].strip(), masked[a0:e0]
                if not is_static(marg) and not arg.startswith("LinkGate.plain("):
                    out.append(f"{rel}:{line(a0)}: a toast with `{' '.join(arg.split())[:60]}` — wrap the message in LinkGate.plain(…)")
            if f != "LinkGate.qml":
                for m in re.finditer(r"Qt\.openUrlExternally\s*\(", masked):
                    out.append(f"{rel}:{line(m.start())}: Qt.openUrlExternally outside LinkGate.qml — use LinkGate.open / openMedia / openLocal")
    return fails, pending


def main():
    qml_dir = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "crates", "app", "qml")
    fails, pending = lint(qml_dir)
    for p in pending:
        print(f"textformat-lint: PENDING (sec/trust lane) {p}")
    for x in fails:
        print(f"textformat-lint: {x}")
    if fails:
        print(f"textformat-lint: FAIL — {len(fails)} finding(s)")
        return 1
    print(f"textformat-lint: PASS ({len(pending)} pending in other lanes' files)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
