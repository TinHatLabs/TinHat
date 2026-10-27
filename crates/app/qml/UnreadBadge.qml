// UnreadBadge — the ONE unread/mention badge for the whole app (room list, dock, space
// rail, quick switcher). Count capped at 99+; `highlight` (mention/DM) uses the danger
// token, plain unread uses accent — replacing three unrelated hand-rolled badge styles.
import QtQuick
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Rectangle {
    id: badge
    property var app
    property int count: 0
    property bool highlight: false

    visible: count > 0
    implicitHeight: Math.round(Kirigami.Units.gridUnit * 0.95)
    implicitWidth: Math.max(implicitHeight, label.implicitWidth + Kirigami.Units.smallSpacing * 2)
    radius: height / 2
    color: highlight ? Th.badgeCountBgMention : Th.badgeCountBg

    Text {
        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
        id: label
        anchors.centerIn: parent
        text: badge.count > 99 ? "99+" : badge.count
        color: Th.badgeCountFg
        font.bold: true
        font.pixelSize: Math.round(badge.implicitHeight * 0.62)
    }
}
