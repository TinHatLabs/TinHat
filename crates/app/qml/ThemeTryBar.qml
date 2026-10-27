// ThemeTryBar.qml — "Trying ‹name› · ‹n› s" + Keep + Undo while a shared theme is on trial (11.0 step 5, contract
// THEMES-UI.md §14.4; the web's TryBar.tsx): centred at the bottom of the window, 12 above its edge, 44 high, padding
// 0 8 0 16, the house toast's look (the Kirigami corner radius, toast.bg at 0.9 under the content, toast.fg, a
// toast.shadow shadow 2 / 9). The number changes once a second — the only thing that moves (GPU.md). Undo or Esc goes
// back now. `trial` = AppCore.themeTrial ({key, docJson, name, left}).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: toast.*

FocusScope {
    id: bar
    property var app: null
    property var trial: null
    signal keep()
    signal undo()

    visible: bar.trial !== null
    width: Math.min(row.implicitWidth + 24, (parent ? parent.width : 400) - 24)
    height: 44
    x: ((parent ? parent.width : width) - width) / 2
    y: (parent ? parent.height : height) - 12 - height
    Accessible.role: Accessible.StatusBar
    Keys.onEscapePressed: (e) => { bar.undo(); e.accepted = true }

    Kirigami.ShadowedRectangle {
        anchors.fill: parent
        radius: Kirigami.Units.cornerRadius
        color: Th.toastBg
        opacity: 0.9
        shadow.size: 9
        shadow.yOffset: 2
        shadow.color: Th.toastShadow
    }
    RowLayout {
        id: row
        anchors.fill: parent
        anchors.leftMargin: 16
        anchors.rightMargin: 8
        spacing: 4
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            Layout.fillWidth: true
            Layout.rightMargin: 8
            text: bar.trial ? "Trying " + bar.trial.name + " · " + bar.trial.left + " s" : ""
            color: Th.toastFg
            elide: Text.ElideRight
            font.features: ({ "tnum": 1 })
        }
        ThButton { app: bar.app; flat: true; text: "Keep"; onClicked: bar.keep() }
        ThButton { app: bar.app; flat: true; text: "Undo"; onClicked: bar.undo() }
    }
}
