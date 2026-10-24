// ThCheckBox — the house check box (0.8.33; on QtQuick.Templates since 0.8.34: no
// platform style underneath — see ThComboBox). Stock CheckBox API, our indicator (a dark
// rounded square that fills accent with a drawn tick) and label colour — the same on both
// platforms. Colours: the toggle.* tokens of Th.
import QtQuick
import QtQuick.Templates as T
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

T.CheckBox {
    id: cb
    property color labelColor: Th.toggleFg

    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding,
                             implicitIndicatorHeight + topPadding + bottomPadding)
    spacing: 8
    padding: 2
    hoverEnabled: true
    font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale   // ENGINE.md §10: the theme's text size

    indicator: Rectangle {
        implicitWidth: 20; implicitHeight: 20
        x: cb.leftPadding
        y: (cb.height - height) / 2
        radius: 5
        color: cb.checked ? Th.toggleIndicatorBgChecked : (cb.hovered ? Th.toggleIndicatorBgHover : Th.toggleIndicatorBg)
        border.width: cb.activeFocus ? 2 : 1
        border.color: cb.checked || cb.activeFocus ? Th.toggleIndicatorBorderChecked : Th.toggleIndicatorBorder
        Behavior on color { ColorAnimation { duration: Motion.durShort } }
        Canvas {
            id: tick
            anchors.centerIn: parent
            width: 12; height: 10
            opacity: cb.checked ? 1 : 0
            Behavior on opacity { NumberAnimation { duration: Motion.durShort } }
            onPaint: {
                var c = getContext("2d"); c.clearRect(0, 0, width, height)
                c.strokeStyle = Th.toggleIndicatorIcon; c.lineWidth = 2.2; c.lineCap = "round"; c.lineJoin = "round"
                c.beginPath(); c.moveTo(1.5, 5.5); c.lineTo(4.8, 8.5); c.lineTo(10.5, 1.5); c.stroke()
            }
            Component.onCompleted: requestPaint()
            Connections { target: Th; function onRevisionChanged() { tick.requestPaint() } }   // a theme switch repaints the tick
        }
    }
    contentItem: Text {
        text: cb.text
        font: cb.font
        color: cb.enabled ? cb.labelColor : Th.toggleFgDisabled
        opacity: cb.enabled ? 1.0 : 0.6
        verticalAlignment: Text.AlignVCenter
        wrapMode: Text.WordWrap
        leftPadding: cb.indicator.width + cb.spacing
    }
}
