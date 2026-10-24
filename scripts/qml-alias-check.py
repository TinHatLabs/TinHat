#!/usr/bin/env python3
"""qml-alias-check.py — `property alias NAME: ID.MEMBER` whose ID is an instance of one of OUR components must name a
member that component declares. qmllint does not resolve these (Main.qml aliases ~150 AppCore properties), and a
missing target refuses the whole component at run time: "Invalid alias target location" → no main window at all
(found 2026-09-27: renaming AppCore's _navPartTick left Main.qml's alias behind; the lints passed, the app never showed).

Usage: qml-alias-check.py QML_DIR [FILES...]   (default: every *.qml under QML_DIR). Exit 1 on a problem.
"""
import os
import re
import sys

QML_DIR = sys.argv[1]
FILES = sys.argv[2:] or [os.path.join(dp, f) for dp, _, fs in os.walk(QML_DIR) for f in fs if f.endswith(".qml")]

MEMBER = re.compile(
    r"^\s*(?:(?:readonly|required|default)\s+)*property\s+(?:alias\s+|[\w.<>]+\s+)(\w+)\s*[:;\n]"
    r"|^\s*signal\s+(\w+)|^\s*function\s+(\w+)\s*\(",
    re.M,
)
INSTANCE = re.compile(r"(?:^|[\s{;])([A-Z]\w*)\s*\{\s*(?:\n\s*)?id:\s*(\w+)", re.M)
ALIAS = re.compile(r"^\s*(?:readonly\s+)?property\s+alias\s+(\w+)\s*:\s*(\w+)\.(\w+)\s*$", re.M)

_members = {}
# Item / QtObject members an alias may legitimately point at on one of our components (they are not declared in its file)
QT_ITEM = set(
    "x y z width height implicitWidth implicitHeight visible enabled opacity scale rotation parent children data "
    "anchors state states transitions focus activeFocus objectName clip smooth antialiasing layer baselineOffset "
    "childrenRect transform transformOrigin containmentMask resources visibleChildren".split()
)
# fix/sliders-volume (2026-09-28): a component whose ROOT is a Qt Quick Controls type (ThSlider = `Controls.Slider {`,
# ThMenu = `Controls.Menu {`) inherits that type's members — an alias at one (ThemeEditorSimple's SliderRow: from / to
# / stepSize → its ThSlider) is fine. The Control members + the types our components are rooted at.
QQC2_CONTROL = set(
    "font palette padding topPadding bottomPadding leftPadding rightPadding horizontalPadding verticalPadding spacing "
    "hovered hoverEnabled wheelEnabled focusPolicy focusReason visualFocus availableWidth availableHeight background "
    "contentItem mirrored locale implicitContentWidth implicitContentHeight implicitBackgroundWidth "
    "implicitBackgroundHeight topInset bottomInset leftInset rightInset".split()
)
QQC2_MEMBERS = {
    "Slider": set("from to value stepSize snapMode position visualPosition pressed orientation horizontal vertical live "
                  "touchDragThreshold handle implicitHandleWidth implicitHandleHeight moved increase decrease valueAt".split()),
    "Menu": set("title count currentIndex contentModel contentData delegate cascade overlap icon popup dismiss itemAt "
                "addItem insertItem removeItem takeItem menuAt addMenu insertMenu removeMenu takeMenu actionAt addAction "
                "insertAction removeAction takeAction open close opened modal dim closePolicy margins".split()),
}
ROOT = re.compile(r"^(?:[A-Za-z_]\w*\.)?([A-Z]\w*)\s*\{", re.M)


def members(type_name):
    """The members a component file declares (None when it is not one of ours)."""
    if type_name in _members:
        return _members[type_name]
    path = None
    for dp, _, fs in os.walk(QML_DIR):
        if type_name + ".qml" in fs:
            path = os.path.join(dp, type_name + ".qml")
            break
    if path is None:
        _members[type_name] = None
        return None
    text = open(path, encoding="utf-8").read()
    names = set()
    for m in MEMBER.finditer(text):
        names.add(next(g for g in m.groups() if g))
    root = ROOT.search(text)
    if root and root.group(0).split("{")[0].strip().startswith(("Controls.", "QQC2.", "T.")) and root.group(1) in QQC2_MEMBERS:
        names |= QQC2_CONTROL | QQC2_MEMBERS[root.group(1)]
    # members of the root object's own base types are out of scope (Item.visible …): only flag what looks like ours
    _members[type_name] = names
    return names


problems = []
for f in sorted(FILES):
    text = open(f, encoding="utf-8").read()
    ids = {m.group(2): m.group(1) for m in INSTANCE.finditer(text)}
    for m in ALIAS.finditer(text):
        name, target_id, member = m.groups()
        t = ids.get(target_id)
        if not t:
            continue
        declared = members(t)
        if declared is None or member in declared:
            continue
        if member in QT_ITEM:
            continue  # a base-type property of the target (Item.visible …)
        line = text[: m.start()].count("\n") + 1
        problems.append(f"{os.path.relpath(f, QML_DIR)}:{line}: alias {name} → {target_id}.{member}, but {t}.qml declares no {member}")

for p in problems:
    print(p)
print(f"qml-alias-check: {len(problems)} problem(s) in {len(FILES)} file(s)")
sys.exit(1 if problems else 0)
