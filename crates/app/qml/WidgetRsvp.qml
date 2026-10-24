// WidgetRsvp.qml — widgets: the RSVP buttons of a calendar event (WIDGETS-UI.md §4 Up next line 4; widgets W2 moved it
// out of UpNextWidget.qml unchanged so Game night answers with the same buttons): Going / Maybe / Can't (the calendar's
// words) as three segments, or icon-only 24 × 24 / mini 20 × 20 with ✓ ? ✕; the current answer again clears it.
// `host` = the widget: preview, myOf(ev), answer(ev, v) (the calendar's write), hostRef (the motion steps), app.
// ── RSVP: three segments (Going / Maybe / Can't — the calendar's words), gap 4, radius 6; icon-only 24 × 24
//    (mini: 20 × 20, radius 5) with ✓ ? ✕; disabled (0.5) where I may not write the calendar ──
// (an Item around the Row: Qt 6.11 refuses an implicit size assigned on a positioner, and the full-width row must
// not take its implicit width from its segments — they size from the row)
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: rs
    property var host                       // the widget: preview, myOf(ev), answer(ev, v), hostRef, app
    property var ev
    property bool iconOnly: false
    property bool mini: false
    readonly property bool pv: !!rs.host && rs.host.preview === true
    readonly property bool can: !!rs.ev && rs.ev.canWrite === true && !rs.pv
    readonly property string my: rs.host ? rs.host.myOf(rs.ev) : ""
    readonly property int segW: rs.mini ? 20 : (rs.iconOnly ? 24 : Math.floor((rs.width - 8) / 3))
    implicitWidth: rs.mini ? 3 * 20 + 8 : (rs.iconOnly ? 3 * 24 + 8 : 0)
    implicitHeight: rs.mini ? 20 : 24
    Row {
    spacing: 4
    Repeater {
        model: [{ v: "yes", label: "Going", icon: "checkmark-symbolic" }, { v: "maybe", label: "Maybe", icon: "" }, { v: "no", label: "Can't", icon: "window-close-symbolic" }]
        delegate: Controls.AbstractButton {
            id: seg
            required property var modelData
            required property int index
            readonly property bool on: rs.my === seg.modelData.v
            readonly property color fgOn: seg.modelData.v === "yes" ? Th.widgetUpNextRsvpYesFgChecked : seg.modelData.v === "maybe" ? Th.widgetUpNextRsvpMaybeFgChecked : Th.widgetUpNextRsvpNoFgChecked
            readonly property color bgOn: seg.modelData.v === "yes" ? Th.widgetUpNextRsvpYesBgChecked : seg.modelData.v === "maybe" ? Th.widgetUpNextRsvpMaybeBgChecked : Th.widgetUpNextRsvpNoBgChecked
            readonly property color ink: seg.on ? seg.fgOn : Th.widgetUpNextRsvpFg
            // the last segment takes the rounding remainder so the three fill the row exactly
            width: rs.mini || rs.iconOnly ? rs.segW : (seg.index === 2 ? rs.width - 2 * (rs.segW + 4) : rs.segW)
            height: rs.mini ? 20 : 24
            hoverEnabled: true
            focusPolicy: Qt.TabFocus
            enabled: rs.can
            opacity: rs.can || rs.pv ? 1 : 0.5
            Accessible.name: seg.modelData.label
            Controls.ToolTip.text: seg.modelData.label
            Controls.ToolTip.visible: (rs.iconOnly || rs.mini) && seg.hovered
            Controls.ToolTip.delay: 500
            background: Rectangle {
                radius: rs.mini ? 5 : 6
                color: seg.on ? seg.bgOn : ((seg.hovered || seg.visualFocus) && rs.can ? Th.widgetUpNextRsvpBgHover : Th.widgetUpNextRsvpBg)
                Behavior on color { enabled: !!rs.host && !!rs.host.hostRef && rs.host.hostRef.dur > 0; ColorAnimation { duration: rs.host && rs.host.hostRef ? rs.host.hostRef.dur : 0 } }
            }
            contentItem: Item {
                WidgetText {
                    anchors.centerIn: parent
                    visible: !(rs.iconOnly || rs.mini)
                    role: "meta"; semibold: true
                    text: seg.modelData.label
                    color: seg.ink
                }
                WidgetAnswerMark {
                    app: rs.host ? rs.host.app : null
                    anchors.centerIn: parent
                    visible: rs.iconOnly || rs.mini
                    v: seg.modelData.v
                    px: rs.mini ? 11 : 12
                    ink: seg.ink
                }
            }
            onClicked: if (rs.host) rs.host.answer(rs.ev, seg.modelData.v)
        }
    }
    }
}
