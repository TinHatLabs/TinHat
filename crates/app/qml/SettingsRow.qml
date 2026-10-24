// SettingsRow.qml — one row of a Settings window card (contract SETTINGS-WINDOW.md §3; the web's settings Row).
//
// Left: the label (14 px, 500) and a short description (12 px, 2 below, wraps). Right (`trailing`): the control,
// vertically centred, 16 px gap. Under the text (the default property): full-width content — a stacked row's text
// field, meter, list … 10 px below. `stacked: true` moves the trailing control under the text too; so does a narrow
// window (content < 560 px: SettingsWindow.settingsNarrow). Padding 14 / 16, at least 56 tall. A 1 px divider
// (settings.card.divider) separates it from the visible row above it in the same card.
//
// Search (§4): `anchorId` (and `anchorIds`) are the settings-index.json ids that land here; `flash()` paints the
// row's background settings.row.bg.highlight for 1.2 s (a fade; at once with Reduce motion).
// `toggle`: a switch the whole row toggles when clicked outside its controls.
import QtQuick
import QtQuick.Layouts
import QtQuick.Window
import QtQuick.Controls as Controls
import dev.tinhat.app   // Th: the settings.* tokens (docs/MIGRATING-THEME.md)

Item {
    id: row
    property var app: null                   // Main.qml root (the icon pack for `iconName`)
    property string anchorId: ""
    property var anchorIds: []
    property string label: ""
    property string description: ""
    property int descriptionFormat: Text.PlainText
    property color labelColor: Th.settingsRowLabelFg
    property color descriptionColor: Th.settingsRowDescriptionFg
    property real labelPixelSize: 14
    property int labelWeight: Font.Medium
    property string chip: ""                 // a small pill after the label ("This device")
    property string iconName: ""             // a leading 20 px icon (devices, verification)
    property color iconColor: Th.settingsRowDescriptionFg
    property bool stacked: false
    property int labelFormat: Text.PlainText
    property string labelFontFamily: ""
    property Item toggle: null
    // a row that IS a button (About & help): hover, a chevron at the end, Return/Space; clicked()
    property bool clickable: false
    signal clicked()
    property alias trailing: trailingBox.data
    property alias leading: leadingBox.data
    default property alias content: belowBox.data
    readonly property bool isSettingsRow: true
    // a row's switch sits in the row's own padding (the Basic style pads a Switch 6 px, which made switch rows 68 tall)
    onToggleChanged: if (row.toggle) { row.toggle.topPadding = 0; row.toggle.bottomPadding = 0 }

    readonly property bool narrow: row.Window.window ? row.Window.window.settingsNarrow === true : false
    readonly property bool trailingBelow: (row.stacked || row.narrow) && !row.clickable
    readonly property bool hasText: row.label.length > 0 || row.description.length > 0
    readonly property bool hasTrailing: trailingBox.implicitWidth > 0
    readonly property bool hasBelow: belowBox.implicitHeight > 0
    property real padV: 14                   // a dense list (Shortcuts) may take less; the row stays ≥ 56
    readonly property real padH: 16
    readonly property real leadW: leadingBox.implicitWidth > 0 ? leadingBox.implicitWidth + 12 : 0
    readonly property real textX: row.padH + row.leadW
    readonly property real sideW: (row.hasTrailing && !row.trailingBelow) ? trailingBox.implicitWidth + 16 : 0
    readonly property real topH: Math.max(row.hasText ? textBlock.implicitHeight : 0,
                                          (row.hasTrailing && !row.trailingBelow) ? trailingBox.implicitHeight : 0,
                                          leadingBox.implicitHeight)
    readonly property real trailingY: row.trailingBelow ? row.padV + row.topH + (row.hasText ? 10 : 0)
                                                        : row.padV + (row.topH - trailingBox.implicitHeight) / 2
    readonly property real belowY: (row.hasTrailing && row.trailingBelow ? row.trailingY + trailingBox.implicitHeight
                                                                        : row.padV + row.topH) + ((row.hasText || row.hasTrailing) ? 10 : 0)

    Layout.fillWidth: true
    implicitWidth: 320
    implicitHeight: Math.max(56, (row.hasBelow ? row.belowY + belowBox.implicitHeight
                                              : (row.hasTrailing && row.trailingBelow ? row.trailingY + trailingBox.implicitHeight
                                                                                     : row.padV + row.topH)) + row.padV)
    Accessible.name: row.label

    // the visible row above in the same card (a divider) / below (the bottom corners of the flash)
    readonly property bool topDivider: {
        var p = row.parent
        if (!p) return false
        var kids = p.children
        for (var i = 0; i < kids.length; i++) {
            if (kids[i] === row) return p.isSettingsRowGroup === true ? p.groupTopDivider === true : false
            if (kids[i].visible && kids[i].isSettingsRow === true) return true
        }
        return false
    }
    readonly property bool lastShown: {
        var p = row.parent
        if (!p) return true
        var kids = p.children, after = false
        for (var i = 0; i < kids.length; i++) {
            if (kids[i] === row) { after = true; continue }
            if (after && kids[i].visible && kids[i].isSettingsRow === true) return false
        }
        return true
    }

    function flash() {
        hl.opacity = 1
        if (Motion.animate) { holdT.stop(); fade.restart() } else { fade.stop(); holdT.restart() }
    }
    Rectangle {
        id: hl
        anchors.fill: parent
        color: Th.settingsRowBgHighlight
        opacity: 0
        topLeftRadius: row.topDivider ? 0 : 9
        topRightRadius: row.topDivider ? 0 : 9
        bottomLeftRadius: row.lastShown ? 9 : 0
        bottomRightRadius: row.lastShown ? 9 : 0
    }
    SequentialAnimation {
        id: fade
        PauseAnimation { duration: 400 }
        NumberAnimation { target: hl; property: "opacity"; to: 0; duration: 800; easing.type: Easing.InOutQuad }
    }
    Timer { id: holdT; interval: 1200; onTriggered: hl.opacity = 0 }

    Rectangle {
        visible: row.topDivider
        width: parent.width
        height: 1
        color: Th.settingsCardDivider
    }
    // a click on the row (not on a control) toggles its switch / runs a clickable row
    Rectangle {
        anchors.fill: parent
        anchors.topMargin: row.topDivider ? 1 : 0
        visible: row.clickable && (rowMa.containsMouse || row.activeFocus)
        color: Th.settingsListItemBgPressed
        topLeftRadius: row.topDivider ? 0 : 9
        topRightRadius: row.topDivider ? 0 : 9
        bottomLeftRadius: row.lastShown ? 9 : 0
        bottomRightRadius: row.lastShown ? 9 : 0
        border.width: row.activeFocus ? 1 : 0
        border.color: Th.settingsRailItemBorderFocus
    }
    MouseArea {
        id: rowMa
        anchors.fill: parent
        enabled: row.enabled && (row.clickable || (!!row.toggle && row.toggle.enabled))
        hoverEnabled: row.clickable
        cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: {
            if (row.clickable) row.clicked()
            else { row.toggle.toggle(); row.toggle.toggled() }
        }
    }
    activeFocusOnTab: row.clickable
    Accessible.role: row.clickable ? Accessible.Button : Accessible.Grouping
    Accessible.onPressAction: if (row.clickable) row.clicked()
    Keys.onReturnPressed: if (row.clickable) row.clicked()
    Keys.onEnterPressed: if (row.clickable) row.clicked()
    Keys.onSpacePressed: if (row.clickable) row.clicked()

    RowLayout {
        id: leadingBox
        x: row.padH
        y: row.padV + (row.topH - implicitHeight) / 2
        spacing: 8
        ThIcon {
            visible: row.iconName.length > 0
            Layout.preferredWidth: 20
            Layout.preferredHeight: 20
            app: row.app
            name: row.iconName
            color: row.iconColor
        }
    }

    ColumnLayout {
        id: textBlock
        visible: row.hasText
        x: row.textX
        y: row.padV + (row.topH - implicitHeight) / 2
        width: Math.max(0, row.width - row.textX - row.padH - row.sideW)
        spacing: 2
        opacity: row.enabled ? 1 : 0.5
        RowLayout {
            Layout.fillWidth: true
            visible: row.label.length > 0
            spacing: 8
            Controls.Label {
                Layout.fillWidth: row.chip.length === 0
                Layout.maximumWidth: textBlock.width - (chipBox.visible ? chipBox.implicitWidth + 8 : 0)
                text: row.label
                textFormat: row.labelFormat
                color: row.labelColor
                font.pixelSize: Math.round(row.labelPixelSize * Th.textScale)
                font.weight: row.labelWeight
                font.family: row.labelFontFamily.length > 0 ? row.labelFontFamily : Qt.application.font.family
                wrapMode: Text.Wrap
            }
            Rectangle {
                id: chipBox
                visible: row.chip.length > 0
                implicitWidth: chipText.implicitWidth + 14
                implicitHeight: chipText.implicitHeight + 4
                radius: height / 2
                color: Th.settingsRailItemBgSelected
                Controls.Label {
                    id: chipText
                    anchors.centerIn: parent
                    text: row.chip
                    color: Th.settingsRowLabelFg
                    font.pixelSize: Math.round(11 * Th.textScale)
                    font.weight: Font.DemiBold
                }
            }
            Item { visible: row.chip.length > 0; Layout.fillWidth: true }
        }
        Controls.Label {
            Layout.fillWidth: true
            visible: row.description.length > 0
            text: row.description
            textFormat: row.descriptionFormat
            color: row.descriptionColor
            font.pixelSize: Math.round(12 * Th.textScale)
            wrapMode: Text.Wrap
            onLinkActivated: (link) => Qt.openUrlExternally(link)
        }
    }

    RowLayout {
        id: trailingBox
        x: row.trailingBelow && !row.clickable ? row.textX : row.width - row.padH - implicitWidth
        y: row.clickable ? row.padV + (row.topH - implicitHeight) / 2 : row.trailingY
        width: row.trailingBelow && !row.clickable ? Math.min(implicitWidth, Math.max(0, row.width - row.textX - row.padH)) : implicitWidth
        spacing: 8
        ThIcon {
            visible: row.clickable
            Layout.preferredWidth: 16
            Layout.preferredHeight: 16
            app: row.app
            name: "go-next-symbolic"
            color: Th.settingsListChevronIcon
            hovered: rowMa.containsMouse
        }
    }

    ColumnLayout {
        id: belowBox
        x: row.textX
        y: row.belowY
        width: Math.max(0, row.width - row.textX - row.padH)
        spacing: 8
    }
}
