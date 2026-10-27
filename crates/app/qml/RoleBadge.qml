// RoleBadge — the ONE role chip (RBAC, docs/RBAC.md §7.2): a 1 px border in the role's
// colour, an 18 % tint fill and the name in bold small print. Member rows in the Space
// settings window wear one per role; RightPanel's badge is the same look with the room's
// `roleName` / `roleColor`. Pure-presentational (`app` is still accepted; the default tone is Th's).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: roles.badge.default.fg

Rectangle {
    id: badge
    property var app: null
    property string name: ""
    // A "#rrggbb" string (role colours cross the boundary as strings); "" = the dim text token.
    property string roleColor: ""
    property bool compact: false     // tighter padding for dense lists (the members list)

    // the role's colour (content: server data) or roles.badge.default.fg
    readonly property color tone: {
        if (roleColor.length === 0) return Th.rolesBadgeDefaultFg
        var c = Qt.color(roleColor)   // theme-lint: ok content: the role's own colour
        return c.valid ? c : Th.rolesBadgeDefaultFg
    }

    visible: name.length > 0
    implicitWidth: label.implicitWidth + (compact ? Kirigami.Units.smallSpacing * 2 + 2 : Kirigami.Units.largeSpacing)
    implicitHeight: compact ? Math.round(Kirigami.Units.gridUnit * 1.0) : Math.round(Kirigami.Units.gridUnit * 1.1)
    radius: height / 2
    color: Qt.rgba(tone.r, tone.g, tone.b, 0.18)
    border.width: 1
    border.color: tone

    Controls.Label {
        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
        id: label
        anchors.centerIn: parent
        text: badge.name
        color: badge.tone
        font.pointSize: Kirigami.Theme.smallFont.pointSize
        font.bold: true
        elide: Text.ElideRight
        width: Math.min(implicitWidth, 160)
    }
}
