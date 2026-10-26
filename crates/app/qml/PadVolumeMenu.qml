// PadVolumeMenu.qml — Pad: one person's three volumes ON THE RECEIVER (docs/PAD.md `vol` / `vol.reset`; DESIGN §1
// "People"). The same menu as the per-person volume menu (ParticipantVolumeMenu.qml — its dials, labels, 0–200 %, a
// mute per row, "Reset all to 100%"), but every change goes to the receiver: a drag sends live pushes (≤ 4/s, paced in
// pad_engine.rs, no disk write there), the release / a wheel notch sends the final level, a mute is a set. The
// values come from the receiver's state (only non-default ones travel) and follow it.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the volume menu tokens

ThMenu {
    id: menu
    property var app: null
    property string mxid: ""
    property string name: ""
    property string deviceName: ""
    property var person: null          // the receiver's people[] entry {u, vol: {cat: n}, mute: [cat]}
    signal command(var cmd)            // the Pad window sends it (and keeps the press optimistic)

    modal: true
    dim: false   // like the web's popover: the window behind stays as it is (a modal Popup dims by default)
    margins: 6
    readonly property var dials: [
        { label: "Their mic",          cat: "mic" },
        { label: "Their screen share", cat: "screenshare" },
        { label: "Their soundboard",   cat: "music" }
    ]
    property var vals: ({ mic: 1, screenshare: 1, music: 1 })
    property var muted: ({ mic: false, screenshare: false, music: false })
    // the receiver's values, unless a dial is held right now
    property string dragging: ""
    function syncFrom(p) {
        var v = {}, m = {}
        for (var i = 0; i < menu.dials.length; i++) {
            var c = menu.dials[i].cat
            v[c] = (p && p.vol && typeof p.vol[c] === "number") ? p.vol[c] : 1
            m[c] = !!(p && p.mute && p.mute.indexOf(c) !== -1)
        }
        if (menu.dragging.length > 0) v[menu.dragging] = menu.vals[menu.dragging]
        menu.vals = v
        menu.muted = m
    }
    onPersonChanged: menu.syncFrom(menu.person)
    onAboutToShow: menu.syncFrom(menu.person)
    function level(x) { return Math.round(Math.max(0, Math.min(2, x)) * 100) / 100 }

    Controls.Label {
        leftPadding: Kirigami.Units.smallSpacing
        rightPadding: Kirigami.Units.smallSpacing
        topPadding: Kirigami.Units.smallSpacing
        text: "Volume — " + menu.name + " (on " + menu.deviceName + ")"
        color: Th.callVolumeTitleFg
        font.bold: true
        elide: Text.ElideRight
        width: Math.min(implicitWidth, 300)
    }
    Repeater {
        model: menu.dials
        delegate: RowLayout {
            id: dialRow
            required property var modelData
            readonly property string cat: modelData.cat
            spacing: Kirigami.Units.smallSpacing
            Item { Layout.preferredWidth: Kirigami.Units.smallSpacing }
            Controls.Label {
                Layout.preferredWidth: Math.max(118, Math.ceil(8.75 * fontInfo.pixelSize))
                text: modelData.label
                color: menu.muted[dialRow.cat] ? Th.callVolumeLabelFgMuted : Th.callVolumeLabelFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            ThSlider {
                callHandle: true
                Layout.preferredWidth: 150
                from: 0.0; to: 2.0
                stepSize: 0.01
                snapMode: Controls.Slider.SnapAlways
                wheelEnabled: true
                Kirigami.StyleHints.tickMarkStepSize: -1
                enabled: !menu.muted[dialRow.cat]
                value: menu.vals[dialRow.cat]
                onMoved: {
                    var v = {}; for (var k in menu.vals) v[k] = menu.vals[k]
                    v[dialRow.cat] = value
                    menu.vals = v
                    if (pressed) {
                        menu.dragging = dialRow.cat
                        menu.command({ op: "vol", user: menu.mxid, cat: dialRow.cat, level: menu.level(value), live: true })
                    } else {
                        menu.command({ op: "vol", user: menu.mxid, cat: dialRow.cat, level: menu.level(value) })   // a wheel notch: final
                    }
                }
                onPressedChanged: {
                    if (pressed) { menu.dragging = dialRow.cat; return }
                    menu.dragging = ""
                    menu.command({ op: "vol", user: menu.mxid, cat: dialRow.cat, level: menu.level(value) })   // the release: always sent
                }
            }
            Controls.Label {
                Layout.preferredWidth: 38
                text: Math.round(menu.vals[dialRow.cat] * 100) + "%"
                color: Th.callVolumeValueFg
                horizontalAlignment: Text.AlignRight
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            Controls.ToolButton {
                icon.name: menu.muted[dialRow.cat] ? "audio-volume-muted-symbolic" : "audio-volume-high-symbolic"
                Controls.ToolTip.text: menu.muted[dialRow.cat] ? ("Unmute " + modelData.label) : ("Mute " + modelData.label)
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.delay: 400
                onClicked: {
                    var nm = !menu.muted[dialRow.cat]
                    var m = {}; for (var k in menu.muted) m[k] = menu.muted[k]
                    m[dialRow.cat] = nm
                    menu.muted = m
                    menu.command({ op: "vol", user: menu.mxid, cat: dialRow.cat, muted: nm })
                }
            }
            Item { Layout.preferredWidth: Kirigami.Units.smallSpacing }
        }
    }
    Controls.MenuSeparator {}
    Controls.MenuItem {
        text: "Reset all to 100%"
        enabled: Math.abs(menu.vals.mic - 1) > 0.001 || Math.abs(menu.vals.screenshare - 1) > 0.001 || Math.abs(menu.vals.music - 1) > 0.001
            || menu.muted.mic || menu.muted.screenshare || menu.muted.music
        onTriggered: {
            menu.vals = { mic: 1, screenshare: 1, music: 1 }
            menu.muted = { mic: false, screenshare: false, music: false }
            menu.command({ op: "vol.reset", user: menu.mxid })
        }
    }
}
