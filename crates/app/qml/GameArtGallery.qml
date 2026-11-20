// GameArtGallery.qml — "Match the game" (sun-gamebg) without a sign-in (the web's #/__gallery/gamebg): the real
// GameArtLayer (in the real AmbienceLayer's art slot) over the stand-in conversation, one capture per art × theme.
//   lab builds only: the game-art gallery switch (main.rs lab_gallery_qml) + TINHAT_GUI_THEMESYNC=0 QT_QPA_PLATFORM=xcb
//   tinhat --gallery-dir=<dir>
//       --arts=valheim:/path/valheim.jpg,enshrouded:/path/enshrouded.jpg [--themes=goose,oled,adwaita]
//       [--size=1600x1000] [--settle-ms=4000] [--bare]   (bare: the background alone, for the pixel measure)
// Each capture logs "GAMEBG ok <png> veil=<v> ink=<json>" (the ink = every text colour measured, for the gate).
import QtQuick
import QtQuick.Window
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Controls.ApplicationWindow {
    id: win
    width: 1600; height: 1000
    visible: true
    title: "Tin Hat game art gallery"
    color: Th.windowBg
    Kirigami.Theme.inherit: false
    Kirigami.Theme.textColor: Th.textPrimary
    Kirigami.Theme.backgroundColor: Th.surfaceWindow

    ThemeController { id: themeCtlObj }
    ThemeEditorController { id: editorCtlObj }
    BootController { id: bootObj }
    AmbienceController { id: ambCtl }
    function paintTheme(id) {
        var c = themeCtlObj.resolve(id)
        Th.apply(c, themeCtlObj.themeId, themeCtlObj.themeName, themeCtlObj.variant, themeCtlObj.names, themeCtlObj.shape, themeCtlObj.look)
    }
    readonly property var ambience: ambCtl
    property bool mChatBackgrounds: true
    property bool mChatEffects: false
    property bool mChatEffectsPause: false
    property bool mCallUnfocusedSaver: false
    property bool mChatGameArt: true
    readonly property string gameArtWords: "{}"
    property string galleryDir: ""
    property var themes: ["goose", "oled", "adwaita"]
    property var arts: []
    property int settleMs: 4000
    property bool bare: false
    property var shots: []
    property int idx: -1
    property int fails: 0
    property string artFile: ""

    Component.onCompleted: {
        var a = Qt.application.arguments
        for (var i = 0; i < a.length; i++) {
            var s = a[i]
            if (s.indexOf("--gallery-dir=") === 0) galleryDir = s.substring(14)
            else if (s.indexOf("--themes=") === 0) themes = s.substring(9).split(",")
            else if (s.indexOf("--arts=") === 0) arts = s.substring(7).split(",").map(function (x) { var k = x.indexOf(":"); return { name: x.substring(0, k), file: x.substring(k + 1) } })
            else if (s.indexOf("--settle-ms=") === 0) settleMs = Number(s.substring(12))
            else if (s === "--bare") bare = true
            else if (s.indexOf("--size=") === 0) { var wh = s.substring(7).split("x"); win.width = Number(wh[0]); win.height = Number(wh[1]) }
        }
        bootObj.finish()
        themeCtlObj.start()
        for (var g = 0; g < arts.length; g++) for (var t = 0; t < themes.length; t++) shots.push({ art: arts[g], theme: themes[t] })
        Qt.callLater(next)
    }
    Timer { id: shoot; onTriggered: shotNow() }
    function next() {
        idx++
        if (idx >= shots.length) { console.error("GAMEBG done fails=" + fails); Qt.exit(fails > 0 ? 1 : 0); return }
        paintTheme(shots[idx].theme)
        art.pushInk()
        artFile = shots[idx].art.file
        shoot.interval = settleMs
        shoot.restart()
    }
    function shotNow() {
        var st = shots[idx]
        var path = galleryDir + "/" + st.art.name + "-" + st.theme + "-desktop" + (bare ? "-bare" : "") + ".png"
        var ok = editorCtlObj.grabWindow(path, win.title)
        var texts = [art.hex(Th.timelineStartFg), art.hex(Th.timelineEmptyFg), art.hex(Th.timelineNewLineFg)].concat(art.nameColours())
        console.error("GAMEBG " + (ok ? "ok " : "FAIL ") + path + " veil=" + (art.hasArt ? art.shownVeil : "none") + " art=" + art.hasArt
                      + " ink=" + JSON.stringify({ bg: art.hex(Th.timelineBg), texts: texts }))
        if (!ok) fails++
        Qt.callLater(next)
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
            background: JSON.parse(win.ambience.parse("", ""))
            artShown: art.hasArt
            GameArtLayer {
                id: art
                app: win
                previewFile: win.artFile
            }
        }
        ListView {
            id: list
            visible: !win.bare
            anchors.fill: parent
            anchors.margins: 0
            leftMargin: 4; rightMargin: 16
            spacing: 4
            verticalLayoutDirection: ListView.BottomToTop
            model: win.msgs.length
            delegate: Item {
                id: row
                required property int index
                readonly property var m: win.msgs[win.msgs.length - 1 - index]
                readonly property var prev: (win.msgs.length - 2 - index) >= 0 ? win.msgs[win.msgs.length - 2 - index] : null
                readonly property bool grouped: !!prev && prev.who === m.who && !m.day
                readonly property bool mine: m.who === "me"
                width: list.width - 20
                height: col.height
                function ambRects(target, out) {
                    function add(it, k, rl, rr) { if (!it.visible || it.width < 2) return; var p = it.mapToItem(target, 0, 0); out.push({ id: "m" + index + k, x: p.x, y: p.y, w: it.width, h: it.height, rl: rl, rr: rr }) }
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
                            font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
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
                            Controls.Label { anchors.centerIn: parent; textFormat: Text.PlainText; text: row.m.name[0]; color: "white"; font.bold: true }
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
                                font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize * Th.textScale)
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
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.85 * Th.textScale
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
