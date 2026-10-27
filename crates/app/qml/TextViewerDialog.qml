// TextViewerDialog — a text-like attachment (plain text, JSON, CSV, Markdown, logs, source; viewer.rs text_like) shown
// IN THE APP as plain text (launch-gate's web review #8, 2026-10-26: the web opens the same files as text/plain in a
// tab). Never handed to an outside opener: Rust downloads at most viewer::TEXT_VIEW_MAX, checks the bytes are plain
// text (the `txt` sniff) and hands over the text (RoomController.mediaText); here it is read-only PlainText, so nothing
// in it can render as markup or load anything. `show(json)` takes mediaText's {id, name, text}.
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // LinkGate

Controls.Dialog {
    id: dlg
    property string fileName: ""
    property string body: ""

    function show(json) {
        var v = null
        try { v = JSON.parse(String(json || "")) } catch (e) { v = null }
        if (!v) return
        dlg.fileName = String(v.name || "")
        dlg.body = String(v.text || "")
        dlg.open()
    }

    title: LinkGate.plain(dlg.fileName.length > 0 ? dlg.fileName : "Text file")
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    // fixed sizes from the window, never from the text (a long file mustn't grow the dialog)
    width: Math.min(900, (parent ? parent.width : 900) - Kirigami.Units.gridUnit * 2)
    height: Math.min(700, (parent ? parent.height : 700) - Kirigami.Units.gridUnit * 2)
    standardButtons: Controls.Dialog.Close
    onClosed: dlg.body = ""   // don't keep someone's file in memory

    Controls.ScrollView {
        anchors.fill: parent
        clip: true
        Controls.TextArea {
            textFormat: TextEdit.PlainText   // never markup, whatever the file holds
            readOnly: true
            selectByMouse: true
            wrapMode: TextEdit.WrapAnywhere
            font: Kirigami.Theme.fixedWidthFont
            text: dlg.body
            Accessible.name: "Contents of " + dlg.fileName
        }
    }
}
