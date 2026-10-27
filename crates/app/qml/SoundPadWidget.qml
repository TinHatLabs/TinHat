// SoundPadWidget.qml — widgets W2: the Soundboard pad (`soundPad`, contract WIDGETS.md §22, WIDGETS-UI.md §22; the web's
// features/widgets/soundpad/SoundPad.tsx). Your soundboard favourites (the panel's "Favourites", starred with "Add to
// favourites") as big buttons in the order you starred them — M 6, L 12 (3 per row), S the first 3 as emoji chips in
// the header. A press plays exactly as the soundboard panel does (its own toggleSound: in a call into the call AND
// locally; outside a call only locally — "Only you hear these here…"); pressing the playing sound stops it. In a call
// whose roles turned `use_soundboard` off for you the buttons are disabled with the panel's own line.
//
// Data: AppCore's mPersonalSounds (SoundboardController, read when this widget appears and when the window gets the
// focus back), the pad's list = SoundboardController.padSounds (crates/widgets soundpad.rs = the web's soundpad.ts), the
// gate = padGate(mInCall, mCallPerms); playback = app.soundboardPanel.toggleSound / playingUrl. Samples: the web's
// (TINHAT_WIDGETS_SCENARIO=soundPad=<key>). Motion: pressRipple (one ring from the press point, verylong, only while the
// host animates). No timer.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: w
    property var app
    property var hostRef
    property string size: "M"
    property var settings: ({})
    property bool preview: false
    readonly property var wc: w.hostRef ? w.hostRef.widgets : null
    readonly property var sb: w.app ? w.app.soundboardCtl : null
    readonly property var panel: w.app ? w.app.soundboardPanel : null
    readonly property string scenario: w.wc ? String(w.wc.scenarioOf("soundPad") || "") : ""
    readonly property int maxShown: w.size === "L" && !w.preview ? 12 : 6

    // the favourites: read when the widget appears and when the window gets the focus back (another device may have
    // starred one; the panel re-reads them itself when it opens)
    function reload() { if (w.sb && w.scenario.length === 0 && w.app && w.app.mLoggedIn === true) w.sb.refreshPersonal() }
    Component.onCompleted: w.reload()
    Connections { target: w.app; ignoreUnknownSignals: true; function onMLoggedInChanged() { w.reload() } }
    readonly property string winState: w.hostRef ? w.hostRef.winState : "focused"
    onWinStateChanged: if (w.winState === "focused") w.reload()

    // ── the model (the web's SoundPadModel) ──
    readonly property var liveSounds: {
        if (!w.sb) return []
        try { return JSON.parse(String(w.sb.padSounds(JSON.stringify(w.app.mPersonalSounds || []), 12))) } catch (e) { return [] }
    }
    readonly property var liveGate: {
        if (!w.sb) return { allowed: true, hint: "" }
        try { return JSON.parse(String(w.sb.padGate(!!w.app.mInCall, JSON.stringify(w.app.mCallPerms || {})))) } catch (e) { return { allowed: true, hint: "" } }
    }
    readonly property bool hasData: w.liveSounds.length > 0
    readonly property bool sample: w.scenario.length > 0 || (w.preview && !w.hasData)
    readonly property var model: w.sample ? w.sampleModel(w.scenario.length > 0 ? w.scenario : "preview")
        : ({ sounds: w.liveSounds, gate: w.liveGate, playingUrl: w.panel ? String(w.panel.playingUrl || "") : "" })
    readonly property var shown: w.model.sounds.slice(0, w.maxShown)
    readonly property bool live: false

    readonly property Component summaryComponent: Component { Chips {} }
    // M / L header: the sound playing now
    readonly property Component extraComponent: Component { WidgetHeaderText { text: w.playingName.length > 0 ? "▶ " + w.playingName : "" } }
    readonly property string playingName: {
        var u = w.model.playingUrl
        if (!u) return ""
        for (var i = 0; i < w.model.sounds.length; ++i) if (w.model.sounds[i].url === u) return String(w.model.sounds[i].name)
        return ""
    }

    function press(d) {
        if (w.preview || w.sample || !w.panel || !w.model.gate.allowed) return
        w.panel.toggleSound(d.url, d.name, d.emoji)
    }
    function openBoard() {
        if (w.preview || w.sample || !w.app) return
        w.app.soundboardRefresh(); w.panel.openOrRaise()
    }

    implicitHeight: bodyCol.implicitHeight

    Column {
        id: bodyCol
        x: 8; width: parent.width - 16
        spacing: 6
        // empty: "Star sounds in the soundboard to pin them here" + Open soundboard
        Item {
            visible: w.shown.length === 0
            width: parent.width; height: visible ? Math.max(64, emptyCol.implicitHeight + 8) : 0
            Column {
                id: emptyCol
                anchors.centerIn: parent
                width: parent.width - 16
                spacing: 4
                WidgetText {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    role: "text"
                    text: "Star sounds in the soundboard to pin them here"
                    color: Th.widgetMetaFg
                    wrapMode: Text.Wrap
                    maximumLineCount: 2
                }
                Controls.AbstractButton {
                    id: openBtn
                    anchors.horizontalCenter: parent.horizontalCenter
                    implicitWidth: openTxt.implicitWidth; implicitHeight: openTxt.implicitHeight
                    enabled: !w.preview && !w.sample
                    opacity: enabled || w.sample ? 1 : 0.5
                    hoverEnabled: true
                    focusPolicy: Qt.TabFocus
                    contentItem: WidgetText { id: openTxt; role: "meta"; semibold: true; text: "Open soundboard"; color: Th.accentBright; font.underline: openBtn.hovered || openBtn.visualFocus }
                    onClicked: w.openBoard()
                }
            }
        }
        // the pad: 3 per row, 56 tall, gap 6
        Grid {
            visible: w.shown.length > 0
            columns: 3
            spacing: 6
            width: parent.width
            Repeater {
                model: w.shown.length
                delegate: PadButton {
                    required property int index
                    width: (bodyCol.width - 12) / 3
                    height: 56
                    d: w.shown[index]
                }
            }
        }
        WidgetText {
            visible: w.shown.length > 0 && String(w.model.gate.hint || "").length > 0
            x: 6; width: parent.width - 12
            role: "meta"
            text: String(w.model.gate.hint || "")
            color: w.model.gate.allowed ? Th.widgetMutedFg : Th.soundboardStatusFgError
            wrapMode: Text.Wrap
            maximumLineCount: 3
        }
        Item { width: 1; height: 1 }   // 6 px under the last line (the Column's spacing), the web's padding
    }

    // ── a pad button: the soundboard panel's card (rest / hover / playing / focus), the emoji over the name ──
    component PadButton: Controls.AbstractButton {
        id: pb
        property var d: ({ name: "", url: "", emoji: "" })
        property bool compact: false
        readonly property bool playing: pb.d.url.length > 0 && pb.d.url === w.model.playingUrl
        enabled: !w.preview && w.model.gate.allowed
        opacity: pb.enabled ? 1 : 0.5
        hoverEnabled: true
        focusPolicy: Qt.TabFocus
        Accessible.name: (pb.playing ? "Stop " : "Play ") + pb.d.name
        Controls.ToolTip.text: pb.playing ? "Stop " + pb.d.name : pb.d.name
        Controls.ToolTip.visible: hovered && pb.compact
        Controls.ToolTip.delay: 500
        background: Rectangle {
            radius: pb.compact ? 11 : 10
            color: pb.playing ? Th.soundboardCardBgActive : (pb.hovered && pb.enabled ? Th.soundboardCardBgHover : Th.soundboardCardBg)
            border.width: pb.visualFocus || pb.playing ? 2 : 1
            border.color: pb.visualFocus ? Th.soundboardCardBorderFocus : (pb.playing ? Th.soundboardCardBorderActive : Th.soundboardCardBorder)
            clip: true
            // pressRipple: one ring from the press point (verylong, OutCubic) — only while the host animates
            Rectangle {
                id: ripple
                width: 16; height: 16; radius: 8
                color: Th.soundboardCardBorderActive
                opacity: 0
                property real cx: 0
                property real cy: 0
                x: ripple.cx - 8; y: ripple.cy - 8
                transform: Scale { id: rs; origin.x: 8; origin.y: 8; xScale: 1; yScale: 1 }
                ParallelAnimation {
                    id: rippleAnim
                    NumberAnimation { target: rs; properties: "xScale,yScale"; from: 1; to: 9; duration: w.hostRef ? w.hostRef.durVeryLong : 0; easing.type: Easing.OutCubic }
                    NumberAnimation { target: ripple; property: "opacity"; from: 0.45; to: 0; duration: w.hostRef ? w.hostRef.durVeryLong : 0; easing.type: Easing.OutCubic }
                }
            }
        }
        contentItem: Item {
            Column {
                anchors.centerIn: parent
                width: parent.width
                spacing: 2
                Text {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: pb.d.emoji
                    font.pixelSize: pb.compact ? 13 : 20
                    color: Th.soundboardCardFg
                }
                WidgetText {
                    visible: !pb.compact
                    width: parent.width - 12
                    anchors.horizontalCenter: parent.horizontalCenter
                    horizontalAlignment: Text.AlignHCenter
                    role: "meta"; semibold: true
                    text: pb.d.name
                    color: Th.soundboardCardFg
                }
            }
        }
        onPressed: {
            if (!w.hostRef || w.hostRef.durVeryLong <= 0) return
            ripple.cx = pb.pressX; ripple.cy = pb.pressY
            rippleAnim.restart()
        }
        onClicked: w.press(pb.d)
    }

    // S / collapsed: the first three as emoji chips (30 × 22, they play too; the name is the tooltip)
    component Chips: Item {
        id: chips
        readonly property var first: w.model.sounds.slice(0, 3)
        // as many of them as fit whole (30 + 4 each): a narrow header drops the last ones instead of cutting one (the
        // web's wrapping chip row). implicitWidth stays the full row's, so the frame's min(implicitWidth, slot) decides.
        readonly property int fit: Math.min(chips.first.length, Math.max(0, Math.floor((chips.width + 4) / 34)))
        implicitWidth: chips.first.length > 0 ? chips.first.length * 34 - 4 : emptyTxt.implicitWidth
        implicitHeight: 22
        Row {
            id: chipRow
            visible: chips.first.length > 0
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            spacing: 4
            Repeater {
                model: chips.fit
                delegate: PadButton { required property int index; compact: true; width: 30; height: 22; d: chips.first[index] }
            }
        }
        WidgetHeaderText {
            id: emptyTxt
            visible: chips.first.length === 0
            width: parent.width
            anchors.verticalCenter: parent.verticalCenter
            text: "Star sounds to pin them here"
        }
    }

    // ── the sample states (the gallery's preview, the review screenshots, the gates): the web's samples ──
    function sampleModel(key) {
        var mi = function (f) { return "https://www.myinstants.com/media/sounds/" + f }
        var all = [
            { name: "Bruh", url: mi("bruh.mp3"), emoji: "😐" },
            { name: "MLG Airhorn", url: mi("airhorn.mp3"), emoji: "📯" },
            { name: "Sad trombone", url: mi("sadtrombone.mp3"), emoji: "🎺" },
            { name: "Vine boom", url: mi("vine-boom.mp3"), emoji: "💥" },
            { name: "Emotional damage", url: mi("emotional-damage.mp3"), emoji: "😭" },
            { name: "Discord call", url: mi("discord-call.mp3"), emoji: "📞" },
            { name: "Oof", url: mi("oof.mp3"), emoji: "🔊" },
            { name: "Wow", url: mi("wow.mp3"), emoji: "🤩" },
            { name: "Drum roll", url: mi("drumroll.mp3"), emoji: "🥁" },
            { name: "Crickets", url: mi("crickets.mp3"), emoji: "🦗" }
        ]
        var n = key === "few" ? 2 : (key === "empty" ? 0 : 8)
        var sounds = all.slice(0, n)
        var inCall = key !== "local" && key !== "empty"
        var gate = { allowed: true, hint: "" }
        try { gate = JSON.parse(String(w.sb.padGate(inCall, key === "off" ? "{\"use_soundboard\":false}" : "{}"))) } catch (e) {}
        return { sounds: sounds, gate: gate, playingUrl: key === "playing" ? sounds[1].url : "" }
    }
}
