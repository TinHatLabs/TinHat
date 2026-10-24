#!/usr/bin/env python3
"""theme-lint.py — static checks of the theme runtime (docs/MIGRATING-THEME.md). Seconds; run it before a build.

  1. qml/Th.qml is the generator's output and committed: build.rs rewrites it whenever crates/theme/tokens.json (or
     the built-in OLED) changed — an uncommitted Th.qml means a token change that nobody reviewed.
  2. Every `Th.<name>` any QML file reads exists in Th.qml (a typo is a runtime `undefined` → an invisible element).
  3. The MIGRATED files (below; an area agent adds its files when its area is done) read no DEPRECATED name — the
     DEPRECATED-NAMES list at the end of Th.qml: `pal.*`, `accentColor`, `panelHover`, … — and write no colour
     literal (`#rrggbb`, Qt.rgba/hsla/lighter/darker/tint/alpha over literals), except the audit's content-keep
     sites (person/data colours: matched by their line text in scripts/theme-lint-keep.json) and lines marked
     `// theme-lint: ok <why>`. `property var pal: null` (the ignored, still-passed DEPRECATED property) is fine.
  4. Every switch is a ThSwitch (fix/basic-switch-on, 2026-09-28): no QML file but ThSwitch.qml instantiates a
     stock `Switch` / `SwitchDelegate` (any import alias). The Basic style (Windows, the Linux rig) paints their CHECKED
     track in palette.dark = surface.window, so an ON switch read as OFF (PARITY P44); ThSwitch paints it
     toggle.track.checked.
  5. Every slider is a ThSlider (fix/sliders-volume, 2026-09-28): no QML file but ThSlider.qml instantiates a stock
     `Slider` / `RangeSlider` (any import alias). The Basic style paints their filled part in palette.dark =
     surface.window (no slider showed its value, PARITY P44) and their groove in palette.midlight (the OS palette on
     Windows); ThSlider paints slider.fill / slider.track / slider.handle.* (+ call.slider.thumb in the call menus).

Exit 0 = clean. Usage: scripts/theme-lint.py [--no-git]
"""
import glob
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
QML = os.path.join(ROOT, "crates/app/qml")
TH = os.path.join(QML, "Th.qml")
KEEP = os.path.join(ROOT, "scripts/theme-lint-keep.json")

# Files whose every colour comes from Th (controls area, desktop C1). Area agents: append yours when done.
MIGRATED = [
    "Avatar.qml", "ConfirmDialog.qml", "CornerMask.qml", "EmptyState.qml", "FallbackTheme.qml", "FoxScrollBar.qml",
    "GlowButton.qml", "HeaderIconButton.qml", "HintLabel.qml", "ThButton.qml", "ThCheckBox.qml",
    "ThComboBox.qml", "ThIcon.qml", "ThRadioButton.qml", "ThTextArea.qml", "ThTextField.qml",
    "ThToolButton.qml", "MenuSectionLabel.qml", "RowLayoutShim.qml", "SectionLabel.qml", "UnreadBadge.qml",
    "ThSwitch.qml",   # fix/basic-switch-on (2026-09-28): the house switch
    "ThSlider.qml", "ThMenu.qml",   # fix/sliders-volume (2026-09-28): the house slider; the menu as wide as its rows
    # desktop C2 — shell
    "LoginField.qml", "LoginView.qml", "ThDock.qml", "ThRoomList.qml", "ThSpaceRail.qml", "PresenceLine.qml",
    "QuickSwitcher.qml", "SpaceInviteCard.qml", "StartupWindow.qml", "StatusChip.qml", "TinHatLoader.qml", "Tour.qml",
    # desktop C2 — chat
    "CalendarCard.qml", "ChatBubble.qml", "ConversationView.qml", "EmojiPicker.qml", "FloatingChats.qml", "Lightbox.qml",
    "LinkPreviewCard.qml", "ThEmbedCard.qml", "RoomHeaderBar.qml",
    # desktop C2 — settings
    "BugReportDialog.qml", "RightPanel.qml", "RoleBadge.qml", "SpaceSettingsWindow.qml", "VerifyDeviceDialog.qml",
    # desktop C2 — media
    "CalendarWindow.qml", "GameNightAdmin.qml", "GameReelPanel.qml", "GameReelWindow.qml", "IconGalleryWindow.qml",
    "MemoriesWindow.qml", "MyGamesEditor.qml", "ReelButton.qml", "ReelCover.qml", "TierList.qml", "WatchPartyWindow.qml",
    # desktop C2 — call (+ Main.qml: every area of it is done; its DEPRECATED shim block is marked for C3)
    "CallOptionsPopover.qml", "EcOutputsPopup.qml", "IncomingCallToast.qml", "Main.qml", "OverlayWindow.qml",
    "ParticipantVolumeMenu.qml", "PipWindow.qml", "SoundboardPopover.qml", "StreamPopover.qml",
    # themes step 1b (feat/modular-themes): the Appearance pane, the gallery, the cards, the import dialog
    "AppearancePane.qml", "ThemeCard.qml", "ThemeDropOverlay.qml", "ThemeGalleryPage.qml",
    "ThemeGrid.qml", "ThemeImportDialog.qml", "ThemeNotices.qml", "ThemeSection.qml", "ThemeSummary.qml", "ThemeTile.qml",
    # the ☰ Theme menu's recents (feat/modular-thememenu, THEMES-UI §12): its swatch paints the ROW theme's own colours
    "ThemeSwatch.qml",
    # 11.0 buttons (feat/modular-buttons): action buttons, rows, the row editor, button islands, the Buttons and
    # Shortcuts settings, the runtime
    "ActionButton.qml", "ActionsHost.qml", "ButtonRow.qml", "ButtonsIsland.qml", "ButtonsSettings.qml", "LayoutStore.qml",
    "RowEditor.qml", "RowModel.qml", "ShortcutsPane.qml",
    # the Settings window (feat/settings-window, contract SETTINGS-WINDOW.md): the window, its pieces, the meter
    "SettingsWindow.qml", "SettingsPage.qml", "SettingsSection.qml", "SettingsCard.qml", "SettingsRow.qml",
    "SettingsRowGroup.qml", "LevelMeter.qml", "LayoutSection.qml",
]
# FallbackTheme IS the shim (it may forward DEPRECATED names); it still may not hold a literal.
SHIM_FILES = {"FallbackTheme.qml"}

