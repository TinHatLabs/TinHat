// ThemeEditorWarnings.qml — the Simple mode's readability warning (THEMES-UI.md §13.5; the web's InlineMessage
// "warning" with its lines): "‹n› colours are hard to read", then at most 4 lines "‹label› — ‹ratio›:1 on ‹background›,
// needs ‹need›:1" (settings.themeEditor.warn.fg, small) each with a flat Fix (one undoable step), then "and ‹k› more —
// see Advanced". The frame is the house InlineMessage's look (Kirigami's, as the web draws it): a 1 px banner.warning.border
// edge around the banner.warning.bg fill, the warning emblem, banner.fg text. `fixes` = stateJson.fixes ({op, line},
// roles first).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: banner.warning.*, settings.themeEditor.warn.fg

Item {
    id: w
    property var app: null
    property var fixes: []
    signal fix(var op)

    implicitHeight: Math.max(32, body.implicitHeight + 10)

    Rectangle {
        anchors.fill: parent
        radius: Kirigami.Units.cornerRadius
        color: Th.bannerWarningBorder
        Rectangle {
            anchors.fill: parent
            anchors.margins: 1
            radius: 3
            color: Th.bannerWarningBg
        }
    }
    Kirigami.Icon {
        id: emblem
        x: 5 + 4
        anchors.verticalCenter: parent.verticalCenter
        width: 22; height: 22
        source: "emblem-warning"
    }
    ColumnLayout {
        id: body
        x: emblem.x + emblem.width + 8
        y: 5
        width: w.width - x - 5
        spacing: 4
        Controls.Label {
            Layout.fillWidth: true
            text: w.fixes.length === 1 ? "1 colour is hard to read" : w.fixes.length + " colours are hard to read"
            color: Th.bannerFg
            wrapMode: Text.Wrap
        }
        Repeater {
            model: w.fixes.slice(0, 4)
            delegate: RowLayout {
                required property var modelData
                Layout.fillWidth: true
                spacing: 8
                Controls.Label {
                    Layout.fillWidth: true
                    text: modelData.line
                    color: Th.settingsThemeEditorWarnFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
                    wrapMode: Text.Wrap
                }
                ThButton {
                    app: w.app
                    flat: true
                    text: "Fix"
                    onClicked: w.fix(modelData.op)
                }
            }
        }
        Controls.Label {
            visible: w.fixes.length > 4
            Layout.fillWidth: true
            text: "and " + (w.fixes.length - 4) + " more — see Advanced"
            color: Th.settingsThemeEditorWarnFg
            font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
            wrapMode: Text.Wrap
        }
    }
}
