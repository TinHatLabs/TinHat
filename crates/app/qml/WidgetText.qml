// WidgetText.qml — widgets (modular UI step 2): one text of the widgets' type ramp (contract WIDGETS-UI.md §3). Every
// size is a multiple of the app's body font (Kirigami.Theme.defaultFont — never smallFont):
//   title 1.0 × 600 · text 1.0 × 400 · strong 1.0 × 600 · meta 0.85 × 400 · num 0.85 × 400 tabular figures ·
//   label 0.75 × 700 UPPERCASE +0.06 em · chip 0.75 × 600 · date 1.1 × 700 · dow 0.7 × 700 UPPERCASE +0.04 em
// Plain text only (names and statuses are other people's text), one line, elided.
import QtQuick
import QtQuick.Window
import org.kde.kirigami as Kirigami

Text {
    id: t
    property string role: "text"
    property bool semibold: false             // "W-meta 600" and friends
    readonly property real body: Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10
    readonly property real factor: role === "meta" || role === "num" ? 0.85
        : role === "label" || role === "chip" ? 0.75
        : role === "date" ? 1.1
        : role === "dow" ? 0.7 : 1.0
    readonly property real pxPerPt: Screen.logicalPixelDensity > 0 ? Screen.logicalPixelDensity * 25.4 / 72 : 96 / 72
    font.family: Kirigami.Theme.defaultFont.family
    font.pointSize: t.body * t.factor
    font.weight: (t.role === "label" || t.role === "date" || t.role === "dow") ? Font.Bold
        : (t.semibold || t.role === "title" || t.role === "strong" || t.role === "chip") ? Font.DemiBold : Font.Normal
    font.capitalization: (t.role === "label" || t.role === "dow") ? Font.AllUppercase : Font.MixedCase
    font.letterSpacing: t.role === "label" ? t.body * t.factor * t.pxPerPt * 0.06
        : t.role === "dow" ? t.body * t.factor * t.pxPerPt * 0.04 : 0
    font.features: t.role === "num" ? { "tnum": 1 } : ({})
    textFormat: Text.PlainText
    elide: Text.ElideRight
    maximumLineCount: 1
    verticalAlignment: Text.AlignVCenter
}