# 4. a stock Switch / SwitchDelegate instantiated (`Controls.Switch {`, `QQC2.SwitchDelegate {`, a bare `Switch {`) —
# not ThSwitch, not Kirigami's FormSwitchDelegate, not a JS `switch (x) {`
STOCK_SWITCH = re.compile(r"(?:^|[\s{;:])(?:[A-Za-z_]\w*\.)?(Switch(?:Delegate)?)\s*\{")
SWITCH_HOME = "ThSwitch.qml"
# 5. a stock Slider / RangeSlider instantiated (`Controls.Slider {`, `QQC2.RangeSlider {`, a bare `Slider {`) — not
# ThSlider, not ThemeEditorSimple's `component SliderRow:`, not `Accessible.Slider`, not `Controls.Slider.SnapAlways`
STOCK_SLIDER = re.compile(r"(?:^|[\s{;:])(?:[A-Za-z_]\w*\.)?((?:Range)?Slider)\s*\{")
SLIDER_HOME = "ThSlider.qml"

# A colour made in place: a hex string, Qt.rgba/hsla/hsva/color(…), or Qt.lighter/darker/tint/alpha over anything but a
# Th token (`Qt.alpha(Th.scrollbarThumb, sb.idleAlpha)` = the token with a per-instance alpha is fine).
LITERAL = re.compile(r"""["']#[0-9a-fA-F]{3,8}["']|Qt\.(?:rgba|hsla|hsva|color)\s*\(|Qt\.(?:lighter|darker|tint|alpha)\s*\((?!\s*Th\.)""")


OLD_NAMES = ["pal.bg", "pal.panel", "pal.panelHi", "pal.text", "pal.textDim", "pal.sep", "pal.accent",
             "pal.accentBright", "pal.danger", "pal.dangerBright", "pal.warning", "pal.positive", "pal.onAccent", "pal.info",
             "pal.aside", "pal.name", "accentColor", "accentBrightColor", "panelColor", "accentR", "accentG", "accentB",
             "accentDim", "accentGlow", "panelHover", "linkColor"]


def th_members():
    names = set()
    text = open(TH, encoding="utf-8").read()
    names |= set(re.findall(r"^\s*property \w+ (\w+):", text, re.M))
    names |= set(re.findall(r"^\s*function (\w+)\(", text, re.M))
    dep = re.search(r"// DEPRECATED-NAMES:(.*)", text)
    # After desktop C3 Th.qml has no DEPRECATED block; the old names stay forbidden so they cannot come back.
    deprecated = dep.group(1).split() if dep else [f"{k}=removed" for k in OLD_NAMES]
    return names, text, deprecated


def content_lines():
    """file basename → set of stripped line texts that are content-keep sites (person/data colours that stay literal):
    scripts/theme-lint-keep.json, the list the theme audit produced. A new site: add its line there, or mark the
    line `// theme-lint: ok <why>`."""
    with open(KEEP, encoding="utf-8") as fh:
        return {f: set(texts) for f, texts in json.load(fh).items()}


