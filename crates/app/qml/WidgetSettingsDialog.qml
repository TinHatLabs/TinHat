// WidgetSettingsDialog.qml — a widget's Settings… (contract WIDGETS-UI.md §2; the web's SettingsDialog in Frame.tsx):
// the house Dialog "<title> — settings" (width 360, Close) with one control per registry setting — int → a combo box
// of min…max with the unit ("7 days", "1 day"), bool → a check box — saved at once (the host writes the list).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.Dialog {
    id: dlg
    property var app
    property var hostRef
    property int index: -1
    readonly property var reg: (dlg.hostRef && dlg.index >= 0 && dlg.index < dlg.hostRef.items.length) ? dlg.hostRef.regOf(dlg.hostRef.items[dlg.index].w) : null
    readonly property var values: (dlg.hostRef && dlg.index >= 0) ? (dlg.hostRef.settingsOf[dlg.index] || ({})) : ({})
    function openFor(i) { dlg.index = i; dlg.open() }
    title: LinkGate.plain(dlg.reg ? dlg.reg.title + " — settings" : "Settings")
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(360, (parent ? parent.width : 360) - Kirigami.Units.gridUnit * 2)
    standardButtons: Controls.Dialog.Close
    contentItem: ColumnLayout {
        spacing: 12
        Repeater {
            model: dlg.reg ? (dlg.reg["settings"] || []) : []
            delegate: Item {
                id: st
                required property var modelData
                Layout.fillWidth: true
                implicitHeight: st.modelData.type === "bool" ? check.implicitHeight : row.implicitHeight
                ThCheckBox {
                    id: check
                    visible: st.modelData.type === "bool"
                    width: parent.width
                    text: st.modelData.label || st.modelData.id
                    checked: dlg.values[st.modelData.id] === true
                    onToggled: dlg.hostRef.setSetting(dlg.index, st.modelData.id, checked)
                }
                RowLayout {
                    id: row
                    visible: st.modelData.type === "int"
                    width: parent.width
                    spacing: 12
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.fillWidth: true
                        text: st.modelData.label || st.modelData.id
                        color: Th.widgetFg
                        wrapMode: Text.Wrap
                    }
                    ThComboBox {
                        id: combo
                        readonly property var opts: {
                            var a = []
                            if (st.modelData.type !== "int") return a
                            for (var v = st.modelData.min; v <= st.modelData.max; ++v) a.push(v)
                            return a
                        }
                        model: combo.opts.map(function (v) { return (v + " " + (st.modelData.unit || "")).trim().replace(/^1 days$/, "1 day") })
                        currentIndex: Math.max(0, combo.opts.indexOf(Number(dlg.values[st.modelData.id])))
                        onActivated: (i) => dlg.hostRef.setSetting(dlg.index, st.modelData.id, combo.opts[i])
                    }
                }
            }
        }
    }
}
