// TuneIn.qml — games round 2, F9 (GAMES-V1.1-ADDENDUM §6): the side game for broke players. A tin hat's antenna hunts
// a hidden signal on a dial 1–100: 7 tries, the static says higher / lower; found early = more chips (the server's
// SIDE_PAY). Everything is the server's (the secret, the range, the tries, the pay, the caps): this screen only shows
// the run and sends guesses (POST /side/tune, /side/tune/{run} through PlayController.playApi). Keyboard: ←/→ move the
// needle, Enter tunes. The static is still when the window isn't focused (low-GPU rule). Tin Hat Web draws the same.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Shapes
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Rectangle {
    id: tu
    property var app: null
    property var play: null
    property string spaceId: ""
    property bool focusedWindow: true
    signal done()   // Back to the table / close
    readonly property var r2: tu.app && tu.app.playWords && tu.app.playWords.table && tu.app.playWords.table.r2 ? tu.app.playWords.table.r2 : ({})
    readonly property var art: tu.app && tu.app.playArt ? tu.app.playArt : ({})
    function say(k, map) { var s = String(tu.r2[k] !== undefined ? tu.r2[k] : k); for (var x in (map || {})) s = s.split("{" + x + "}").join(String(map[x])); return s }
    readonly property real fBody: (Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10) * Th.textScale

    // the run (the server's answers)
    property var run: null            // {run, lo, hi, tries_left, expires, pay, left_today}
    property string result: ""        // "" | higher | lower | found | lost
    property int paid: 0
    property int secret: 0
    property string err: ""
    property bool busy: false
    property int guess: 50
    property string pendingId: ""
    readonly property int lo: tu.run ? tu.run.lo : 1
    readonly property int hi: tu.run ? tu.run.hi : 100
    readonly property int triesLeft: tu.run ? tu.run.tries_left : 7
    readonly property bool over: tu.result === "found" || tu.result === "lost"
    onLoChanged: tu.guess = Math.round((tu.lo + tu.hi) / 2)
    onHiChanged: tu.guess = Math.round((tu.lo + tu.hi) / 2)

    // a contract op id (16–32 url-safe chars) from Rust: the server refuses anything else (lab7, round 2)
    function opId() { return tu.play ? String(tu.play.newOpId()) : "" }
    function start() {
        if (!tu.play || tu.busy) return
        tu.result = ""; tu.paid = 0; tu.secret = 0; tu.err = ""; tu.run = null
        tu.busy = true
        tu.pendingId = "start-" + tu.opId()
        if (!tu.play.playApi(tu.pendingId, "POST", "/side/tune", JSON.stringify({ space: tu.spaceId }))) { tu.busy = false; tu.err = tu.say("tune_error") }
    }
    function tune() {
        if (!tu.play || tu.busy || !tu.run || tu.over) return
        var g = Math.max(tu.lo, Math.min(tu.hi, Math.round(tu.guess)))
        tu.busy = true
        tu.pendingId = "try-" + tu.opId()
        if (!tu.play.playApi(tu.pendingId, "POST", "/side/tune/" + tu.run.run, JSON.stringify({ op: tu.opId(), guess: g }))) { tu.busy = false; tu.err = tu.say("tune_error") }
    }
    Connections {
        target: tu.play
        ignoreUnknownSignals: true
        function onApiRevChanged() {
            var a; try { a = JSON.parse(tu.play.apiJson) } catch (e) { return }
            if (!a || a.id !== tu.pendingId) return
            tu.busy = false; tu.pendingId = ""
            if (!a.ok) { tu.err = tu.say("tune_code_" + (a.code || "other")); if (tu.err.indexOf("tune_code_") === 0) tu.err = tu.say("tune_error"); return }
            var b = a.body || {}
            if (String(a.id).indexOf("start-") === 0) { tu.run = b; return }
            tu.run = { run: tu.run.run, lo: b.lo, hi: b.hi, tries_left: b.tries_left, expires: tu.run.expires, pay: tu.run.pay, left_today: tu.run.left_today }
            tu.result = String(b.result || "")
            if (b.paid) tu.paid = b.paid
            if (b.secret) tu.secret = b.secret
            if (tu.over && tu.play) tu.play.refreshWallet()
        }
    }

    objectName: "tuneIn"
    color: Th.surfaceRaised
    radius: 12
    border.width: 1
    border.color: Th.borderSubtle
    implicitWidth: 420
    implicitHeight: col.implicitHeight + 32
    focus: true
    Keys.onLeftPressed: tu.guess = Math.max(tu.lo, tu.guess - 1)
    Keys.onRightPressed: tu.guess = Math.min(tu.hi, tu.guess + 1)
    Keys.onReturnPressed: tu.over ? tu.start() : tu.tune()
    Keys.onEnterPressed: tu.over ? tu.start() : tu.tune()
    Keys.onEscapePressed: tu.done()
    // F-D2 (12.2.1): while the call fills the app (the game panel fills it), Main's window Shortcut "Escape" (leave
    // "fill the app") took the key before this card's handler: claim Escape at the shortcut-override stage, so it
    // closes Tune In first (the next Escape still leaves "fill the app")
    Keys.onShortcutOverride: (event) => { if (event.key === Qt.Key_Escape) event.accepted = true }
    Component.onCompleted: { tu.forceActiveFocus(); tu.start() }

    MouseArea { anchors.fill: parent; z: -1; acceptedButtons: Qt.AllButtons }   // a click on the card stays on the card

    ColumnLayout {
        id: col
        anchors.fill: parent
        anchors.margins: 16
        spacing: 10
        RowLayout {
            Layout.fillWidth: true
            Controls.Label { text: tu.say("tune_title"); textFormat: Text.PlainText; font.bold: true; font.pointSize: tu.fBody * 1.15; color: Th.textPrimary; Layout.fillWidth: true }
            Row {   // the tries: 7 antenna pips, lit = left
                spacing: 4
                Accessible.role: Accessible.StaticText
                Accessible.name: tu.say("tune_tries", { n: tu.triesLeft })
                Repeater {
                    model: 7
                    Rectangle {
                        required property int index
                        width: 4; height: 12 + (index % 2) * 4; radius: 2
                        anchors.bottom: parent.bottom
                        color: index < tu.triesLeft ? (tu.art.chip || Th.accentBright) : Th.borderSubtle
                    }
                }
            }
        }
        Controls.Label {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            textFormat: Text.PlainText
            color: Th.textSecondary
            font.pointSize: tu.fBody * 0.85
            text: tu.say("tune_how", { n: tu.run && tu.run.left_today !== undefined ? tu.run.left_today : 10 })
        }
        // the dial: a semicircle 1–100, the remaining range shaded, the needle at the guess, static that calms
        Item {
            id: dial
            Layout.fillWidth: true
            Layout.preferredHeight: 150
            readonly property real cx: width / 2
            readonly property real cy: height - 10
            readonly property real r: Math.min(width / 2 - 10, height - 20)
            function ang(v) { return 180 + (v - 1) / 99 * 180 }   // 1 = left, 100 = right (degrees, QML arcs)
            Shape {
                anchors.fill: parent
                preferredRendererType: Shape.CurveRenderer
                ShapePath {   // the scale
                    strokeColor: Th.borderStrong; strokeWidth: 10; fillColor: "transparent"; capStyle: ShapePath.RoundCap
                    PathAngleArc { centerX: dial.cx; centerY: dial.cy; radiusX: dial.r; radiusY: dial.r; startAngle: 180; sweepAngle: 180 }
                }
                ShapePath {   // what's still possible
                    strokeColor: tu.art.chip || Th.accentBase; strokeWidth: 10; fillColor: "transparent"; capStyle: ShapePath.FlatCap
                    PathAngleArc { centerX: dial.cx; centerY: dial.cy; radiusX: dial.r; radiusY: dial.r; startAngle: dial.ang(tu.lo); sweepAngle: Math.max(1, dial.ang(tu.hi) - dial.ang(tu.lo)) }
                }
            }
            Repeater {   // the static: specks over the dial, fewer as the range narrows; still when unfocused
                model: 28
                Rectangle {
                    required property int index
                    property real seed: Math.random()
                    readonly property real a: Math.PI + (index / 28 + seed * 0.03) * Math.PI
                    readonly property real rr: dial.r * (0.55 + seed * 0.4)
                    x: dial.cx + Math.cos(a) * rr; y: dial.cy + Math.sin(a) * rr
                    width: 2; height: 2; radius: 1
                    color: Th.textMuted
                    visible: index < Math.round(28 * (tu.hi - tu.lo + 1) / 100)
                    opacity: 0.25 + seed * 0.5
                }
            }
            Timer {
                interval: 140; repeat: true
                running: tu.focusedWindow && Motion.focused && Motion.animate && !Motion.reducedMotion && tu.visible && !tu.over
                onTriggered: { for (var i = 0; i < dial.children.length; ++i) if (dial.children[i].seed !== undefined) dial.children[i].seed = Math.random() }
            }
            Rectangle {   // the needle
                width: 3; height: dial.r - 6; radius: 1.5
                color: Th.textPrimary
                x: dial.cx - width / 2; y: dial.cy - height
                transformOrigin: Item.Bottom
                rotation: dial.ang(tu.guess) - 270
                Behavior on rotation { NumberAnimation { duration: Motion.durShort } }
            }
            Rectangle { width: 12; height: 12; radius: 6; color: Th.textPrimary; x: dial.cx - 6; y: dial.cy - 6 }
            Controls.Label { text: "1"; x: dial.cx - dial.r - 4; y: dial.cy + 2; color: Th.textMuted; font.pointSize: tu.fBody * 0.72 }
            Controls.Label { text: "100"; x: dial.cx + dial.r - 12; y: dial.cy + 2; color: Th.textMuted; font.pointSize: tu.fBody * 0.72 }
            TapHandler {   // a click on the dial puts the needle there
                onTapped: function (ev) {
                    var dx = ev.position.x - dial.cx, dy = ev.position.y - dial.cy
                    var deg = Math.atan2(dy, dx) * 180 / Math.PI; if (deg < 0) deg += 360
                    if (deg < 180) return
                    tu.guess = Math.max(tu.lo, Math.min(tu.hi, Math.round(1 + (deg - 180) / 180 * 99)))
                }
            }
        }
        // the answer line: the static's word, found / lost
        Controls.Label {
            objectName: "tuneLine"
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            textFormat: Text.PlainText
            font.weight: tu.over ? Font.DemiBold : Font.Normal
            font.pointSize: tu.fBody * (tu.over ? 1.04 : 0.92)
            color: tu.result === "found" ? (tu.art.win || Th.accentBright) : (tu.err !== "" ? Th.statusErrorFg : Th.textPrimary)
            text: tu.err !== "" ? tu.err
                : tu.result === "found" ? (tu.paid === 1 ? tu.say("tune_found_one") : tu.say("tune_found", { n: tu.paid }))
                : tu.result === "lost" ? tu.say("tune_lost", { n: tu.secret })
                : tu.result === "higher" ? tu.say("tune_higher") : tu.result === "lower" ? tu.say("tune_lower")
                : tu.run ? tu.say("tune_range", { lo: tu.lo, hi: tu.hi }) : ""
        }
        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            spacing: 8
            visible: !tu.over
            Controls.SpinBox {
                id: field
                objectName: "tuneField"
                from: tu.lo; to: tu.hi
                value: tu.guess
                editable: true
                onValueModified: tu.guess = value
                Accessible.name: tu.say("tune_field")
            }
            ThButton {
                objectName: "tuneGo"
                app: tu.app
                highlighted: true
                enabled: !!tu.run && !tu.busy
                text: tu.say("tune_go")
                Accessible.name: text
                onClicked: tu.tune()
            }
        }
        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            spacing: 8
            ThButton {
                objectName: "tuneAgain"
                app: tu.app
                visible: tu.over && tu.run && tu.run.left_today !== 0
                highlighted: true
                text: tu.say("tune_again")
                onClicked: tu.start()
            }
            ThButton {
                objectName: "tuneBack"
                app: tu.app
                flat: true
                text: tu.say("tune_back")
                onClicked: tu.done()
            }
        }
    }
}