def main():
    fails = []
    names, text, deprecated = th_members()
    if "GENERATED by crates/app/build.rs" not in text:
        fails.append("Th.qml: not the generator's output (no GENERATED header)")
    if "--no-git" not in sys.argv:
        st = subprocess.run(["git", "-C", ROOT, "status", "--porcelain", "--", "crates/app/qml/Th.qml"],
                            capture_output=True, text=True).stdout.strip()
        if st:
            fails.append("Th.qml: regenerated by the build but not committed (" + st + ") — review the token change "
                         "and commit it with the change that caused it")
    # 2. every Th.x exists
    for path in sorted(glob.glob(os.path.join(QML, "**", "*.qml"), recursive=True)):
        base = os.path.basename(path)
        if base == "Th.qml":
            continue
        for n, line in enumerate(open(path, encoding="utf-8"), 1):
            code = re.sub(r"//.*$", "", line)
            for m in re.finditer(r"\bTh\.(\w+)", code):
                if m.group(1) not in names:
                    fails.append(f"{base}:{n}: Th.{m.group(1)} does not exist (qml/Th.qml) — a token id typo?")
    # 4. every switch is a ThSwitch
    for path in sorted(glob.glob(os.path.join(QML, "**", "*.qml"), recursive=True)):
        base = os.path.basename(path)
        if base == SWITCH_HOME:
            continue
        for n, line in enumerate(open(path, encoding="utf-8"), 1):
            if "theme-lint: ok" in line:
                continue
            m = STOCK_SWITCH.search(re.sub(r"//.*$", "", line))
            if m:
                fails.append(f"{base}:{n}: a stock {m.group(1)} — use ThSwitch (Basic paints a stock switch's ON "
                             f"track in palette.dark = the window colour: it reads as OFF, PARITY P44)")
    # 5. every slider is a ThSlider
    for path in sorted(glob.glob(os.path.join(QML, "**", "*.qml"), recursive=True)):
        base = os.path.basename(path)
        if base == SLIDER_HOME:
            continue
        for n, line in enumerate(open(path, encoding="utf-8"), 1):
            if "theme-lint: ok" in line:
                continue
            m = STOCK_SLIDER.search(re.sub(r"//.*$", "", line))
            if m:
                fails.append(f"{base}:{n}: a stock {m.group(1)} — use ThSlider (Basic paints a stock slider's fill in "
                             f"palette.dark = the window colour: it never shows its value, PARITY P44)")
    # 3. migrated files
    keep = content_lines()
    dep_names = sorted({d.split("=")[0] for d in deprecated if not d.startswith("pal.")}, key=len, reverse=True)
    # a DEPRECATED name read as a value; the NAME side of a binding (`Kirigami.Theme.linkColor: Th.textLink`, plumbing
    # that sets another object's property of that name) is not a read
    # (a Th member is never a DEPRECATED read: Th.accentDim / Th.accentGlow are the roles accent.dim / accent.glow)
    dep_re = re.compile(r"(?<![\w.])(?!Th\.)(?:[A-Za-z_]\w*\.)*(?:pal_?\.\w+|pal_?\b(?!\s*:)|(" + "|".join(map(re.escape, dep_names)) + r")\b(?!\s*:(?!:)))")
    for base in MIGRATED:
        path = os.path.join(QML, base)
        if not os.path.exists(path):
            fails.append(f"{base}: listed as migrated but missing")
            continue
        for n, line in enumerate(open(path, encoding="utf-8"), 1):
            if "theme-lint: ok" in line:
                continue
            code = re.sub(r"//.*$", "", line)
            code_nostr = re.sub(r'"[^"]*"', '""', code)
            if not code.strip():
                continue
            if LITERAL.search(code) and line.strip() not in keep.get(base, set()):
                fails.append(f"{base}:{n}: colour literal in a migrated file: {line.strip()[:110]}")
            if base in SHIM_FILES:
                continue
            if re.match(r"\s*property var pal: null\b", code):
                continue
            m = dep_re.search(code_nostr)
            if m:
                fails.append(f"{base}:{n}: DEPRECATED name `{m.group(0)}` in a migrated file: {line.strip()[:110]}")
    for f in fails:
        print("THEME-LINT FAIL", f)
    if fails:
        print(f"THEME-LINT: {len(fails)} problem(s)")
        return 1
    print(f"THEME-LINT PASS: Th.qml generated + committed; every Th.* exists ({len(names)} members); "
          f"{len(MIGRATED)} migrated files free of literals and DEPRECATED names; every switch a ThSwitch; "
          f"every slider a ThSlider")
    return 0


if __name__ == "__main__":
    sys.exit(main())
