// WidgetAnswerMark.qml — widgets: ✓ / ? / ✕ of an RSVP answer (the "?" is a bold character one px larger than the icons);
// widgets W2 moved it out of UpNextWidget.qml unchanged (Up next's and Game night's answer buttons and glyphs).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: am
    property var app
    property string v: "yes"
    property int px: 12
    property color ink: Th.widgetUpNextRsvpFg
    width: am.px; height: am.px
    ThIcon {
        anchors.fill: parent
        visible: am.v !== "maybe"
        app: am.app
        name: am.v === "yes" ? "checkmark-symbolic" : "window-close-symbolic"
        color: am.ink
    }
    Text {
        anchors.centerIn: parent
        visible: am.v === "maybe"
        text: "?"
        font.family: Kirigami.Theme.defaultFont.family
        font.pixelSize: am.px + 1
        font.weight: Font.Bold
        color: am.ink
        textFormat: Text.PlainText
    }
}
