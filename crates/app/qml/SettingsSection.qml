// SettingsSection.qml — a section of a Settings pane (contract SETTINGS-WINDOW.md §3): the heading (14 px, 600,
// settings.section.title.fg), an optional hint (12 px, settings.section.hint.fg, 2 below), 8 px, then its card of
// rows (SettingsCard). `headerTrailing`: something at the heading's right end (a Refresh button). `plain`: the
// content without a card (Appearance → Theme). `anchorId`: a search id that lands on the whole section.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import dev.tinhat.app   // Th: the settings.section.* tokens

ColumnLayout {
    id: sec
    property string title: ""
    property string hint: ""
    property bool plain: false
    property string anchorId: ""
    property var anchorIds: []
    property alias headerTrailing: headRight.data
    default property alias rows: card.rows
    Layout.fillWidth: true
    spacing: 0
    function flash() { card.flash() }

    RowLayout {
        Layout.fillWidth: true
        visible: sec.title.length > 0 || headRight.implicitWidth > 0
        spacing: 8
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            Layout.fillWidth: true
            text: sec.title
            color: Th.settingsSectionTitleFg
            font.pixelSize: Math.round(14 * Th.textScale)
            font.weight: Font.DemiBold
            elide: Text.ElideRight
        }
        RowLayout { id: headRight; spacing: 4 }
    }
    Controls.Label {
        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
        Layout.fillWidth: true
        Layout.topMargin: 2
        visible: sec.hint.length > 0
        text: sec.hint
        color: Th.settingsSectionHintFg
        font.pixelSize: Math.round(12 * Th.textScale)
        wrapMode: Text.Wrap
        onLinkActivated: (link) => LinkGate.open(link)
    }
    SettingsCard {
        id: card
        Layout.topMargin: (sec.title.length > 0 || sec.hint.length > 0) ? 8 : 0
        plain: sec.plain
    }
}
