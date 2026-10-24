// DiceWidget.qml — widgets W3 (2026-09-28): Dice (`dice`, contract WIDGETS.md §16, WIDGETS-UI.md §16; the web's
// features/widgets/dice/Dice.tsx).
//
// Roll for the open room or just for you: d4…d100 buttons (M / L), an expression field with Enter to roll and the
// Advantage / Disadvantage chips (L), the last roll as a card with the total big (DiceCard: it lands once when it is a
// roll you just made), and the history — the room's rolls from its loaded timeline (RoomController.diceJson: the
// dice messages, decrypted) plus your private rolls (this device only, marked Private), newest first: M the last 5, L 15;
// click one to roll it again. S / collapsed: "Ana · 2d6+3 → 9" + a d20 button. The header's Room | Private switch is the
// widget setting `private`; Room is off when no room is open or you may not send messages there (RoomController
// .canSendMessage) — then rolls are private. A room roll goes out through RoomController.sendDice (the local-echo path).
// Data: DiceController (rolling with the OS CSPRNG, the private list, the shared history). No timer of its own.
// Gate / review: TINHAT_WIDGETS_SCENARIO "dice=<key>" — data, many, nat1, private, empty, noroom — the web's samples.
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
    readonly property var room: w.app ? w.app.room : null
    readonly property int limit: w.size === "L" && !w.preview ? 15 : 5

    DiceController { id: diceCtl }
    Component.onCompleted: { diceCtl.start(); w.prevKeys = w.keys }
    Component.onDestruction: diceCtl.stop()

    // ── the model (the web's useDiceModel) ──
    readonly property string roomId: w.room ? String(w.room.roomId || "") : ""
    readonly property string me: w.app ? String(w.app.mUserId || "") : ""
    readonly property var roomRows: {
        var raw = w.room ? String(w.room.diceJson || "") : ""
        try { return raw.length > 0 ? JSON.parse(raw) : [] } catch (e) { return [] }
    }
    // the rows' names / avatars / send state by id (the shared history keeps the id)
    readonly property var rowInfo: {
        var m = {}
        for (var i = 0; i < w.roomRows.length; ++i) m[w.roomRows[i].id] = w.roomRows[i]
        return m
    }
    readonly property var realEntries: {
        var p = diceCtl.privateJson   // re-read when the private list changes
        var raw = String(diceCtl.history(JSON.stringify(w.roomRows), 15) || "[]")
        try { return JSON.parse(raw) } catch (e) { return [] }
    }
    readonly property var realModel: ({
        room: w.roomId,
        me: w.me,
        canRoom: w.roomId.length > 0 && !!w.room && w.room.canSendMessage === true,
        wantPrivate: w.settings && w.settings["private"] === true,
        entries: w.realEntries,
        hasPrivate: String(diceCtl.privateJson || "[]") !== "[]",
        sample: false
    })
    readonly property bool hasData: w.realModel.entries.length > 0
    readonly property string scenario: w.wc ? String(w.wc.scenarioOf("dice") || "") : ""
    readonly property bool sample: w.scenario.length > 0 || (w.preview && !w.hasData)
    readonly property var model: w.sample ? w.sampleModel(w.scenario.length > 0 ? w.scenario : "preview") : w.realModel
    readonly property var shown: w.model.entries.slice(0, w.limit)
    readonly property var rest: w.shown.slice(1)
    readonly property bool live: false
    readonly property bool toRoom: !w.model.wantPrivate && w.model.canRoom && w.model.room.length > 0
    readonly property bool isPrivate: !w.toRoom

    // ── who / when (the web's Who / When) ──
    function whoOf(e) {
        if (!e) return ""
        if (e["private"]) return "Private"
        if (e.sender === w.model.me) return "You"
        if (w.sample) return w.sampleName(e.sender)
        var info = w.rowInfo[e.id]
        var n = info ? String(info.senderName || "") : ""
        return n.length > 0 ? n : String(e.sender).replace(/^@/, "").split(":")[0]
    }
    function stateOf(e) { var info = (!w.sample && e) ? w.rowInfo[e.id] : null; return info ? (info.failed ? "failed" : info.pending ? "pending" : "") : "" }
    function whenOf(e) {
        var st = w.stateOf(e)
        if (st === "failed") return "Not sent"
        if (st === "pending") return "Sending…"
        if (!w.wc || !e) return ""
        var now = Date.now()
        return String(w.wc.whenLabel(e.ts, now, -new Date(now).getTimezoneOffset(), w.timeOf(e.ts)))
    }
    // the app's clock setting (12 h / 24 h); "System" = the locale's short time WITHOUT seconds (Up next's timeOf)
    function timeOf(ms) {
        var d = new Date(ms)
        var mode = w.app ? String(w.app.clockMode || "") : ""
        if (mode === "12") return Qt.formatTime(d, "h:mm AP")
        if (mode === "24") return Qt.formatTime(d, "HH:mm")
        return Qt.formatTime(d, String(Qt.locale().timeFormat(Locale.ShortFormat)).replace(/[:.]ss/, ""))
    }

    // ── actions ──
    /// roll `expr` where rolls go now; → "" or the expression's friendly error
    function roll(expr) {
        if (w.preview || w.sample) return ""
        if (w.toRoom) {
            var r = {}
            try { r = JSON.parse(String(diceCtl.rollForRoom(expr))) } catch (e) { r = { ok: false, message: "Couldn't roll" } }
            if (!r.ok) return String(r.message || "")
            w.room.sendDice(JSON.stringify(r.content))
            return ""
        }
        return String(diceCtl.rollPrivate(expr) || "")
    }
    function setPrivate(v) {
        if (w.preview || w.sample || !w.hostRef) return
        var i = w.hostRef.indexOfId("dice")
        if (i >= 0) w.hostRef.setSetting(i, "private", v)
    }
    /// the card lands when its roll is the one I just made here (same expression + dice, within a minute)
    readonly property var just: { var k = diceCtl.justKey; try { return JSON.parse(String(diceCtl.justJson || "null")) } catch (e) { return null } }
    function playKeyOf(e) {
        if (!e || !w.just || w.sample) return -1
        var mine = e["private"] === true || e.sender === w.model.me
        var rs = e.result.rolls || []
        var same = e.result.expr === w.just.expr && rs.length === w.just.rolls.length && rs.every(function (v, i) { return v === w.just.rolls[i] })
        return mine && same && Date.now() - w.just.at < 60000 ? diceCtl.justKey : -1
    }

    readonly property string summaryText: {
        var e = w.model.entries.length > 0 ? w.model.entries[0] : null
        return String(diceCtl.summary(e ? JSON.stringify(e) : "", e ? w.whoOf(e) : ""))
    }
    // S / collapsed: the line + a d20 button (the header's own button: a click rolls, it doesn't fold the widget)
    readonly property Component summaryComponent: Component {
        Item {
            id: sumItem
            implicitWidth: sumMeasure.implicitWidth + 6 + d20.width
            implicitHeight: 20
            WidgetText { id: sumMeasure; visible: false; role: "meta"; text: w.summaryText }
            WidgetText {
                width: Math.max(0, sumItem.width - 6 - d20.width)
                anchors.verticalCenter: parent.verticalCenter
                role: "meta"
                text: w.summaryText
                color: Th.widgetSummaryFg
            }
            DiceButton {
                id: d20
                visible: !w.preview
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: w.preview ? 0 : d20Label.implicitWidth + 12
                height: 20
                radius: 6
                tip: "Roll a d20" + (w.toRoom ? "" : " privately")
                accessibleName: "Roll a d20"
                onClicked: w.roll("1d20")
                WidgetText { id: d20Label; anchors.centerIn: parent; role: "chip"; text: "d20"; color: Th.widgetDiceButtonFg }
            }
        }
    }
    // M / L header: the Room | Private switch (the widget setting `private`)
    readonly property Component extraComponent: Component {
        Rectangle {
            implicitWidth: segRow.implicitWidth + 2
            implicitHeight: 20
            radius: 7
            color: Th.widgetDiceTargetBg
            Accessible.role: Accessible.Grouping
            Accessible.name: "Where your rolls go"
            Row {
                id: segRow
                x: 1; y: 1
                Segment {
                    text: "Room"
                    chosen: !w.isPrivate
                    enabled: w.model.room.length > 0 && w.model.canRoom
                    tip: w.model.room.length === 0 ? "Open a room to roll there" : !w.model.canRoom ? "You can't send messages in this room" : "Everyone in the room sees your rolls"
                    onClicked: w.setPrivate(false)
                }
                Segment {
                    text: "Private"
                    chosen: w.isPrivate
                    tip: "Only you see your rolls (kept on this device)"
                    onClicked: w.setPrivate(true)
                }
            }
        }
    }

    // rowSlide: a roll that newly enters the rows under the card (after the first fill)
    readonly property var keys: w.rest.map(function (e) { return String(e.id) })
    property var prevKeys: null
    onKeysChanged: Qt.callLater(function () { w.prevKeys = w.keys })
    function isFresh(k) { return w.prevKeys !== null && w.prevKeys.indexOf(k) < 0 }

    implicitHeight: bodyCol.implicitHeight

    Column {
        id: bodyCol
        x: 6; width: parent.width - 12
        spacing: 6
        // ── quick dice: 7 × (268 − 30) / 7, gap 5, 30 high ──
        Row {
            id: diceRow
            width: parent.width
            spacing: 5
            Accessible.role: Accessible.Grouping
            Accessible.name: "Quick dice"
            Repeater {
                model: [4, 6, 8, 10, 12, 20, 100]
                delegate: DiceButton {
                    required property int modelData
                    width: (diceRow.width - 6 * 5) / 7
                    height: 30
                    radius: 8
                    tip: "Roll 1d" + modelData + (w.toRoom ? "" : " privately")
                    accessibleName: "Roll a d" + modelData
                    onClicked: w.roll("1d" + modelData)
                    WidgetText { anchors.centerIn: parent; role: "strong"; text: "d" + parent.modelData; color: Th.widgetDiceButtonFg }
                }
            }
        }
        // ── L: the expression, its error, the chips ──
        Column {
            id: exprBlock
            visible: w.size === "L" && !w.preview
            width: parent.width
            spacing: 6
            property string error: ""
            RowLayout {
                width: parent.width
                spacing: 6
                ThTextField {
                    id: exprField
                    Layout.fillWidth: true
                    placeholderText: "2d6+3 · 4d6kh3 · 2d20kh1"
                    maximumLength: 100
                    Accessible.name: "Dice to roll"
                    onTextEdited: exprBlock.error = ""
                    onAccepted: exprBlock.go()
                }
                DiceButton {
                    Layout.preferredWidth: 52
                    Layout.preferredHeight: 34
                    radius: 8
                    tip: w.toRoom ? "Roll to the room" : "Roll privately"
                    accessibleName: "Roll"
                    onClicked: exprBlock.go()
                    WidgetText { anchors.centerIn: parent; role: "strong"; text: "Roll"; color: Th.widgetDiceButtonFg }
                }
            }
            function go() {
                var err = w.roll(exprField.text)
                exprBlock.error = err
                if (err.length === 0 && !w.sample) {
                    var c = String(diceCtl.canonical(exprField.text) || "")
                    if (c.length > 0) exprField.text = c
                }
            }
            WidgetText {
                visible: exprBlock.error.length > 0
                width: parent.width
                role: "meta"
                text: exprBlock.error
                color: Th.widgetDiceErrorFg
                Accessible.role: Accessible.AlertMessage
            }
            Row {
                spacing: 6
                Repeater {
                    model: [{ k: true, t: "Advantage" }, { k: false, t: "Disadvantage" }]
                    delegate: DiceButton {
                        required property var modelData
                        readonly property string expr: String(diceCtl.withAdvantage(exprField.text, modelData.k))
                        width: chipLabel.implicitWidth + 20
                        height: 24
                        radius: 12
                        tip: "Roll " + expr
                        accessibleName: modelData.t
                        onClicked: { exprField.text = expr; exprBlock.error = w.roll(expr) }
                        WidgetText { id: chipLabel; anchors.centerIn: parent; role: "chip"; text: parent.modelData.t; color: Th.widgetDiceButtonFg }
                    }
                }
            }
        }
        // ── the last roll ──
        DiceCard {
            visible: w.shown.length > 0
            width: parent.width
            app: w.app
            variant: "widget"
            r: w.shown.length > 0 ? w.shown[0].result : null
            headWho: w.shown.length > 0 ? w.whoOf(w.shown[0]) : ""
            headWhen: w.shown.length > 0 ? w.whenOf(w.shown[0]) : ""
            headPrivate: w.shown.length > 0 && w.shown[0]["private"] === true
            headFailed: w.shown.length > 0 && w.stateOf(w.shown[0]) === "failed"
            playKey: w.playKeyOf(w.shown.length > 0 ? w.shown[0] : null)
            animate: !!w.hostRef && w.hostRef.animate
        }
        // ── empty ──
        Item {
            visible: w.shown.length === 0
            width: parent.width; height: visible ? Math.max(56, emptyCol.implicitHeight) : 0
            Column {
                id: emptyCol
                anchors.centerIn: parent
                spacing: 2
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "text"; text: "No rolls yet"; color: Th.widgetMetaFg }
                WidgetText {
                    anchors.horizontalCenter: parent.horizontalCenter
                    role: "meta"
                    text: w.toRoom ? "Pick a die — everyone here sees the roll." : "Pick a die — only you see the roll."
                    color: Th.widgetMutedFg
                }
            }
        }
        // ── the history under the card ──
        Column {
            visible: w.rest.length > 0
            width: parent.width
            Repeater {
                model: w.rest.length
                delegate: HistoryRow { required property int index; width: bodyCol.width; e: w.rest[index] }
            }
        }
        // ── Clear private rolls ──
        Item {
            visible: w.model.hasPrivate && !w.preview
            width: parent.width; height: visible ? 20 : 0
            Controls.AbstractButton {
                id: clearBtn
                anchors.right: parent.right
                height: 20
                width: clearText.implicitWidth + 16
                hoverEnabled: true
                focusPolicy: Qt.TabFocus
                Accessible.name: "Clear private rolls"
                background: Rectangle { radius: 6; color: "transparent"; border.width: clearBtn.visualFocus ? 1 : 0; border.color: Th.widgetRingFocus }
                contentItem: Item {
                    WidgetText {
                        id: clearText
                        anchors.centerIn: parent
                        role: "meta"
                        text: "Clear private rolls"
                        font.underline: clearBtn.hovered
                        color: clearBtn.hovered ? Th.widgetMetaFg : Th.widgetMutedFg
                    }
                }
                onClicked: if (!w.sample) diceCtl.clearPrivate()
            }
        }
    }

    // ── a history row: 24, padding 0 8, radius 6: who · expression · total · when; click = roll it again ──
    component HistoryRow: Rectangle {
        id: hr
        property var e: null
        readonly property string key: hr.e ? String(hr.e.id) : ""
        readonly property string nat: hr.e ? String(hr.e.result.natural || "") : ""
        onKeyChanged: if (w.isFresh(hr.key)) hrIn.play()
        Component.onCompleted: if (w.isFresh(hr.key)) hrIn.play()
        SlideIn { id: hrIn; target: hr }
        height: 24
        radius: 6
        color: hrMa.containsMouse && !w.preview ? Th.widgetRowBgHover : "transparent"
        border.width: hrMa.activeFocus ? 1 : 0
        border.color: Th.widgetRingFocus
        Accessible.role: Accessible.Button
        Accessible.name: (hr.e && hr.e["private"] ? "Private" : "") + " roll " + (hr.e ? hr.e.result.expr + ": " + hr.e.result.total : "") + ". Roll it again"
        MouseArea {
            id: hrMa
            anchors.fill: parent
            enabled: !w.preview
            hoverEnabled: true
            activeFocusOnTab: !w.preview
            cursorShape: Qt.PointingHandCursor
            onClicked: if (hr.e) w.roll(hr.e.result.expr)
            Keys.onPressed: (k) => { if ((k.key === Qt.Key_Return || k.key === Qt.Key_Enter || k.key === Qt.Key_Space) && hr.e) { w.roll(hr.e.result.expr); k.accepted = true } }
            Controls.ToolTip.text: hr.e ? "Roll " + hr.e.result.expr + " again" : ""
            Controls.ToolTip.visible: containsMouse && !w.preview
            Controls.ToolTip.delay: 500
        }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 8
            spacing: 6
            ThIcon {
                visible: !!hr.e && hr.e["private"] === true
                Layout.preferredWidth: 12; Layout.preferredHeight: 12
                Layout.alignment: Qt.AlignVCenter
                app: w.app
                name: "lock-symbolic"
                color: Th.widgetMutedFg
            }
            WidgetText {
                Layout.fillWidth: true
                role: "text"
                text: w.whoOf(hr.e)
                color: hr.e && hr.e["private"] ? Th.widgetMutedFg : Th.widgetFg
            }
            WidgetText {
                Layout.maximumWidth: (hr.width - 16) * 0.4
                role: "num"
                text: hr.e ? hr.e.result.expr : ""
                color: Th.widgetMetaFg
            }
            WidgetText {
                Layout.minimumWidth: 26
                horizontalAlignment: Text.AlignRight
                role: "strong"
                font.features: ({ "tnum": 1 })
                text: hr.e ? String(hr.e.result.total) : ""
                color: hr.nat === "nat20" ? Th.widgetDiceNat20Fg : hr.nat === "nat1" ? Th.widgetDiceNat1Fg : Th.widgetFg
            }
            WidgetText {
                Layout.preferredWidth: 54
                horizontalAlignment: Text.AlignRight
                role: "num"
                text: w.whenOf(hr.e)
                color: w.stateOf(hr.e) === "failed" ? Th.widgetDiceErrorFg : Th.widgetMetaFg
            }
        }
    }

    // a small flat button of the widget (dice, Roll, the chips, the S line's d20): widget.dice.button.* + a tooltip
    component DiceButton: Controls.AbstractButton {
        id: db
        property real radius: 8
        property string tip: ""
        property string accessibleName: ""
        hoverEnabled: true
        focusPolicy: Qt.TabFocus
        enabled: !w.preview
        Accessible.name: db.accessibleName
        Controls.ToolTip.text: db.tip
        Controls.ToolTip.visible: db.hovered && db.tip.length > 0
        Controls.ToolTip.delay: 500
        background: Rectangle {
            radius: db.radius
            color: (db.hovered || db.pressed) ? Th.widgetDiceButtonBgHover : Th.widgetDiceButtonBg
            border.width: db.visualFocus ? 1 : 0
            border.color: Th.widgetRingFocus
        }
    }

    // a side of the Room | Private switch: 18 high, padding 0 8, radius 6, W-chip
    component Segment: Controls.AbstractButton {
        id: sg
        property bool chosen: false
        property string tip: ""
        height: 18
        width: segLabel.implicitWidth + 16
        hoverEnabled: true
        focusPolicy: Qt.TabFocus
        checkable: false
        opacity: sg.enabled ? 1 : 0.45
        Accessible.role: Accessible.Button
        Accessible.name: sg.text
        Accessible.checked: sg.chosen
        Controls.ToolTip.text: sg.tip
        Controls.ToolTip.visible: sg.hovered && !w.preview
        Controls.ToolTip.delay: 500
        background: Rectangle {
            radius: 6
            color: sg.chosen ? Th.widgetDiceTargetBgChecked : "transparent"
            border.width: sg.visualFocus ? 1 : 0
            border.color: Th.widgetRingFocus
        }
        contentItem: Item {
            WidgetText { id: segLabel; anchors.centerIn: parent; role: "chip"; text: sg.text; color: sg.chosen ? Th.widgetDiceTargetFgChecked : Th.widgetDiceTargetFg }
        }
    }

    component SlideIn: ParallelAnimation {
        id: si
        property Item target
        property Translate shift: Translate { y: 0 }
        Component.onCompleted: if (si.target) si.target.transform = [si.shift]
        NumberAnimation { target: si.target; property: "opacity"; from: 0; to: 1; duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic }
        NumberAnimation { target: si.shift; property: "y"; from: 4; to: 0; duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic }
        function play() { if (w.hostRef && w.hostRef.durLong > 0) { si.stop(); si.start() } }
    }

    // ── the sample states (the gallery's preview, the review screenshots, the gates): the web's sampleEntries ──
    function sampleName(mxid) {
        var n = { "@ana:sample.invalid": "Ana", "@bo:sample.invalid": "Bo", "@cy:sample.invalid": "Cy", "@dee:sample.invalid": "Dee",
                  "@eli:sample.invalid": "Eli", "@fay:sample.invalid": "Fay", "@gus:sample.invalid": "Gus", "@hal:sample.invalid": "Hal" }
        return n[mxid] || String(mxid).replace(/^@/, "").split(":")[0]
    }
    function localAt(daysAgo, h, m) {
        var d = new Date()
        d.setDate(d.getDate() - daysAgo)
        d.setHours(h, m, 0, 0)
        return d.getTime()
    }
    function sampleModel(key) {
        var sp = function (id) { return "@" + id + ":sample.invalid" }
        var you = sp("you")
        var mk = function (id, who, ts, expr, rolls, priv) {
            var t = String(diceCtl.sampleEntry(id, who, ts, expr, JSON.stringify(rolls), priv === true))
            try { return JSON.parse(t) } catch (e) { return null }
        }
        var all = [
            mk("$d1", sp("ana"), w.localAt(0, 20, 42), "2d20kh1+5", [7, 20]),
            mk("$d2", you, w.localAt(0, 20, 40), "4d6kh3", [3, 6, 3, 2]),
            mk("p1", "", w.localAt(0, 20, 38), "1d100", [42], true),
            mk("$d3", sp("bo"), w.localAt(0, 20, 35), "2d6+3", [4, 2]),
            mk("$d4", sp("cy"), w.localAt(0, 20, 31), "1d20-1", [1]),
            mk("$d5", sp("ana"), w.localAt(0, 20, 30), "1d8+1d4+2", [6, 3]),
            mk("$d6", sp("dee"), w.localAt(0, 20, 12), "3d6", [5, 5, 1]),
            mk("$d7", you, w.localAt(0, 19, 58), "1d12", [9]),
            mk("$d8", sp("bo"), w.localAt(1, 21, 15), "2d20kl1+2", [14, 3]),
            mk("$d9", sp("eli"), w.localAt(1, 21, 5), "1d20+7", [13]),
            mk("p2", "", w.localAt(1, 20, 50), "1d4", [3], true),
            mk("$d10", sp("fay"), w.localAt(1, 20, 44), "8d6", [1, 2, 3, 4, 5, 6, 6, 6]),
            mk("$d11", sp("gus"), w.localAt(2, 19, 30), "1d10", [10]),
            mk("$d12", sp("hal"), w.localAt(3, 22, 0), "1d6", [4]),
            mk("$d13", sp("ana"), w.localAt(4, 18, 0), "2d10+4", [7, 7]),
            mk("$d14", sp("bo"), w.localAt(5, 18, 0), "1d20", [11])
        ]
        if (key === "nat1") all = [mk("$n1", sp("cy"), w.localAt(0, 20, 45), "2d20kl1+4", [1, 12])].concat(all)
        var count = key === "many" ? 16 : (key === "empty" || key === "noroom") ? 0 : 6
        var entries = all.slice(0, count)
        return {
            room: key === "noroom" ? "" : "!lounge:sample.invalid",
            me: you,
            canRoom: key !== "noroom",
            wantPrivate: key === "private",
            entries: entries,
            hasPrivate: entries.some(function (e) { return e && e["private"] === true }),
            sample: true
        }
    }
}
