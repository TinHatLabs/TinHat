// SettingsPage.qml — one pane of the Settings window (contract SETTINGS-WINDOW.md §3): one scroll area; an inner
// column at most 720 wide, left-aligned, padding 28 left/right, 24 top, 32 bottom; the pane header (title 22 px 600,
// description 13 px 4 below) and 24 px to the first section and between sections. `bottomSpace` keeps the last row
// clear of the save bar. `scrollToItem(item)` centres an item (the search jump, §4).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import dev.tinhat.app   // Th: the settings.pane.* tokens, Motion

Controls.ScrollView {
    id: page
    property var app: null
    property string key: ""
    property string title: ""
    property string description: ""
    property real bottomSpace: 0
    default property alias content: col.data
    readonly property real innerWidth: col.width

    contentWidth: availableWidth
    contentHeight: inner.implicitHeight
    clip: true
    Controls.ScrollBar.vertical: FoxScrollBar { app: page.app }
    Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff

    function scrollToItem(it) {
        var f = page.contentItem
        if (!f || !it) return
        var p = it.mapToItem(inner, 0, 0)
        // centred (§4); an item taller than the view shows from its top
        var target = it.height > f.height - 48 ? p.y - 16 : p.y + it.height / 2 - f.height / 2
        scrollAnim.to = Math.max(0, Math.min(target, inner.implicitHeight - f.height))
        scrollAnim.restart()
    }
    function scrollToTop() { if (page.contentItem) page.contentItem.contentY = 0 }
    NumberAnimation { id: scrollAnim; target: page.contentItem; property: "contentY"; duration: Motion.durLong; easing.type: Easing.OutCubic }

    Item {
        id: inner
        width: page.availableWidth
        implicitHeight: col.implicitHeight + 24 + 32 + page.bottomSpace
        ColumnLayout {
            id: col
            x: 28
            y: 24
            width: Math.max(0, Math.min(720, inner.width - 56))
            spacing: 24
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    text: page.title
                    color: Th.settingsPaneTitleFg
                    font.pixelSize: Math.round(22 * Th.textScale)
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                    Accessible.role: Accessible.Heading
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    visible: page.description.length > 0
                    text: page.description
                    color: Th.settingsPaneDescriptionFg
                    font.pixelSize: Math.round(13 * Th.textScale)
                    wrapMode: Text.Wrap
                }
            }
        }
    }
}
