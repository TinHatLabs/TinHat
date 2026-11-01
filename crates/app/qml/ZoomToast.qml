// ZoomToast.qml — the App size toast (2026-10-31, docs/UI-ZOOM.md): "App size 120% · Ctrl+0 resets" for 1.8 s after
// Ctrl+= / Ctrl+- / Ctrl+0, centred 12 below the top of the main window, the house toast's look (ThemeTryBar.qml: the
// Kirigami corner radius, toast.bg at 0.9, toast.fg, a toast.shadow shadow 2 / 9). ONE toast: a held key or quick
// presses change its text and restart its timer instead of stacking Kirigami's passive notifications. No animation
// (it appears and goes), so a held key repaints only this text. show(text).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: toast.*

Item {
    id: zt
    property string text: ""
    function show(t) { zt.text = String(t || ""); zt.visible = zt.text.length > 0; hideTimer.restart() }

    visible: false
    width: Math.min(label.implicitWidth + 32, (parent ? parent.width : 400) - 24)
    height: 36
    x: ((parent ? parent.width : width) - width) / 2
    y: 12
    Accessible.role: Accessible.StatusBar
    Accessible.name: zt.text

    Timer { id: hideTimer; interval: 1800; onTriggered: zt.visible = false }
    Kirigami.ShadowedRectangle {
        anchors.fill: parent
        radius: Kirigami.Units.cornerRadius
        color: Th.toastBg
        opacity: 0.9
        shadow.size: 9
        shadow.yOffset: 2
        shadow.color: Th.toastShadow
    }
    Controls.Label {
        id: label
        anchors.centerIn: parent
        width: Math.min(implicitWidth, zt.width - 32)
        textFormat: Text.PlainText
        text: zt.text
        color: Th.toastFg
        elide: Text.ElideRight
        font.features: ({ "tnum": 1 })
    }
}
