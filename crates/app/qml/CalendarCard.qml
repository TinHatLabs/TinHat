// CalendarCard.qml — 0.8.33: the LIVE calendar card inside a chat bubble.
//
// A calendar announcement message carries `dev.tinhat.calendar` {container, event} (see
// calendar.rs `announce_content`). When that event lives in the calendar the app is mirroring
// right now (the room's space — always the case in the room the announcement was posted in),
// the card shows the event as it is NOW: title, time in the reader's own zone, who is going,
// with Going / Maybe / Can't answerable in place and a click anywhere else opening the calendar
// on that event. Anywhere else (another space's calendar in a floating chat, an event that has
// been removed) it is a compact "open the calendar" card. Stock clients see the markdown
// summary the message also carries; this client hides that summary while the live card shows.
// The request (2026-09-10): "make it aware that it's a calendar invite and when you click on it it
// opens that event card to where they can rsvp and stuff".
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the calendar.card.* tokens

Rectangle {
    id: card
    required property var app        // Main.qml root (mCalEvents / mCalContainer) — may be null
    required property var theme      // app or a FallbackTheme (sizes and motion; colours are Th's calendar.card.*)
    property string container: ""    // the calendar container (space / room) the event lives in
    property string eventId: ""      // dev.tinhat.calendar.event state key
    property bool mine: false        // drawn on my own (accent) bubble → onAccent text
    property real maxW: 320
    signal openRequested()
    signal answered(string status)   // "yes" | "maybe" | "no" (toggles off when repeated)

    // The event as the calendar mirror has it, or null when this card can't be live.
    readonly property var live: {
        if (eventId.length === 0 || !app || app.mCalEvents === undefined) return null
        if (String(app.mCalContainer || "") !== container) return null
        var evs = app.mCalEvents
        for (var i = 0; i < evs.length; i++) if (evs[i].id === eventId) return evs[i]
        return null
    }
    readonly property bool canceled: live ? live.canceled === true : false
    readonly property bool answerable: live !== null && !canceled && live.kind !== "focus" && live.past !== true
    readonly property string myStatus: (live && live.my) ? String(live.my.status || "") : ""
    readonly property color stripe: (live && live.gameColor) ? live.gameColor : Th.calendarCardStripe
    readonly property color fg: mine ? Th.calendarCardFgOwn : Th.calendarCardFg
    readonly property color fgDim: mine ? Th.calendarCardMetaFgOwn : Th.calendarCardMetaFg

    // 11.1 the Game servers link (docs/GAMESERVERS.md §5): the server the event's game links to, under the time — its
    // state and join address. The status is polled only while such a card is on screen (a non-member's 403 rests it).
    readonly property var gsCtl: app ? app.gameServersCtl : null
    readonly property bool wantServer: !!card.gsCtl && card.live !== null && !card.canceled && (!!card.live.game || !!card.live.gameId)
    property bool _gsHeld: false
    function _holdServer(on) {
        if (!card.gsCtl || on === card._gsHeld) return
        card._gsHeld = on
        if (on) card.gsCtl.acquire(); else card.gsCtl.release()
    }
    onWantServerChanged: card._holdServer(card.wantServer)
    Component.onCompleted: card._holdServer(card.wantServer)
    Component.onDestruction: card._holdServer(false)
    readonly property string gsText: card.gsCtl ? String(card.gsCtl.statusJson) : ""
    readonly property var linked: {
        if (!card.wantServer || card.gsText.length === 0) return null
        var id = String(card.gsCtl.linkServer(card.gsText, JSON.stringify({ game: card.live.game || "", gameId: card.live.gameId || "" })))
        if (id.length === 0) return null
        var v = null
        try { v = JSON.parse(card.gsText) } catch (e) { return null }
        var list = v && v.servers ? v.servers : []
        for (var i = 0; i < list.length; ++i) if (list[i].id === id) return list[i]
        return null
    }

    // Times are epoch ms; formatted HERE, in the reader's zone (never on the Rust side).
    function fmtWhen(ev) {
        if (!ev) return ""
        if (ev.kind === "focus")
            return Qt.formatDate(new Date(ev.effStart), "MMM d") + " – " + Qt.formatDate(new Date(ev.effEnd), "MMM d")
        if (ev.kind === "proposal" && ev.chosen < 0) {
            var n = ev.options ? ev.options.length : 0
            return n > 0 ? "Vote for a time — " + n + " option" + (n === 1 ? "" : "s") : "Vote for a time"
        }
        if (!ev.effStart) return ""
        var s = new Date(ev.effStart)
        var t = Qt.formatDate(s, "ddd, MMM d") + " · " + Qt.formatTime(s, "h:mm AP")
        if (ev.end && ev.end > ev.effStart) t += " – " + Qt.formatTime(new Date(ev.end), "h:mm AP")
        return t + " " + Qt.formatTime(s, "t")   // 10.1 (TH-CAL-07): the event's zone, not now's
    }
    function kindLabel(ev) {
        switch (ev.kind) {
        case "freeplay": return "Free play"
        case "milestone": return "Scheduled progression — join if you can"
        case "proposal": return ev.chosen >= 0 ? "Time locked in" : "Proposal — vote for a time"
        case "cooloff": return "Cool-off game"
        case "focus": return "Main game"
        default: return "Group session"
        }
    }
    function count(ev, k) { return (ev && ev.rsvp && ev.rsvp[k]) ? ev.rsvp[k].length : 0 }

    implicitWidth: Math.min(maxW, 400)
    implicitHeight: col.implicitHeight + 24
    radius: 12
    color: mine ? Th.calendarCardBgOwn : Th.calendarCardBg
    border.width: 1
    border.color: hover.hovered ? Th.calendarCardBorderHover : Th.calendarCardBorder
    Behavior on border.color { ColorAnimation { duration: Motion.dur } }
    HoverHandler { id: hover; cursorShape: Qt.PointingHandCursor }
    // The buttons take their own presses; a tap anywhere else on the card opens the calendar.
    TapHandler { onTapped: card.openRequested() }
    Rectangle { // game-colour stripe (the calendar's chip colour for this game)
        anchors { left: parent.left; top: parent.top; bottom: parent.bottom; margins: 1 }
        width: 5; radius: 3
        color: card.stripe
    }
    ColumnLayout {
        id: col
        anchors { left: parent.left; right: parent.right; top: parent.top; margins: 12; leftMargin: 18 }
        spacing: 4
        RowLayout {
            Layout.fillWidth: true
            spacing: 6
            Controls.Label { text: "📅"; font.pixelSize: 14 }
            Controls.Label {
                Layout.fillWidth: true
                text: card.live ? card.kindLabel(card.live) : "Calendar event"
                color: card.fgDim
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                font.bold: true
                elide: Text.ElideRight
                opacity: 0.9
            }
            Controls.Label {
                visible: card.canceled
                text: "CANCELLED"
                color: Th.calendarCardCancelledFg
                font.bold: true
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
        Controls.Label {
            visible: card.live !== null
            Layout.fillWidth: true
            text: card.live ? (card.live.title + (card.live.game ? "  ·  " + card.live.game : "")) : ""
            color: card.fg
            font.bold: true
            font.pointSize: Kirigami.Theme.defaultFont.pointSize + 1
            font.strikeout: card.canceled
            wrapMode: Text.WordWrap
        }
        Controls.Label {
            visible: card.live !== null && text.length > 0
            Layout.fillWidth: true
            text: card.fmtWhen(card.live)
            color: card.fg
            opacity: 0.92
            wrapMode: Text.WordWrap
        }
        GameServerLine {   // 11.1 the Game servers link
            visible: !!card.linked
            Layout.fillWidth: true
            app: card.app
            sv: card.linked
            textColor: card.fgDim
            strongColor: card.fg
        }
        Controls.Label {
            visible: card.live !== null && !!card.live.notes
            Layout.fillWidth: true
            text: (card.live && card.live.notes) ? card.live.notes : ""
            color: card.fgDim
            wrapMode: Text.WordWrap
            maximumLineCount: 3
            elide: Text.ElideRight
            font.pointSize: Kirigami.Theme.smallFont.pointSize
        }
        Controls.Label {
            visible: card.live === null
            Layout.fillWidth: true
            text: "Open the calendar for the details and to RSVP."
            color: card.fgDim
            wrapMode: Text.WordWrap
            font.pointSize: Kirigami.Theme.smallFont.pointSize
        }
        Controls.Label {
            visible: card.live !== null && !card.canceled && card.live.kind !== "focus"
            Layout.fillWidth: true
            text: {
                var y = card.count(card.live, "yes"), m = card.count(card.live, "maybe")
                var parts = []
                if (y > 0) parts.push(y + " going")
                if (m > 0) parts.push(m + " maybe")
                return parts.length > 0 ? parts.join(" · ") : "Nobody has answered yet"
            }
            color: card.fgDim
            font.pointSize: Kirigami.Theme.smallFont.pointSize
            elide: Text.ElideRight
        }
        RowLayout {
            visible: card.answerable
            Layout.fillWidth: true
            Layout.topMargin: 2
            spacing: 6
            Repeater {
                model: [ { s: "yes", t: "Going" }, { s: "maybe", t: "Maybe" }, { s: "no", t: "Can't" } ]
                delegate: ThButton {
                    app: card.app;
                    required property var modelData
                    text: (card.myStatus === modelData.s ? "✓ " : "") + modelData.t
                    flat: card.myStatus !== modelData.s
                    highlighted: card.myStatus === modelData.s
                    padding: 6; leftPadding: 10; rightPadding: 10
                    onClicked: card.answered(modelData.s)
                }
            }
            Item { Layout.fillWidth: true }
            ThButton {
                app: card.app; text: "Open"; flat: true; padding: 6; leftPadding: 10; rightPadding: 10; onClicked: card.openRequested() }
        }
        ThButton {
            app: card.app;
            visible: !card.answerable
            Layout.alignment: Qt.AlignRight
            text: "Open calendar"
            flat: true
            padding: 6; leftPadding: 10; rightPadding: 10
            onClicked: card.openRequested()
        }
    }
}
