// GameServerLine.qml — a game server in one line (docs/GAMESERVERS.md §5; the web's gameservers/parts.tsx ServerLine):
// Up next's hero and the calendar card show the server a calendar event's game links to — the state dot, the chip
// text, the join address with a copy button and, when there is one, a copy button for the password. `sv` = one
// server of the controller's status VIEW. Colours can follow the host (the calendar card's own-bubble colours).
import QtQuick
import QtQuick.Layouts
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

RowLayout {
    id: ln
    property var app
    property var sv: null
    property bool preview: false
    property color textColor: Th.widgetMetaFg
    property color strongColor: Th.widgetFg
    readonly property string tone: ln.sv && ln.sv.chip ? String(ln.sv.chip.tone) : "offline"
    spacing: 4
    implicitHeight: 22
    Rectangle {   // the state dot (widget.servers.state.*)
        Layout.alignment: Qt.AlignVCenter
        implicitWidth: 6; implicitHeight: 6; radius: 3
        color: ln.tone === "online" ? Th.widgetServersStateOnline : (ln.tone === "busy" ? Th.widgetServersStateBusy : Th.widgetServersStateOffline)
    }
    WidgetText {
        role: "meta"
        text: ln.sv && ln.sv.chip ? String(ln.sv.chip.text) : ""
        color: ln.textColor
    }
    WidgetText { role: "meta"; text: "·"; color: ln.textColor }
    WidgetText {
        Layout.fillWidth: true
        Layout.minimumWidth: 20
        role: "num"
        text: ln.sv && ln.sv.join ? String(ln.sv.join.address) : ""
        color: ln.strongColor
    }
    GameServerIconButton {
        side: 20
        app: ln.app
        iconName: "edit-copy-symbolic"
        label: "Copy address"
        glyph: ln.textColor
        enabled: !ln.preview
        onClicked: if (ln.app && ln.sv) { ln.app.copyToClipboard(String(ln.sv.join.address)); ln.app.showPassiveNotification("Copied", 2000) }
    }
    GameServerIconButton {
        visible: !!ln.sv && !!ln.sv.join && ln.sv.join.password !== null && ln.sv.join.password !== undefined
        side: 20
        app: ln.app
        iconName: "lock-symbolic"
        label: "Copy password"
        glyph: ln.textColor
        enabled: !ln.preview
        onClicked: if (ln.app && ln.sv && ln.sv.join.password) { ln.app.copyToClipboard(String(ln.sv.join.password)); ln.app.showPassiveNotification("Copied", 2000) }
    }
}
