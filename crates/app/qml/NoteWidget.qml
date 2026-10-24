// NoteWidget.qml — widgets W1 (2026-09-28): Sticky note (`note`, contract WIDGETS.md §14, WIDGETS-UI.md §14; the web's
// features/widgets/note/Note.tsx).
//
// One shared note per space — "Tonight: bring potions" — on a faint sticky-note paper: the space selected on the rail,
// else (Home, Favourites, People) the space of the open room. M the note (up to 6 lines), L the whole note, S /
// collapsed its first line. Under it the author's avatar + "Ana · edited 2 h ago". With `edit_space_note`
// (NoteController.canEdit: the space's roles, else its power levels) a click edits it inline: the house text area, a
// counter "n / 500" (amber from 450), Cancel / Save (Ctrl+Enter saves, Escape cancels; an empty note clears it).
// Links (http / https) are clickable. A change cross-fades (noteFade: `long`, OutCubic). No timer.
// Gate / review: TINHAT_WIDGETS_SCENARIO "note=<key>" (data, long, empty, readonly, emptyro, nospace, edit) — the web's
// samples, the same text.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: w
    property var app
    property var hostRef
    property string size: "M"
    property var settings: ({})
    property bool preview: false
    readonly property var wc: w.hostRef ? w.hostRef.widgets : null
    readonly property bool large: w.size === "L" && !w.preview

    NoteController { id: noteCtl }

    // ── the space: the rail's space, else the open room's ──
    readonly property string railKey: w.app && w.app.nav ? String(w.app.nav.activeSpace || "") : ""
    readonly property string roomId: w.app && w.app.room ? String(w.app.room.roomId || "") : ""
    readonly property string spaceKey: {
        if (w.railKey.charAt(0) === "!") return w.railKey
        if (!w.roomId || !w.app || !w.app.nav) return ""
        var k = String(w.app.nav.spaceOf(w.roomId) || "")
        return k.charAt(0) === "!" ? k : ""
    }
    onSpaceKeyChanged: if (!w.sample) { w.editing = false; noteCtl.show(w.spaceKey) }
    Component.onCompleted: if (!w.sample) noteCtl.show(w.spaceKey)
    Component.onDestruction: noteCtl.close()

    // ── the model (the web's useNote) ──
    readonly property var realNote: {
        var raw = String(noteCtl.noteJson || "")
        if (raw.length === 0) return { ready: false, space: w.spaceKey, spaceName: "", text: null, canEdit: false }
        try { var o = JSON.parse(raw); o.ready = true; return o } catch (e) { return { ready: true, space: "", text: null, canEdit: false } }
    }
    readonly property bool hasData: !!w.realNote.space && w.realNote.text !== null && w.realNote.text !== undefined
    readonly property string scenario: w.wc ? String(w.wc.scenarioOf("note") || "") : ""
    readonly property bool sample: w.scenario.length > 0 || (w.preview && !w.hasData)
    readonly property var note: w.sample ? w.sampleNote(w.scenario.length > 0 ? w.scenario : "preview") : w.realNote
    readonly property bool live: false
    readonly property bool hasText: typeof w.note.text === "string" && w.note.text.length > 0

    readonly property string summaryText: !w.note.space ? "No space" : !w.note.ready ? "…"
        : (w.hasText ? String(noteCtl.firstLine(w.note.text)) : "No note yet")
    readonly property Component summaryComponent: Component { WidgetHeaderText { text: w.summaryText } }
    // M / L header: the space the note belongs to
    readonly property Component extraComponent: Component { WidgetHeaderText { text: w.note.space ? String(w.note.spaceName || "") : "" } }

    // ── editing ──
    property bool editing: w.sample && w.scenario === "edit"
    property string draft: w.sample && w.scenario === "edit" ? w.note.text : ""
    function startEdit() {
        if (w.preview || w.sample || !w.note.canEdit) return
        w.draft = w.hasText ? w.note.text : ""
        w.editing = true
        Qt.callLater(function () { if (editor.item) { editor.item.forceActiveFocus(); editor.item.cursorPosition = editor.item.length } })
    }
    function cancelEdit() { w.editing = false }
    function saveEdit() {
        if (!w.editing || noteCtl.saving) return
        var c = JSON.parse(String(noteCtl.counter(w.draft)))
        if (c.over) return
        noteCtl.save(w.draft)
        w.editing = false
    }
    readonly property var counter: {
        var d = w.draft
        try { return JSON.parse(String(noteCtl.counter(d))) } catch (e) { return { count: 0, max: 500, near: false, over: false } }
    }
    function agoText(ts) { return w.wc && ts > 0 ? "edited " + String(w.wc.fmtAgo(Date.now() - ts)) : "" }
    // the note as rich text: each segment escaped, links as anchors, line breaks kept (note::note_segments)
    function richOf(text) {
        var segs = []
        try { segs = JSON.parse(String(noteCtl.segments(text || ""))) } catch (e) { segs = [] }
        var esc = function (s) { return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/\n/g, "<br>") }
        return segs.map(function (s) { return s.url ? "<a href=\"" + esc(s.url) + "\">" + esc(s.text) + "</a>" : esc(s.text) }).join("")
    }

    // noteFade: the note's text cross-fades on a change (old out, new in; `long`, OutCubic); the first fill snaps
    property string shownText: w.hasText ? w.note.text : ""
    property string fadingText: ""
    onShownTextChanged: {
        var d = w.hostRef ? w.hostRef.durLong : 0
        var last = w._lastText
        w._lastText = w.shownText
        if (!w._filled || d <= 0 || last.length === 0) { w._filled = true; w.fadingText = ""; return }
        fadeAnim.stop()
        w.fadingText = last
        fadeAnim.duration = d
        fadeAnim.start()
    }
    property string _lastText: ""
    property bool _filled: false

    implicitHeight: bodyCol.implicitHeight

    Column {
        id: bodyCol
        x: 6; width: parent.width - 12
        spacing: 0
        // no space
        WidgetText {
            visible: !w.note.space
            width: parent.width; height: visible ? 44 : 0
            horizontalAlignment: Text.AlignHCenter
            role: "meta"
            text: "Pick a space to see its note"
            color: Th.widgetMutedFg
        }
        // reading
        WidgetText {
            visible: !!w.note.space && !w.note.ready
            width: parent.width; height: visible ? 44 : 0
            horizontalAlignment: Text.AlignHCenter
            role: "meta"
            text: "Reading the note…"
            color: Th.widgetMutedFg
        }
        // no note yet
        Item {
            visible: !!w.note.space && w.note.ready && !w.hasText && !w.editing
            width: parent.width; height: visible ? Math.max(56, emptyCol.implicitHeight + 16) : 0
            Column {
                id: emptyCol
                anchors.centerIn: parent
                spacing: 4
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "text"; text: "No note yet"; color: Th.widgetMetaFg }
                ThButton {
                    visible: !!w.note.canEdit
                    anchors.horizontalCenter: parent.horizontalCenter
                    app: w.app
                    flat: true
                    text: "Add a note"
                    icon.name: "list-add-symbolic"
                    implicitHeight: 26
                    enabled: !w.preview
                    onClicked: w.startEdit()
                }
            }
        }
        // the note: the paper (radius 8, a 3 px edge) with the text, then the author line
        Rectangle {
            id: paper
            visible: !!w.note.space && w.note.ready && w.hasText && !w.editing
            width: parent.width
            // the text's height for the paper's width (known once laid out: a 0-wide first pass would measure one glyph
            // per line). Taken over one event-loop turn later: bound directly, the text's height → the body's height →
            // the frame's scroll bar → the paper's width → the text's height made a binding loop (Basic style).
            property real textH: 0
            function syncH() { Qt.callLater(function () { paper.textH = noteText.implicitHeight }) }
            height: visible && width > 24 && textH > 0 ? Math.ceil(textH) + 16 : 0
            radius: 8
            color: Th.widgetNotePaperBg
            Rectangle { x: 0; y: 8; width: 3; height: parent.height - 16; radius: 2; color: Th.widgetNotePaperEdge }
            HoverHandler { id: paperHover; enabled: !w.preview && !!w.note.canEdit; cursorShape: Qt.PointingHandCursor }
            TapHandler { enabled: !w.preview && !!w.note.canEdit; onTapped: w.startEdit() }
            Text {
                id: noteText
                x: 12; y: 8
                width: parent.width - 24
                text: w.richOf(w.shownText)
                textFormat: Text.StyledText
                wrapMode: Text.Wrap
                maximumLineCount: w.large ? 100000 : 6
                elide: Text.ElideRight
                color: Th.widgetFg
                linkColor: Th.widgetNoteLinkFg
                font.family: Kirigami.Theme.defaultFont.family
                font.pointSize: Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10
                onLinkActivated: (link) => { if (!w.preview && !w.sample) Qt.openUrlExternally(link) }
                onImplicitHeightChanged: paper.syncH()
                Component.onCompleted: paper.syncH()
                HoverHandler { cursorShape: noteText.hoveredLink ? Qt.PointingHandCursor : (paperHover.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor) }
            }
            // the old text fading out over the new (noteFade)
            Text {
                id: fadeText
                x: 12; y: 8
                width: parent.width - 24
                visible: w.fadingText.length > 0
                text: w.richOf(w.fadingText)
                textFormat: Text.StyledText
                wrapMode: Text.Wrap
                maximumLineCount: w.large ? 100000 : 6
                elide: Text.ElideRight
                color: Th.widgetFg
                linkColor: Th.widgetNoteLinkFg
                font: noteText.font
                opacity: 0
            }
            ParallelAnimation {
                id: fadeAnim
                property int duration: 200
                NumberAnimation { target: fadeText; property: "opacity"; from: 1; to: 0; duration: fadeAnim.duration; easing.type: Easing.OutCubic }
                NumberAnimation { target: noteText; property: "opacity"; from: 0; to: 1; duration: fadeAnim.duration; easing.type: Easing.OutCubic }
                onFinished: w.fadingText = ""
            }
        }
        // the author line: avatar 16 · "Ana · edited 2 h ago" · (Edit)
        Item {
            visible: paper.visible
            width: parent.width; height: visible ? 26 : 0
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 4; anchors.rightMargin: 2
                spacing: 6
                Avatar {
                    Layout.preferredWidth: 16; Layout.preferredHeight: 16
                    Layout.alignment: Qt.AlignVCenter
                    app: w.app
                    size: 16
                    seed: String(w.note.sender || "")
                    name: String(w.note.name || "")
                    imageSource: String(w.note.avatar || "")
                }
                WidgetText { Layout.fillWidth: true; role: "meta"; text: String(w.note.name || "") + " · " + w.agoText(w.note.ts || 0); color: Th.widgetMetaFg }
                ThButton {
                    visible: !!w.note.canEdit
                    app: w.app
                    flat: true
                    text: "Edit"
                    implicitHeight: 22
                    leftPadding: 8; rightPadding: 8
                    font.pointSize: (Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10) * 0.85
                    enabled: !w.preview
                    onClicked: w.startEdit()
                }
            }
        }
        // editing: the house text area, the counter, Cancel / Save
        Loader {
            id: editor
            active: w.editing
            visible: active
            width: parent.width
            height: active && item ? item.implicitHeight : 0
            sourceComponent: Component {
                ThTextArea {
                    width: editor.width
                    text: w.draft
                    placeholderText: "Tonight: bring potions…"
                    padding: 8
                    readOnly: w.preview
                    onTextChanged: w.draft = text
                    Keys.onPressed: (e) => {
                        if ((e.key === Qt.Key_Return || e.key === Qt.Key_Enter) && (e.modifiers & Qt.ControlModifier)) { w.saveEdit(); e.accepted = true }
                        else if (e.key === Qt.Key_Escape) { w.cancelEdit(); e.accepted = true }
                    }
                }
            }
        }
        Item {
            visible: w.editing
            width: parent.width; height: visible ? 34 : 0
            RowLayout {
                anchors.fill: parent
                anchors.topMargin: 6
                spacing: 6
                WidgetText {
                    Layout.fillWidth: true
                    role: "num"
                    text: w.counter.count + " / " + w.counter.max
                    color: w.counter.near ? Th.widgetNoteCounterLimitFg : Th.widgetMutedFg
                }
                ThButton { app: w.app; flat: true; text: "Cancel"; implicitHeight: 26; enabled: !w.preview; onClicked: w.cancelEdit() }
                ThButton { app: w.app; highlighted: true; text: "Save"; implicitHeight: 26; enabled: !w.preview && !w.counter.over && !noteCtl.saving; onClicked: w.saveEdit() }
            }
        }
        // a failed Save
        WidgetText {
            visible: String(noteCtl.error || "").length > 0 && !w.sample
            width: parent.width; height: visible ? 22 : 0
            role: "meta"
            text: String(noteCtl.error || "")
            color: Th.widgetNoteCounterLimitFg
        }
    }

    // ── the sample states (the gallery's preview, the review screenshots, the gates): the web's samples ──
    function localAt(daysAgo, h, m) {
        var d = new Date()
        d.setDate(d.getDate() - daysAgo)
        d.setHours(h, m, 0, 0)
        return d.getTime()
    }
    function sampleNote(key) {
        // edited 2 h ago (fmtAgo floors to the hour: the line reads the same for the whole hour after any capture)
        var base = { ready: true, space: "!squad:sample.invalid", spaceName: "Squad chat", sender: "@ana:sample.invalid", name: "Ana", avatar: "", ts: Date.now() - 2 * 3600000 - 60000, canEdit: key !== "readonly" && key !== "emptyro" }
        var shortText = "Tonight: bring potions 🧪\nRaid at 9 PM — Dee tanks, Bo heals.\nRoute: https://example.com/raid"
        var longText = shortText + "\n\nAfter the raid: loot rolls in #loot, no ninja-ing.\nSaturday: Valheim night, bring your own mead.\nIf you're late, ping me and I'll hold the portal.\nLast one out turns off the lights."
        if (key === "nospace") return { ready: true, space: "", text: null, canEdit: false }
        if (key === "empty" || key === "emptyro") { base.text = null; return base }
        base.text = key === "long" ? longText : shortText
        return base
    }
}
