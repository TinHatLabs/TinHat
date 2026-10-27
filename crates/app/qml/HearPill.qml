import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Dialogs
import QtQuick.Window
import QtMultimedia
import org.kde.kirigami as Kirigami
import dev.tinhat.app

// HearPill.qml — was Main.qml's inline component; islands C3 (ISLANDS-ARCH §4.2) gives it a file so the
// extracted islands can draw it too. `app` = the Main.qml root.
Rectangle {
    id: hp
    property var app: null   // the Main.qml root
    property string text: ""
    property string icon: ""
    property string tone: "info"
    property string tip: ""
    property string action: ""
    // Narrow windows: the short forms (a meaning, not an ellipsis) take over before anything elides.
    property string shortText: ""
    property string shortAction: ""
    signal activated()
    readonly property color fg: tone === "danger" ? Th.hearPillDangerFg : (tone === "warn" ? Th.hearPillWarnFg : Th.hearPillInfoFg)
    TextMetrics { id: tmFull; font: hpText.font; text: hp.text }
    TextMetrics { id: tmShort; font: hpText.font; text: hp.shortText.length > 0 ? hp.shortText : hp.text }
    TextMetrics { id: tmAct; font: hpAct.font; text: "· " + hp.action }
    TextMetrics { id: tmActShort; font: hpAct.font; text: "· " + (hp.shortAction.length > 0 ? hp.shortAction : hp.action) }
    readonly property real iconW: hp.icon.length > 0 ? 18 : 0
    readonly property real fullW: Math.ceil(16 + iconW + tmFull.advanceWidth + (hp.action.length > 0 ? 5 + tmAct.advanceWidth : 0))
    readonly property real shortW: Math.ceil(16 + iconW + tmShort.advanceWidth + (hp.action.length > 0 ? 5 + tmActShort.advanceWidth : 0))
    readonly property bool compact: width < fullW - 0.5
    // In the call header's RowLayout: full width when there is room, down to the short form when
    // there is not (the layout shrinks the room name first). Metrics only — no width↔implicit loop.
    implicitWidth: fullW
    implicitHeight: Math.max(hpText.implicitHeight, 13) + 6
    Layout.fillWidth: true
    Layout.preferredWidth: fullW
    Layout.maximumWidth: fullW
    Layout.minimumWidth: shortW
    radius: height / 2
    color: tone === "danger" ? Th.hearPillDangerBg : (tone === "warn" ? Th.hearPillWarnBg : Th.hearPillInfoBg)
    border.width: 1
    border.color: tone === "danger" ? Th.hearPillDangerBorder : (tone === "warn" ? Th.hearPillWarnBorder : Th.hearPillInfoBorder)
    HoverHandler { id: hpHover; cursorShape: hp.action.length > 0 ? Qt.PointingHandCursor : Qt.ArrowCursor }
    TapHandler { enabled: hp.action.length > 0; onTapped: hp.activated() }
    Controls.ToolTip.visible: hpHover.hovered && (hp.tip.length > 0 || hp.compact)
    Controls.ToolTip.delay: 300
    Controls.ToolTip.text: hp.compact ? hp.text + (hp.tip.length > 0 ? "\n" + hp.tip : "") : hp.tip
    Accessible.role: hp.action.length > 0 ? Accessible.Button : Accessible.StaticText
    Accessible.name: hp.text + (hp.action.length > 0 ? " — " + hp.action : "")
    Row {
        id: hpRow
        anchors.left: parent.left
        anchors.leftMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        spacing: 5
        ThIcon {
            visible: hp.icon.length > 0
            anchors.verticalCenter: parent.verticalCenter
            width: 13; height: 13
            app: hp.app
            name: hp.icon
            color: hp.fg
            isMask: true
        }
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            id: hpText
            anchors.verticalCenter: parent.verticalCenter
            text: hp.compact && hp.shortText.length > 0 ? hp.shortText : hp.text
            width: Math.max(0, Math.min(implicitWidth, hp.width - 16 - hp.iconW - (hp.action.length > 0 ? 5 + hpAct.implicitWidth : 0)))
            elide: Text.ElideRight
            color: hp.fg
            font.pointSize: Kirigami.Theme.smallFont.pointSize
        }
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            id: hpAct
            visible: hp.action.length > 0
            anchors.verticalCenter: parent.verticalCenter
            text: "· " + (hp.compact && hp.shortAction.length > 0 ? hp.shortAction : hp.action)
            color: hpHover.hovered ? Th.hearPillActionFgHover
                                   : (hp.tone === "danger" ? Th.hearPillDangerActionFg : (hp.tone === "warn" ? Th.hearPillWarnActionFg : Th.hearPillInfoActionFg))
            font.pointSize: Kirigami.Theme.smallFont.pointSize
            font.bold: true
            font.underline: hpHover.hovered
        }
    }
}
