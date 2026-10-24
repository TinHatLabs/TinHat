// WidgetHeaderText.qml — widgets W1: one line of W-meta for the widget frame's header "extra" slot (a summary or a
// header extra) that ELIDES when the slot is narrower than the text. The frame's slot Loader sizes its item to
// min(item.implicitWidth, slot width); a plain elided Text reports the width it was given as its implicitWidth, so a
// text that starts squeezed stays at 0 px (found 2026-09-28: Pinned's "5 pins · latest: …" never showed). Here the
// implicitWidth comes from an unconstrained, invisible copy of the text, and the visible one elides to our width.
import QtQuick
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: ht
    property string text: ""
    property color color: Th.widgetSummaryFg
    property string role: "meta"
    implicitWidth: measure.implicitWidth
    implicitHeight: shown.implicitHeight
    WidgetText { id: measure; visible: false; role: ht.role; text: ht.text }
    WidgetText {
        id: shown
        width: ht.width
        anchors.verticalCenter: parent.verticalCenter
        role: ht.role
        text: ht.text
        color: ht.color
    }
}
