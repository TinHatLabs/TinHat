// AmbienceGallery.qml — chat backgrounds + particle effects without a sign-in (the web's #/__gallery/ambience, the same
// stand-in conversation): the real AmbienceLayer over stand-in bubbles in the theme's own tokens. Review captures and
// the performance runs (docs/AMBIENCE.md).
//   lab builds only: the ambience gallery switch (main.rs lab_gallery_qml) + TINHAT_GUI_THEMESYNC=0 QT_QPA_PLATFORM=xcb
//   tinhat --gallery-dir=<dir>
//       [--themes=oled,nord,atom-one-light] [--presets=none,rain,…] [--size=1100x720] [--settle-ms=2500]
//       [--run=<preset>:<theme>:<seconds>]   (performance: no captures, one scene for that long, frame lines logged)
//       [--soak]   (with --run: the rows rotate every 2 s — a new message in, the oldest out: colliders and pools churn)
//       [--menu=mod,readonly,home] [--kbd]   (--kbd: ONE case, no captures: a real "Menu" button with the keyboard focus
//                                     opens the ☰ KeyMenu; scripts/a11y-keyboard-gate.sh A11Y_GALLERY drives it)
//       [--menu=mod,readonly,home]   (round 4: the ☰ menu with the real ParticlesMenu open, per theme and case:
//                                     a moderator in a Rainy evening room / someone who can't change it / no room)
import QtQuick
import QtQuick.Window
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Controls.ApplicationWindow {
    id: win
    width: 1100; height: 720
    visible: true
    title: "Tin Hat ambience gallery"
    color: Th.windowBg
    Kirigami.Theme.inherit: false
    Kirigami.Theme.textColor: Th.textPrimary
    Kirigami.Theme.backgroundColor: Th.surfaceWindow

    ThemeController { id: themeCtlObj }
    ThemeEditorController { id: editorCtlObj }
    BootController { id: bootObj }
    AmbienceController { id: ambCtl }
    property var thColours: null
    function paintTheme(id) {
        var c = themeCtlObj.resolve(id)
        win.thColours = c
        Th.apply(c, themeCtlObj.themeId, themeCtlObj.themeName, themeCtlObj.variant, themeCtlObj.names, themeCtlObj.shape, themeCtlObj.look)
    }
    // the bits of Main.qml the layer reads
    readonly property var ambience: ambCtl
    property bool mChatBackgrounds: true
    property bool mChatEffects: true
    property bool mChatEffectsPause: false   // the gallery window may not have the focus under xcb
    property bool mCallUnfocusedSaver: false
    property string bgPreset: "rain"
    // the bits of Main.qml the ☰ ParticlesMenu reads (round 4; --menu): a stand-in room, the prefs written nowhere
    property string mUserId: "@me:gallery"
    readonly property var settings: QtObject { function applyUiPref(k, v) { console.error("AMBIENCE menu pref " + k + "=" + v) } }
    QtObject {
        id: fakeRoom
        property string backgroundJson: JSON.stringify({ content: { v: 1, kind: "preset", preset: "rain" }, sender: "@mod:gallery" })
        property bool canSetBackground: true
        function setBackground(j) { console.error("AMBIENCE menu write " + j) }
    }
    property var menuCases: []
    property bool kbdMode: false

    property string galleryDir: ""
    property var themes: ["oled", "nord", "atom-one-light"]
    property var presets: ["none", "rain", "hearth", "snow", "summer", "dusk", "deep"]
    property int settleMs: 2500
    property var shots: []
    property int idx: -1
    property int fails: 0
    property string run: ""
    property int frames: 0
    property int turn: 0
    Timer { id: soakTimer; interval: 2000; repeat: true; onTriggered: win.turn++ }

    Component.onCompleted: {
        var a = Qt.application.arguments
        for (var i = 0; i < a.length; i++) {
            var s = a[i]
            if (s.indexOf("--gallery-dir=") === 0) galleryDir = s.substring(14)
            else if (s.indexOf("--themes=") === 0) themes = s.substring(9).split(",")
            else if (s.indexOf("--presets=") === 0) presets = s.substring(10).split(",")
            else if (s.indexOf("--settle-ms=") === 0) settleMs = Number(s.substring(12))
            else if (s.indexOf("--size=") === 0) { var wh = s.substring(7).split("x"); win.width = Number(wh[0]); win.height = Number(wh[1]) }
            else if (s.indexOf("--run=") === 0) run = s.substring(6)
            else if (s === "--soak") soakTimer.start()
            else if (s.indexOf("--menu=") === 0) menuCases = s.substring(7).split(",")
            else if (s === "--kbd") kbdMode = true
        }
        bootObj.finish()
        themeCtlObj.start()
        if (run.length > 0) {
            var r = run.split(":")
            paintTheme(r[1]); bgPreset = r[0]
            console.error("AMBIENCE run " + run + " api=" + GraphicsInfo.api)
            runEnd.interval = Number(r[2]) * 1000
            runEnd.start(); perfTick.start()
            return
        }
        if (kbdMode && menuCases.length > 0) {
            paintTheme(themes[0]); bgPreset = "rain"
            fakeRoom.canSetBackground = menuCases[0] !== "readonly"
            particlesMenu.roomOpen = menuCases[0] !== "home"
            win.mChatEffectsPause = true
            menuBtn.forceActiveFocus(Qt.TabFocusReason)
            runEnd.interval = 300000; runEnd.start()
            return
        }
        if (menuCases.length > 0) for (var mt = 0; mt < themes.length; mt++) for (var mc = 0; mc < menuCases.length; mc++) shots.push({ theme: themes[mt], preset: "rain", menu: menuCases[mc] })
        else for (var t = 0; t < themes.length; t++) for (var p = 0; p < presets.length; p++) shots.push({ theme: themes[t], preset: presets[p] })
        Qt.callLater(next)
    }
    Timer { id: runEnd; onTriggered: { console.error("AMBIENCE run done"); Qt.exit(0) } }
    Timer {
        id: perfTick; interval: 5000; repeat: true
        onTriggered: { console.error("AMBIENCE frames " + win.frames + " in 5 s, verts " + layer.fxVerts + ", particles " + layer.fxParticles + ", state " + layer.fxState); win.frames = 0 }
    }
    Connections { target: win; function onFrameSwapped() { win.frames++ } }
    onTurnChanged: layer.effectItem && list.contentHeightChanged()
    Timer { id: shoot; onTriggered: shotNow() }
    function next() {
        idx++
        if (idx >= shots.length) { console.error("AMBIENCE done fails=" + fails); Qt.exit(fails > 0 ? 1 : 0); return }
        paintTheme(shots[idx].theme)
        bgPreset = shots[idx].preset
        if (shots[idx].menu) openMenuCase(shots[idx].menu)
        shoot.interval = settleMs
        shoot.restart()
    }
    function shotNow() {
        var st = shots[idx]
        var path = galleryDir + "/" + (st.menu ? "menu-" + st.menu : st.preset) + "-" + st.theme + "-desktop.png"
        var ok = editorCtlObj.grabWindow(path, win.title)
        console.error("AMBIENCE " + (ok ? "ok " : "FAIL ") + path + " state=" + layer.fxState + " verts=" + layer.fxVerts)
        if (!ok) fails++
        Qt.callLater(next)
    }

    // ---- round 4 (--menu): the ☰ list as Main.qml draws it (stand-in rows) with the REAL ParticlesMenu beside it ----
    function openMenuCase(c) {
        fakeAppMenu.close()
        fakeRoom.canSetBackground = c !== "readonly"
        particlesMenu.roomOpen = c !== "home"
        win.mChatEffectsPause = true   // the default (the gallery's own runs keep it off for xcb focus)
        fakeAppMenu.popup(24, 64)
        // open Particles ▸ the way a click does: the row's triggered → QQuickMenu opens its submenu
        Qt.callLater(function () { var row = fakeAppMenu.itemAt(3); if (row) { fakeAppMenu.currentIndex = 3; row.triggered() } })
    }
    Controls.Button {
        id: menuBtn
        visible: win.kbdMode
        x: 24; y: 16
        text: "☰"
        Accessible.name: "Menu"
        onClicked: fakeAppMenu.popup(menuBtn, 0, menuBtn.height)
    }
    KeyMenu {
        id: fakeAppMenu
        modal: false
        closePolicy: win.kbdMode ? (Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside) : Controls.Popup.NoAutoClose
        Controls.MenuItem { text: "Settings…"; icon.name: "settings-configure" }
        Controls.MenuItem { text: "Edit layout…"; icon.name: "window-symbolic" }
        Controls.Menu { title: "Theme"; icon.name: "preferences-desktop-color"; Controls.MenuItem { text: "OLED" } }
        ParticlesMenu {
            id: particlesMenu
            app: win
            roomCtl: fakeRoom
        }
        Controls.MenuSeparator {}
        Controls.MenuItem { text: "About Tin Hat"; icon.name: "help-about-symbolic" }
        Controls.MenuItem { text: "Sign out"; icon.name: "system-log-out-symbolic" }
    }

    readonly property var msgs: [
        { who: "ava", name: "Ava", text: "Anyone up for a round tonight?", time: "18:02", day: "Today" },
        { who: "ava", name: "Ava", text: "I was thinking the new co-op map, it looked great in the trailer", time: "18:02" },
        { who: "me", name: "Me", text: "I'm in after dinner. 9 pm?", time: "18:05" },
        { who: "kai", name: "Kai", text: "9 works. I'll bring the snacks and the bad jokes", time: "18:07" },
        { who: "noor", name: "Noor", text: "Count me in! Still downloading the patch though, it's huge", time: "18:11" },
        { who: "me", name: "Me", text: "No rush, we'll wait in the lobby", time: "18:12" },
        { who: "ava", name: "Ava", text: "Perfect. Let's meet in the voice room at 8:55 🎮", time: "18:15" },
        { who: "kai", name: "Kai", text: "See you there", time: "18:16" }
    ]

    Rectangle {
        anchors.fill: parent
        color: Th.timelineBg
        AmbienceLayer {
            id: layer
            app: win
            anchors.fill: list
            list: list
            readonly property int fxVerts: layer.effectItem.verts
            readonly property int fxParticles: layer.effectItem.particles
            background: JSON.parse(win.ambience.parse(win.bgPreset === "none" ? "" : JSON.stringify({ v: 1, kind: "preset", preset: win.bgPreset }), "@ava:example.org"))
        }
        ListView {
            id: list
            anchors.fill: parent
            anchors.margins: 0
            leftMargin: 4; rightMargin: 16
            spacing: 4
            verticalLayoutDirection: ListView.BottomToTop
            model: win.msgs.length
            delegate: Item {
                id: row
                required property int index
                readonly property int at: win.msgs.length - 1 - index
                readonly property var m: win.msgs[(at + win.turn) % win.msgs.length]
                readonly property var prev: at > 0 ? win.msgs[(at - 1 + win.turn) % win.msgs.length] : null
                readonly property bool grouped: !!prev && prev.who === m.who && !m.day
                readonly property bool mine: m.who === "me"
                width: list.width - 20
                height: col.height
                function ambRects(target, out) {
                    function add(it, k, rl, rr) { if (!it.visible || it.width < 2) return; var p = it.mapToItem(target, 0, 0); out.push({ id: "m" + (at + win.turn) + k, x: p.x, y: p.y, w: it.width, h: it.height, rl: rl, rr: rr }) }
                    add(pill, ":d", pill.height / 2, pill.height / 2)
                    add(nameLbl, ":n", 0, 0)
                    add(bubbleRect, ":b", bubbleRect.corners.topLeftRadius, bubbleRect.corners.topRightRadius)
                }
                Column {
                    id: col
                    width: parent.width
                    spacing: 2
                    topPadding: row.grouped ? 1 : 4
                    Item {
                        width: parent.width; height: row.m.day ? 30 : 0; visible: !!row.m.day
                        Controls.Label {
                            textFormat: Text.PlainText
                            id: pill
                            anchors.centerIn: parent
                            visible: !!row.m.day
                            text: row.m.day || ""
                            color: Th.timelineDayPillFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                            leftPadding: 10; rightPadding: 10; topPadding: 2; bottomPadding: 2
                            background: Rectangle { color: Th.timelineDayPillBg; radius: height / 2 }
                        }
                    }
                    Item {
                        width: parent.width
                        height: inner.height
                        Rectangle {
                            visible: !row.mine && !row.grouped
                            x: 4; y: 0; width: 34; height: 34; radius: 17
                            color: Qt.hsla((row.m.name.charCodeAt(0) * 37 % 360) / 360, 0.55, 0.5, 1)
                            Controls.Label { anchors.centerIn: parent; textFormat: Text.PlainText; text: row.m.name[0]; color: Th.textOnAccent; font.bold: true }
                        }
                        Column {
                            id: inner
                            x: row.mine ? parent.width - width : 42
                            spacing: 2
                            Controls.Label {
                                textFormat: Text.PlainText
                                id: nameLbl
                                visible: !row.mine && !row.grouped
                                text: row.m.name
                                font.bold: true
                                font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                                color: Qt.hsla((row.m.name.charCodeAt(0) * 37 % 360) / 360, 0.6, Th.variant === "light" ? 0.35 : 0.7, 1)
                            }
                            Kirigami.ShadowedRectangle {
                                id: bubbleRect
                                width: Math.min(txt.implicitWidth + 24, list.width * 0.6)
                                height: txt.height + stamp.height + 16
                                radius: 12
                                corners { topLeftRadius: (!row.mine && !row.grouped) ? 4 : 12; topRightRadius: (row.mine && !row.grouped) ? 4 : 12; bottomLeftRadius: 12; bottomRightRadius: 12 }
                                color: row.mine ? Th.bubbleOwnBg : Th.bubbleOtherBg
                                border.width: row.mine ? 0 : 1
                                border.color: Th.bubbleOtherBorder
                                shadow.size: 10; shadow.color: row.mine ? Th.bubbleOwnShadow : Th.bubbleOtherShadow; shadow.yOffset: 2
                                Controls.Label {
                                    textFormat: Text.PlainText
                                    id: txt
                                    x: 12; y: 8
                                    width: Math.min(implicitWidth, list.width * 0.6 - 24)
                                    wrapMode: Text.Wrap
                                    text: row.m.text
                                    color: row.mine ? Th.bubbleOwnFg : Th.bubbleOtherFg
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText
                                    id: stamp
                                    anchors.right: parent.right; anchors.rightMargin: 12
                                    y: txt.y + txt.height
                                    text: row.m.time
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.85
                                    color: row.mine ? Th.bubbleOwnMetaFg : Th.bubbleOtherMetaFg
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
