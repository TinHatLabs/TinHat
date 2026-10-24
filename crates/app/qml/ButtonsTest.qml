// ButtonsTest.qml — the buttons track's offscreen review captures (the web's #/__gallery/actions, same content): today's
// rows from the default rows (idle / in a call), a button island of your own in every look and scale, and the Ctrl+K
// palette over a sample world — on OLED and Goose. Not a gate: nothing is compared.
// Run: TINHAT_BUTTONS_TEST=1 QT_QPA_PLATFORM=offscreen tinhat --buttonstest-out=<dir>   → <dir>/desktop-*.png, then quits.
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Window {
    id: win
    width: 1560; height: 1720
    visible: true
    color: Th.surfaceWindow
    title: "Tin Hat buttons test"

    readonly property string outDir: String(Qt.application.arguments.filter(function (a) { return a.indexOf("--buttonstest-out=") === 0 })[0] || "").substring(18)

    // what the house components read from `app` (FallbackTheme's sizes, the icon pack, the actions engine)
    readonly property real radiusSm: 5
    readonly property real radius: 8
    readonly property real radiusLg: 12
    readonly property real radiusXl: 16
    readonly property int dur: Motion.dur
    readonly property var font: ({ family: "" })
    property var mIconPack: null
    readonly property string mIconStyle: themeCtl.iconStyle
    IconPackController { id: iconPack }
    Component.onCompleted: {
        try { mIconPack = iconPack.packJson.length > 0 ? JSON.parse(iconPack.packJson) : null } catch (e) { mIconPack = null }
        step.start()
    }
    ThemeController { id: themeCtl }
    ActionsController { id: buttonsTestCtl }   // (a unique id: the boundary check reads ids app-wide)
    readonly property var actionsCtl: buttonsTestCtl
    readonly property var actionsTable: JSON.parse(buttonsTestCtl.tableJson())
    readonly property var room: QtObject { property string roomId: "!gallery:example.com" }
    readonly property var actions: QtObject {
        property string snapshotJson: JSON.stringify(win.world(true))
        function run(id, inv) { console.log("run", id, JSON.stringify(inv ? inv.target : null)) }
        function keysFor(id) { return [] }   // the palette's key hints (ActionsHost.keysFor): none in the review world
    }

    readonly property string roomId: "!gallery:example.com"
    readonly property string other: "!valheim:example.com"
    function world(inCall) {
        var values = { "ui.roomOpen": true, "presence.known": true, "gif.available": true, "platform.screenCapture": true,
                       "server.calls": true, "server.soundboard": true, "server.gifs": true, "server.watch": true, "server.games": true, "server.steam": true,
                       "server.gameservers": true, "server.presence": true, "server.push": true, "server.memories": true, "server.reports": true,
                       "server.guests": true, "selfhost.retired": false,
                       "prefs.callLayout": "auto", "prefs.nsMode": "standard", "prefs.echoCancel": true, "update.state": "none",
                       "prefs.theme": "goose", "calendar.hasContainer": true }
        if (inCall) {
            values["call.in"] = true; values["ui.callShown"] = true; values["call.micMuted"] = true; values["call.cameraOn"] = true
            values["call.recActive"] = true; values["call.soundPlaying"] = true; values["call.soundName"] = "Bruh"
            values["call.hasSavedAudioSource"] = true
        }
        var rooms = {}
        rooms[roomId] = { name: "Gaming Central", values: { "room.joined": true, "room.current": true, "room.inSpace": true, "room.hasPinned": true,
                                                            "room.pinnedCount": "3", "room.callLive": inCall, "room.inCall": inCall, "room.recordOn": inCall } }
        rooms[other] = { name: "Valheim", values: { "room.joined": true, "room.callLive": true } }
        return { values: values, current: { room: roomId, activeSpace: "!space:example.com" },
                 subjects: { room: rooms,
                             space: { "!space:example.com": { name: "Example", values: { "space.joined": true, "space.canManage": true } } },
                             theme: { oled: { name: "OLED", values: {} }, goose: { name: "Goose", values: {} } },
                             sound: { "mxc://example.com/bruh": { name: "Bruh", values: {} } } } }
    }
    readonly property var island: [
        { a: "call.mic.toggle" }, { a: "call.deafen.toggle" }, { a: "call.camera.toggle" }, { a: "call.leave" },
        { a: "call.join", target: { room: "!valheim:example.com" } }, { a: "sound.play", target: { sound: "mxc://example.com/bruh" } },
        { a: "status.set", target: { status: "lfg" } }, { a: "app.theme.set", target: { theme: "goose" } },
        { a: "app.reduceMotion.toggle" }, { a: "call.teleport" }, { a: "calendar.open" }
    ]
    readonly property var rows: [["header", "main", "header.main"], ["account", "main", "account.main"], ["rooms", "header", "rooms.header"],
                                 ["composer", "start", "composer.start"], ["composer", "end", "composer.end"],
                                 ["call", "status", "call.status"], ["call", "bar", "call.bar"]]
    function dflt(isl, row) { var t = buttonsTestCtl.defaultRow(isl, row, "desktop", "classic"); return t === "null" ? null : JSON.parse(t) }

    // a row with its own snapshot (the preview shows two worlds side by side)
    component PreviewRow: ButtonRow {
        property bool inCall: true
        app: win
        runner: function (it, anchor) { console.log("run", it.a) }
        readonly property string _snap: JSON.stringify(win.world(inCall))
        // ButtonRow reads app.actions.snapshotJson; a preview row evaluates its own world
        snapshotOverride: _snap
    }
    component Label: Controls.Label {
        Layout.preferredWidth: 190
        font.pointSize: Kirigami.Theme.smallFont.pointSize
        color: Th.textSecondary
    }

    Rectangle {
        id: page
        anchors.fill: parent
        color: Th.surfaceWindow
        GridLayout {
            anchors { left: parent.left; top: parent.top; margins: 16 }
            columns: 2
            columnSpacing: 12
            rowSpacing: 8
            Repeater {
                model: win.rows.length * 2
                delegate: Item {
                    required property int index
                    Layout.columnSpan: 2
                    implicitWidth: rowLine.implicitWidth
                    implicitHeight: Math.max(40, rowLine.implicitHeight)
                    readonly property var r: win.rows[Math.floor(index / 2)]
                    readonly property bool inCall: index % 2 === 1
                    RowLayout {
                        id: rowLine
                        spacing: 12
                        Label { text: r[2] + (inCall ? " — in a call" : " — idle") }
                        PreviewRow {
                            inCall: parent.parent.inCall
                            rowKind: parent.parent.r[2]
                            value: win.dflt(parent.parent.r[0], parent.parent.r[1])
                            context: (parent.parent.r[2].indexOf("composer") === 0 || parent.parent.r[2].indexOf("call") === 0) ? ({ room: win.roomId }) : null
                        }
                    }
                }
            }
            Repeater {
                model: 6
                delegate: Item {
                    required property int index
                    Layout.columnSpan: 2
                    implicitWidth: 1500
                    implicitHeight: islandLine.implicitHeight
                    readonly property string look: ["icon", "iconLabel", "label"][Math.floor(index / 2)]
                    readonly property string sc: index % 2 === 0 ? "S" : "M"
                    RowLayout {
                        id: islandLine
                        spacing: 12
                        Label { text: "buttons island — " + parent.parent.look + " " + parent.parent.sc }
                        PreviewRow {
                            Layout.preferredWidth: 1250
                            wrap: true
                            rowKind: "buttons.main"
                            value: ({ look: parent.parent.look, scale: parent.parent.sc, items: win.island })
                        }
                    }
                }
            }
            Label { text: "buttons island — idle" }
            PreviewRow { inCall: false; wrap: true; Layout.preferredWidth: 1250; rowKind: "buttons.main"; value: ({ look: "iconLabel", scale: "S", items: win.island }) }
            Label { text: "edit mode (hidden items faint)" }
            PreviewRow { inCall: false; editing: true; rowKind: "buttons.main"
                         value: ({ items: [{ a: "call.leave", hideWhenOff: true }, { a: "call.teleport", hideWhenOff: true }, { a: "soundboard.open" }] }) }
        }
    }

    QuickSwitcher {
        id: qs
        app: win
        rooms: [
            // two pending invites (Ctrl+K lists them first, newest first — ACTIONS.md §10.1; mRooms puts them on top)
            { id: "!invsam:example.com", name: "Sam", unread: 0, dm: true, peer: "@sam:example.com", membership: "invite", invitedAt: 2000, inviteRank: 0, avatar: "" },
            { id: "!invmovie:example.com", name: "Movie night 2", unread: 0, dm: false, membership: "invite", invitedAt: 1000, inviteRank: 1, avatar: "" },
            { id: win.roomId, name: "Gaming Central", unread: 2, dm: false, alias: "#gaming:example.com", avatar: "" },
            { id: win.other, name: "Valheim", unread: 0, dm: false, avatar: "" },
            { id: "!jordan:example.com", name: "Jordan", unread: 1, dm: true, peer: "@jordan:example.com", avatar: "" },
            { id: "!movie:example.com", name: "Movie night", unread: 0, dm: false, avatar: "" }
        ]
    }

    // the capture script: rows on OLED, rows on Goose, then the palette states on Goose
    property int stepNo: 0
    readonly property var paletteSteps: [["palette-empty", "", ""], ["palette-invites", "mo", ""], ["palette-mute", "mute", ""], ["palette-ca", "ca", ""],
                                         ["palette-actions", ">", ""], ["palette-leave", "leave", ""], ["palette-people", "@", ""],
                                         ["palette-stage-theme", "", "app.theme.set"], ["palette-stage-status", "", "status.set"]]
    function shot(name, next) {
        win.contentItem.grabToImage(function (res) {
            var p = win.outDir + "/desktop-" + name + ".png"
            console.log("shot", p, res.saveToFile(p))
            next()
        })
    }
    function paint(themeId) {
        Th.apply(themeCtl.resolve(themeId), themeCtl.themeId, themeCtl.themeName, themeCtl.variant, themeCtl.names)
    }
    Timer {
        id: step
        interval: 900
        onTriggered: {
            var n = win.stepNo++
            if (n === 0) { paint("oled"); step.start(); return }
            if (n === 1) { shot("rows-oled", function () { paint("goose"); step.start() }); return }
            if (n === 2) { shot("rows-goose", function () { step.start() }); return }
            var k = n - 3
            if (k < win.paletteSteps.length * 2) {
                var s = win.paletteSteps[Math.floor(k / 2)]
                if (k % 2 === 0) {
                    qs.close()
                    if (s[2].length > 0) qs.openAt(s[2]); else qs.open()
                    step.start()
                    return
                }
                if (s[1].length > 0) qs.setQuery(s[1])
                shot(s[0], function () { step.start() })
                return
            }
            Qt.quit()
        }
    }
}
