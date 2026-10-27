// ThemeEditorGallery.qml — the theme editor's and the share cards' review harness (11.0 step 5; the web's
// GalleryThemeEditor.tsx + tools/modular/editor-shots.mjs, the same states): a 1280 × 800 window with a stand-in for
// the app behind the panel (the window, a rooms island, the header, two bubbles, the composer — painted by the
// preview), the REAL ThemeEditorPanel / ThemeShareCard / ThemeTryBar, no sign-in. Run (on the build machine):
//   TINHAT_THEME_GALLERY=1 TINHAT_GUI_THEMESYNC=0 QT_QPA_PLATFORM=xcb tinhat --gallery-dir=<out dir> [--gallery-only=id,id]
// Every state is set up in this one process (the editor closed, its popups closed and every preview ended between
// states), shot with QQuickWindow::grabWindow (popups included) into <out dir>/desktop/<state>.png; "GALLERY ok|FAIL
// <state>" lines on stderr, then it quits. "font-probe" (only when asked for) logs what a look does to the fonts.
import QtQuick
import QtQuick.Window
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Controls.ApplicationWindow {
    id: win
    width: 1280; height: 800
    visible: true
    title: "Tin Hat theme editor gallery"
    color: Th.windowBg
    // Main.qml's role layer (P46): the controls' palette and Kirigami's colours from the theme, so the house controls look
    // here exactly as they do in the app (a bare window drew white fields, radios and sliders)
    palette.window: Th.surfaceWindow
    palette.windowText: Th.textPrimary
    palette.base: Th.surfaceIsland
    palette.alternateBase: Th.surfaceRaised
    palette.text: Th.textPrimary
    palette.button: Th.surfaceIsland
    palette.buttonText: Th.textPrimary
    palette.highlight: Th.accentBase
    palette.highlightedText: Th.textOnAccent
    palette.mid: Th.borderSubtle
    palette.light: Th.surfaceRaised
    palette.dark: Th.surfaceWindow
    palette.placeholderText: Th.textSecondary
    palette.link: Th.textLink
    Kirigami.Theme.inherit: false
    Kirigami.Theme.backgroundColor: Th.surfaceWindow
    Kirigami.Theme.textColor: Th.textPrimary
    Kirigami.Theme.disabledTextColor: Th.textSecondary
    Kirigami.Theme.highlightColor: Th.accentBase
    Kirigami.Theme.highlightedTextColor: Th.textOnAccent
    Kirigami.Theme.alternateBackgroundColor: Th.surfaceIsland
    Kirigami.Theme.activeTextColor: Th.accentBright
    Kirigami.Theme.linkColor: Th.textLink
    Kirigami.Theme.positiveTextColor: Th.stateSuccess
    Kirigami.Theme.negativeTextColor: Th.stateDangerBright
    Kirigami.Theme.neutralTextColor: Th.iconSchemeNeutral
    Kirigami.Theme.hoverColor: Th.accentBase
    Kirigami.Theme.focusColor: Th.accentBase
    // Main.qml's look (ENGINE.md §10): the theme's interface font + text size on the window's font
    Binding { target: win; property: "font.family"; value: Th.fontUi; when: Th.fontUi.length > 0; restoreMode: Binding.RestoreValue }
    Binding {
        target: win; property: "font.pointSize"; restoreMode: Binding.RestoreValue
        when: Th.textSize !== 100 && Kirigami.Theme.defaultFont.pointSize > 0
        value: Kirigami.Theme.defaultFont.pointSize * Th.textScale
    }

    ThemeController { id: themeCtlObj }
    ThemeEditorController { id: editorCtlObj }
    BootController { id: bootObj }   // the startup window: finished at once (nothing to load here)
    // what the cards and the panel ask of `app` (Main.qml's forwarders)
    readonly property var themeCtl: themeCtlObj
    readonly property var editorCtl: editorCtlObj
    property var themeTrial: null
    function tryTheme(key, docJson, name) { return false }
    function endThemeTrial(back) { }
    function keepTheme(themeJson) { }
    // the legacy theme values the house components read from `app` (Main.qml has them)
    readonly property real radiusSm: 5
    readonly property real radius: 8
    readonly property int radiusLg: 12
    readonly property real radiusXl: 16
    readonly property int dur: 0

    property string galleryDir: ""
    property var only: []
    /// --gallery-scheme (the Breeze run): the base theme's KDE colour scheme is activated per state, as the app does for
    /// the theme on screen — Breeze's native controls paint from it (without it they were light-scheme white)
    property bool withScheme: false
    property var thColours: ({})

    function paintTheme(id) {
        if (win.withScheme) editorCtlObj.harnessScheme(id)
        var c = themeCtlObj.resolve(id)
        win.thColours = c
        Th.apply(c, themeCtlObj.themeId, themeCtlObj.themeName, themeCtlObj.variant, themeCtlObj.names, themeCtlObj.shape, themeCtlObj.look)
    }
    function paintPreview() {
        var d = editorCtlObj.workingDoc()
        if (d.length === 0) return
        var c = editorCtlObj.previewDoc("editor", d)
        win.thColours = c
        Th.apply(c, editorCtlObj.previewId, editorCtlObj.previewName, editorCtlObj.previewVariant, editorCtlObj.previewNames, editorCtlObj.previewShape, editorCtlObj.previewLook)
    }
    // the font probe (state "font-probe", only on request): what a theme's look does to the fonts on this platform
    Controls.Label { id: probeLabel; visible: false; text: "x" }
    Text { id: probeText; visible: false; text: "x" }
    SectionLabel { id: probeSection; visible: false; text: "x" }
    ThButton { id: probeButton; visible: false; app: win; text: "x" }
    function logFonts(tag) {
        console.error("GALLERY font " + tag + ": look=" + (Th.fontUi || "-") + "/" + Th.textSize + " mono=" + Th.fontMono
                      + " window=" + win.font.family + " " + win.font.pointSize
                      + " kirigami=" + Kirigami.Theme.defaultFont.family + " " + Kirigami.Theme.defaultFont.pointSize + " small=" + Kirigami.Theme.smallFont.pointSize
                      + " label=" + probeLabel.font.family + " " + probeLabel.font.pointSize + " section=" + probeSection.font.family + " " + probeSection.font.pointSize
                      + " button=" + probeButton.font.family + " " + probeButton.font.pointSize + " text=" + probeText.font.family + " " + probeText.font.pointSize)
    }
    Connections {
        target: editorCtlObj
        function onStateRevChanged() { if (editorCtlObj.open) win.paintPreview() }
    }

    // ---- the states (the web's editor-shots.mjs STATES, same ids) ------------------------------------------------------
    readonly property var states: [
        { id: "editor-simple-nord", theme: "nord" },
        { id: "editor-simple-oled", theme: "oled" },
        { id: "editor-simple-latte", theme: "catppuccin-latte" },
        { id: "editor-simple-edited", theme: "nord", ops: [{ op: "accent", value: "c5" }, { op: "font", slot: "ui", value: "Inter" }, { op: "shape", key: "radius", value: 16 }, { op: "opacity", value: 85 }] },
        // ENGINE.md §10 painted: fonts the rig has (the captures' own fontconfig adds DejaVu Serif + Sans Mono) at 120 %
        { id: "editor-fonts", theme: "nord", ops: [{ op: "font", slot: "ui", value: "DejaVu Serif" }, { op: "font", slot: "code", value: "DejaVu Sans Mono" }, { op: "size", value: 120 }] },
        { id: "editor-warnings-oled", theme: "oled", ops: [{ op: "role", id: "text.primary", value: "#303030" }, { op: "token", id: "composer.fg", value: "#202020" }] },
        { id: "editor-advanced-nord", theme: "nord", mode: "advanced" },
        { id: "editor-advanced-search", theme: "nord", mode: "advanced", query: "composer" },
        { id: "editor-advanced-changed", theme: "nord", mode: "advanced", changed: true, ops: [{ op: "background", value: "#101830" }, { op: "token", id: "composer.bg", value: "#3b4252" }, { op: "role", id: "text.link", value: "#ebcb8b" }] },
        { id: "editor-popover-background", theme: "nord", step: "popover-background" },
        { id: "editor-popover-token", theme: "nord", mode: "advanced", query: "composer.bg", step: "popover-row:token:composer.bg" },
        { id: "editor-more-menu", theme: "nord", ops: [{ op: "accent", value: "c3" }], step: "more" },
        { id: "editor-discard", theme: "nord", ops: [{ op: "accent", value: "c3" }], step: "close" },
        { id: "editor-pick-armed", theme: "nord", step: "pick-hover" },
        { id: "editor-picked", theme: "nord", step: "pick-click" },
        { id: "share-cards", share: true },
        { id: "share-try-bar", share: true, trial: true },
        { id: "font-probe", theme: "nord", probe: true, ops: [{ op: "font", slot: "ui", value: "DejaVu Serif" }, { op: "font", slot: "code", value: "DejaVu Sans Mono" }, { op: "size", value: 120 }] }
    ]
    property int idx: -1
    property int fails: 0

    Timer { id: settle; interval: 700; onTriggered: win.stepNow() }
    Timer { id: shoot; interval: 600; onTriggered: win.shotNow() }
    Component.onCompleted: {
        var env = Qt.application.arguments
        for (var i = 0; i < env.length; i++) {
            if (env[i].indexOf("--gallery-dir=") === 0) win.galleryDir = env[i].substring(14)
            if (env[i].indexOf("--gallery-only=") === 0) win.only = env[i].substring(15).split(",").filter(function (x) { return x.length > 0 })
            if (env[i] === "--gallery-scheme") win.withScheme = true
        }
        bootObj.finish()
        themeCtlObj.start()   // gate mode (TINHAT_GUI_THEMESYNC=0): this run's own theme store — OLED, nothing synced
        win.paintTheme("oled")
        Qt.callLater(win.next)
    }
    function reset() {
        themeEditor.closePopups()
        themeEditor.pickArmed = false
        if (editorCtlObj.open) editorCtlObj.close()
        editorCtlObj.endPreview("editor")
        editorCtlObj.endPreview("try")
        win.themeTrial = null
        shareView.visible = false
        backdrop.visible = true
        win.paintTheme("oled")
    }
    function next() {
        win.idx++
        if (win.idx >= win.states.length) {
            console.error("GALLERY done fails=" + win.fails)
            Qt.exit(win.fails > 0 ? 1 : 0)
            return
        }
        var st = win.states[win.idx]
        if ((win.only.length > 0 && win.only.indexOf(st.id) < 0) || (st.probe && win.only.indexOf(st.id) < 0)) { Qt.callLater(win.next); return }
        win.reset()
        if (st.probe) win.logFonts("before")
        if (st.share) {
            backdrop.visible = false
            shareView.build()
            shareView.visible = true
            if (st.trial) win.themeTrial = { key: "$a", docJson: shareView.nordJson, name: "Nord", left: 27 }
        } else {
            var doc = editorCtlObj.docJson(st.theme)
            if (win.withScheme) editorCtlObj.harnessScheme(st.theme)   // Customize… on the theme on screen: its scheme
            var err = editorCtlObj.openEditor(doc, "", JSON.parse(doc).name, true)
            if (err.length > 0) console.error("GALLERY open " + st.theme + ": " + err)
            win.paintPreview()
            themeEditor.mode = st.mode || "simple"
            themeEditor.query = st.query || ""
            for (var o = 0; o < (st.ops || []).length; o++) {
                var e = editorCtlObj.edit(JSON.stringify(st.ops[o]), 0)
                if (e.length > 0) console.error("GALLERY op refused: " + e)
            }
            if (st.changed) themeEditor.changedOnly = true
            if (st.probe) win.logFonts("after")
        }
        settle.restart()
    }
    function stepNow() {
        var st = win.states[win.idx]
        var s = st.step || ""
        if (s === "popover-background") themeEditor.testOpenPopover("background", "")
        else if (s.indexOf("popover-row:") === 0) { var p = s.split(":"); themeEditor.testOpenPopover(p[1], p[2]) }
        else if (s === "more") themeEditor.testOpenMore()
        else if (s === "close") themeEditor.cancel(false)
        else if (s === "pick-hover") { themeEditor.pickArmed = true; Qt.callLater(function () { themeEditor.testPickHover(130, 140) }) }
        else if (s === "pick-click") { themeEditor.pickArmed = true; Qt.callLater(function () { themeEditor.testPickClick(130, 140) }) }
        shoot.restart()
    }
    function shotNow() {
        var st = win.states[win.idx]
        var path = win.galleryDir + "/desktop/" + st.id + ".png"
        // the whole window: the content AND the popup overlay (menus, popovers, dialogs) — QQuickWindow::grabWindow
        var saved = editorCtlObj.grabWindow(path, win.title)
        console.error("GALLERY " + (saved ? "ok " : "FAIL ") + st.id)
        if (!saved) win.fails++
        Qt.callLater(win.next)
    }

    // ---- the stand-in for the app behind the panel (the web's Backdrop: the same tokens) ------------------------------
    // (a Rectangle in the window's colour — the web's backdrop div — so the Pick tool finds window.bg under the islands)
    Rectangle {
        id: backdrop
        anchors.fill: parent
        color: Th.windowBg
        Rectangle {
            id: roomsBox
            x: 6; y: 6; width: 260; height: parent.height - 12
            radius: 12; color: Th.islandBgRooms
            border.width: 1; border.color: Th.islandBorder
            Controls.Label { x: 13; y: 13; text: "Gaming Central"; font.weight: Font.DemiBold; color: Th.roomsItemFg }
            // the web's geometry (rig fonts): the title line, 12 below it the rows, 29 high (padding 6 + a 17 px line)
            Column {
                x: 13; y: 41
                width: parent.width - 26
                Repeater {
                    model: ["general", "game-night", "memes", "valheim"]
                    delegate: Rectangle {
                        required property var modelData
                        required property int index
                        width: parent.width; height: 29
                        radius: 6
                        color: index === 1 ? Th.roomsItemBgSelected : "transparent"
                        // the text fills the row (the web's row div carries the text colour): a Pick on the row finds it
                        Controls.Label { textFormat: Text.PlainText; x: 8; width: parent.width - 16; anchors.verticalCenter: parent.verticalCenter; text: "# " + modelData; color: Th.roomsItemFg }
                    }
                }
            }
        }
        Rectangle {
            x: roomsBox.x + roomsBox.width + 6; y: 6
            width: parent.width - x - 6; height: 52
            radius: 12; color: Th.islandBgHeader; border.width: 1; border.color: Th.islandBorder
        }
        Rectangle {
            id: tl
            x: roomsBox.x + roomsBox.width + 6; y: 64
            width: parent.width - x - 6; height: parent.height - 64 - 60
            radius: 12; color: Th.islandBgTimeline; border.width: 1; border.color: Th.islandBorder
            Rectangle {
                id: b1
                x: 16; y: 16
                width: t1.implicitWidth + 24; height: t1.implicitHeight + 16
                radius: 12; color: Th.bubbleOtherBg
                Controls.Label { id: t1; anchors.centerIn: parent; text: "Who's in for game night?"; color: Th.bubbleOtherFg }
            }
            Rectangle {
                x: 16; y: b1.y + b1.height + 8
                width: t2.implicitWidth + 24; height: t2.implicitHeight + 16
                radius: 12; color: Th.bubbleOwnBg
                Controls.Label { id: t2; anchors.centerIn: parent; text: "Me! 9 pm?"; color: Th.bubbleOwnFg }
            }
        }
        Rectangle {
            x: roomsBox.x + roomsBox.width + 6; y: parent.height - 54
            width: parent.width - x - 6; height: 48
            radius: 12; color: Th.islandBgComposer; border.width: 1; border.color: Th.islandBorder
        }
    }

    // ---- the share cards (the web's ThemeSharePage) -------------------------------------------------------------------
    Rectangle {
        id: shareView
        anchors.fill: parent
        visible: false
        color: Th.timelineBg
        property var rows: []
        property string nordJson: ""
        function build() {
            var nord = JSON.parse(editorCtlObj.docJson("nord"))
            var latte = editorCtlObj.docJson("catppuccin-latte")
            var oled = editorCtlObj.docJson("oled")
            var a = JSON.parse(JSON.stringify(nord)); a.author = "Arctic Ice Studio"
            shareView.nordJson = JSON.stringify(nord)
            function content(docText) { var t = editorCtlObj.shareContentJson(docText); return t.length > 0 ? JSON.parse(t) : ({}) }
            var ca = content(JSON.stringify(a)), cb = content(latte), cc = content(oled)
            var broken = JSON.parse(JSON.stringify(nord)); broken.id = "broken"; broken.name = "Broken Theme"
            var cd = content(JSON.stringify(broken))
            cd["dev.tinhat.theme.share"].theme = JSON.parse(JSON.stringify(broken))
            cd["dev.tinhat.theme.share"].theme.roles = { "text.primary": "nope(" }
            var ce = content(JSON.stringify(nord)); ce["dev.tinhat.theme.share"].v = 2
            var out = []
            var list = [["$a", "@jordan:example.com", ca], ["$b", "@sam:example.com", cb], ["$c", "@alex:example.com", cc],
                        ["$d", "@jordan:example.com", cd], ["$e", "@jordan:example.com", ce]]
            for (var i = 0; i < list.length; i++)
                out.push({ key: list[i][0], sender: list[i][1], row: JSON.parse(editorCtlObj.shareRowJson(JSON.stringify(list[i][2]))) })
            shareView.rows = out
        }
        Column {
            x: 24; y: 24
            spacing: 16
            Repeater {
                model: shareView.rows
                delegate: Rectangle {
                    required property var modelData
                    width: Math.min(360, cardItem.implicitWidth + 24)
                    height: senderLabel.implicitHeight + 4 + cardItem.implicitHeight + 16
                    radius: 12
                    color: Th.bubbleOtherBg
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        id: senderLabel
                        x: 12; y: 8
                        text: modelData.sender
                        font.weight: Font.DemiBold
                        color: Th.bubbleOtherFg
                    }
                    ThemeShareCard {
                        id: cardItem
                        x: 12; y: senderLabel.y + senderLabel.implicitHeight + 4
                        width: implicitWidth; height: implicitHeight
                        app: win
                        row: modelData.row
                        eventId: modelData.key
                        maxWidth: 336
                    }
                }
            }
        }
    }
    ThemeTryBar { z: 10; app: win; trial: win.themeTrial }

    // ---- the real panel ------------------------------------------------------------------------------------------------
    ThemeEditorPanel {
        id: themeEditor
        z: 5
        app: win
        ctl: editorCtlObj
        host: QtObject {
            readonly property Item pickRoot: win.contentItem
            function notify(text, timeout) { console.error("GALLERY toast: " + text) }
            function shareDoc(docJson, name) { }
            function realColours() { return win.thColours }
        }
    }
}
