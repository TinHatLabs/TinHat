// ThToolButton.qml — a Controls.ToolButton drawn with the icon pack (0.8.30).
//
// A stock ToolButton renders `icon.name` through the style: the pack's STATIC glyph, no hover
// story, and a typeset emoji where a name was missing (😊 📌 🎧 ⏺ 🔖). The pack only animates
// through ThIcon, so every utility button that still went through icon.name (composer,
// room-list footer, call status strip, the tile controls) read as a second, frozen icon
// family next to the header and call-bar buttons. This keeps the style's background, focus
// ring, padding and tooltip behaviour and only replaces the content: a ThIcon (hover →
// the story runs, `active` holds the end pose) plus an optional bold label.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.ToolButton {
    id: tb
    property var app: null               // Main.qml root (icon pack); optional
    property string iconName: ""
    property string tip: ""
    property string label: ""            // optional text beside the glyph
    property bool active: false          // "on" state: holds the story's end pose (like checked)
    property color iconColor: Th.buttonToolFg
    property int iconSize: Kirigami.Units.iconSizes.smallMedium

    display: Controls.AbstractButton.IconOnly
    Accessible.role: Accessible.Button
    Accessible.name: tip.length > 0 ? tip : label
    Controls.ToolTip.text: tip
    Controls.ToolTip.visible: hovered && tip.length > 0
    Controls.ToolTip.delay: 400

    contentItem: RowLayout {
        spacing: Kirigami.Units.smallSpacing
        ThIcon {
            Layout.preferredWidth: tb.iconSize
            Layout.preferredHeight: tb.iconSize
            Layout.alignment: Qt.AlignCenter
            app: tb.app
            name: tb.iconName
            color: tb.iconColor
            isMask: true
            hovered: tb.hovered
            active: tb.active || tb.checked
        }
        Controls.Label {
            visible: tb.label.length > 0
            Layout.alignment: Qt.AlignVCenter
            text: tb.label
            color: tb.iconColor
            font.bold: true
        }
    }
}
