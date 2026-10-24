// SettingsRowGroup.qml — several Settings rows that live in another component (LayoutSection, …) but sit in the
// card like its own rows: the group counts as a row for its siblings' dividers, and its first visible row takes the
// divider the group would have (SettingsRow.topDivider).
import QtQuick
import QtQuick.Layouts

ColumnLayout {
    id: grp
    readonly property bool isSettingsRow: true
    readonly property bool isSettingsRowGroup: true
    readonly property bool groupTopDivider: {
        var p = grp.parent
        if (!p) return false
        var kids = p.children
        for (var i = 0; i < kids.length; i++) {
            if (kids[i] === grp) return p.isSettingsRowGroup === true ? p.groupTopDivider === true : false
            if (kids[i].visible && kids[i].isSettingsRow === true) return true
        }
        return false
    }
    Layout.fillWidth: true
    spacing: 0
}
