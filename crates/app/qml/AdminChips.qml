// AdminChips.qml — games round 2, F8 (GAMES-V1.1-ADDENDUM §5): the admin money screen for ONE space (the space whose
// chips the call plays for). Shown only to the people the space owner's server calls admins (GET /admin/spaces/{space}
// → you.admin: the space's admin power level, or the server owner); every change is the server's journal entry + a log
// row (who, what, when, before/after, why). Three cards: Economy (per-space rates with their source + Reset), People
// (search, multi-select / Everyone, Give · Take · Set · Reset · Rank, an amount, a reason, then a confirm sheet that
// says exactly what will happen), Log (newest first, Load more). Words: words.json table.r2 admin_*. Play money only:
// no currency signs anywhere. Tin Hat Web's admin tab draws the same.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Rectangle {
    id: ad
    property var app: null
    property var play: null
    property string spaceId: ""
    property string spaceName: ""
    // Space settings → Games: the space may not be this call's: the requests go to ITS owner (playApiAt)
    property bool atSpace: false
    property bool embedded: false   // inside a settings page: no card frame, no Close
    signal done()
    readonly property var r2: ad.app && ad.app.playWords && ad.app.playWords.table && ad.app.playWords.table.r2 ? ad.app.playWords.table.r2 : ({})
    function say(k, map) { var s = String(ad.r2[k] !== undefined ? ad.r2[k] : k); for (var x in (map || {})) s = s.split("{" + x + "}").join(String(map[x])); return s }
    readonly property real fBody: (Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10) * Th.textScale
    function nameOf(u) { return ad.app && ad.app.localpart ? String(ad.app.localpart(u)) : String(u) }
    readonly property string enc: encodeURIComponent(ad.spaceId)

    // the economy keys, their bounds (the server is the word; the fields can't go outside them)
    readonly property var keys: [
        { k: "chip_minutes", min: 2, max: 60 }, { k: "day_max", min: 1, max: 200 }, { k: "start", min: 0, max: 500 },
        { k: "topup_to", min: 0, max: 100 }, { k: "cap_base", min: 20, max: 100000 }, { k: "cap_per_rank", min: 0, max: 100000 },
        { k: "side_day_max", min: 0, max: 100 }, { k: "side_on", bool: true }
    ]
    property var info: null           // the GET answer
    property var edits: ({})          // key → new value
    property var resets: ({})         // key → true
    property var log: []
    property bool logMore: true
    property var picked: ({})         // mxid → true
    property bool everyone: false
    property string action: "grant"
    // the amount always inside the chosen action's bounds (the field's clamp doesn't emit valueModified)
    function amountMin(a) { return a === "set" || a === "rank" ? 0 : 1 }
    function amountMax(a) { return a === "set" ? 100000 : (a === "rank" ? 50 : 10000) }
    onActionChanged: ad.amount = Math.max(ad.amountMin(ad.action), Math.min(ad.amountMax(ad.action), ad.amount))
    property int amount: 5
    property string reason: ""
    property string filter: ""
    property string err: ""
    property string okLine: ""
    property var pending: ({})        // request id → kind

    // a contract op id (16–32 url-safe chars) from Rust: the server refuses anything else (lab7, round 2)
    function opId() { return ad.play ? String(ad.play.newOpId()) : "" }
    function req(kind, method, path, body) {
        if (!ad.play) return
        var id = kind + "-" + ad.opId()
        var p = Object.assign({}, ad.pending); p[id] = kind; ad.pending = p
        var sent = ad.atSpace ? ad.play.playApiAt(id, ad.spaceId, method, path, body ? JSON.stringify(body) : "")
                              : ad.play.playApi(id, method, path, body ? JSON.stringify(body) : "")
        if (!sent) { ad.err = ad.say("admin_error"); delete ad.pending[id] }
    }
    function load() { ad.req("get", "GET", "/admin/spaces/" + ad.enc, null); ad.log = []; ad.logMore = true; ad.loadLog() }
    function loadLog() {
        var last = ad.log.length > 0 ? ad.log[ad.log.length - 1].id : 0
        ad.req("log", "GET", "/admin/spaces/" + ad.enc + "/log" + (last ? "?before=" + last + "&limit=50" : "?limit=50"), null)
    }
    Component.onCompleted: ad.load()
    Connections {
        target: ad.play
        ignoreUnknownSignals: true
        function onApiRevChanged() {
            var a; try { a = JSON.parse(ad.play.apiJson) } catch (e) { return }
            var kind = a && ad.pending[a.id]
            if (!kind) return
            var p = Object.assign({}, ad.pending); delete p[a.id]; ad.pending = p
            if (!a.ok) { ad.err = ad.say("admin_code_" + (a.code || "other")); if (ad.err.indexOf("admin_code_") === 0) ad.err = ad.say("admin_error"); return }
            ad.err = ""
            var b = a.body || {}
            if (kind === "get") { ad.info = b; ad.edits = ({}); ad.resets = ({}) }
            else if (kind === "log") { ad.log = ad.log.concat(b.entries || []); ad.logMore = (b.entries || []).length >= 50 }
            else if (kind === "economy") { ad.okLine = ad.say("admin_saved"); ad.load() }
            else if (kind === "wallets") {
                var sk = (b.skipped || []).length
                ad.okLine = ad.say("admin_done", { n: (b.done || []).length }) + (sk ? " · " + ad.say("admin_skipped", { n: sk }) : "")
                ad.picked = ({}); ad.everyone = false; ad.load()
            }
        }
    }
    readonly property bool isAdmin: !!ad.info && !!ad.info.you && ad.info.you.admin === true
    readonly property var econ: ad.info && ad.info.economy ? ad.info.economy : ({})
    function valueOf(k) { return ad.edits[k] !== undefined ? ad.edits[k] : (ad.econ[k] ? ad.econ[k].value : undefined) }
    readonly property int nPicked: { var n = 0; for (var k in ad.picked) if (ad.picked[k]) n++; return n }
    readonly property var wallets: {
        var w = ad.info && ad.info.wallets ? ad.info.wallets : [], f = ad.filter.toLowerCase(), out = []
        for (var i = 0; i < w.length; ++i) if (f === "" || ad.nameOf(w[i].u).toLowerCase().indexOf(f) >= 0 || String(w[i].u).toLowerCase().indexOf(f) >= 0) out.push(w[i])
        return out
    }
    readonly property bool needsAmount: ad.action !== "reset"
    // the confirm sheet's sentence: exactly what will happen
    readonly property string confirmLine: {
        var who = ad.everyone ? ad.say("admin_everyone") : (ad.nPicked === 1 ? (function () { for (var k in ad.picked) if (ad.picked[k]) return ad.nameOf(k); return "" })() : ad.say("admin_n_people", { n: ad.nPicked }))
        var line = ad.say("admin_confirm_" + ad.action, { n: ad.amount, who: who })
        var atTable = []
        var w = ad.info && ad.info.wallets ? ad.info.wallets : []
        for (var i = 0; i < w.length; ++i) if (w[i].at_table && (ad.everyone || ad.picked[w[i].u])) atTable.push(ad.nameOf(w[i].u))
        if (atTable.length > 0) line += " " + (ad.action === "grant" ? ad.say("admin_at_table_grant", { who: atTable.join(", ") }) : ad.say("admin_at_table_skip", { who: atTable.join(", ") }))
        if (ad.reason !== "") line += " (" + ad.reason + ")"
        return line
    }
    function doWallets() {
        var who = []
        for (var k in ad.picked) if (ad.picked[k]) who.push(k)
        var body = { op: ad.opId(), action: ad.action, who: ad.everyone ? "everyone" : who }
        if (ad.needsAmount) body.amount = ad.amount
        if (ad.reason !== "") body.reason = ad.reason.substring(0, 140)
        ad.req("wallets", "POST", "/admin/spaces/" + ad.enc + "/wallets", body)
    }
    function doEconomy() {
        var set = ({}), reset = [], any = false
        for (var k in ad.edits) { set[k] = ad.edits[k]; any = true }
        for (var r in ad.resets) if (ad.resets[r]) { reset.push(r); any = true }
        if (!any) return
        ad.req("economy", "PUT", "/admin/spaces/" + ad.enc + "/economy", { op: ad.opId(), set: set, reset: reset })
    }
    function when(ms) { var d = new Date(ms); var t = Qt.formatTime(d, "hh:mm"); return (new Date().toDateString() === d.toDateString() ? ad.say("admin_today") : Qt.formatDate(d, "MMM d")) + " " + t }
    function logLine(e) {
        var who = e.who === "everyone" ? ad.say("admin_everyone") : (e.who ? ad.nameOf(e.who) : "")
        var line = ad.say("admin_log_" + e.action, { by: ad.nameOf(e.by), who: who, n: e.amount !== undefined && e.amount !== null ? e.amount : "" })
        return ad.when(e.at) + " · " + line + (e.reason ? " · " + e.reason : "")
    }

    objectName: "adminChips"
    color: ad.embedded ? "transparent" : Th.surfaceRaised
    radius: 12
    border.width: ad.embedded ? 0 : 1
    border.color: Th.borderSubtle
    MouseArea { anchors.fill: parent; z: -1; acceptedButtons: Qt.AllButtons; onWheel: function (w) { w.accepted = false } }
    Keys.onEscapePressed: confirm.visible ? confirm.visible = false : ad.done()

    component Card: Rectangle {
        default property alias content: cardCol.data
        property string title: ""
        Layout.fillWidth: true
        implicitHeight: cardCol.implicitHeight + 24
        radius: 10
        color: Th.surfaceOverlay
        border.width: 1
        border.color: Th.borderSubtle
        ColumnLayout {
            id: cardCol
            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
            anchors.margins: 12
            spacing: 8
            Controls.Label { text: parent.parent.title; textFormat: Text.PlainText; font.bold: true; color: Th.textPrimary; font.pointSize: ad.fBody }
        }
    }
    component Small: Controls.Label { textFormat: Text.PlainText; color: Th.textSecondary; font.pointSize: ad.fBody * 0.85 }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 14
        spacing: 10
        RowLayout {
            Layout.fillWidth: true
            Controls.Label { text: ad.say("admin_title", { space: ad.spaceName }); textFormat: Text.PlainText; font.bold: true; font.pointSize: ad.fBody * 1.15; color: Th.textPrimary; Layout.fillWidth: true; elide: Text.ElideRight }
            ThButton { objectName: "adminClose"; visible: !ad.embedded; app: ad.app; flat: true; text: ad.say("hands_close"); onClicked: ad.done() }
        }
        Small { visible: ad.err !== ""; text: ad.err; color: Th.statusErrorFg; Layout.fillWidth: true; wrapMode: Text.WordWrap }
        Small { visible: ad.okLine !== "" && ad.err === ""; text: ad.okLine; color: Th.accentBright; Layout.fillWidth: true }
        Small { visible: !!ad.info && !ad.isAdmin; text: ad.say("admin_not_admin"); Layout.fillWidth: true }
        Controls.ScrollView {
            id: sv
            visible: ad.isAdmin
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            ColumnLayout {
                width: sv.availableWidth
                spacing: 10
                // ── 1. Economy ──
                Card {
                    title: ad.say("admin_economy")
                    Small {
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        text: ad.say("admin_earn_line", { m: ad.valueOf("chip_minutes") || 10, d: ad.valueOf("day_max") || 24 })
                    }
                    Repeater {
                        model: ad.keys
                        RowLayout {
                            id: er
                            required property var modelData
                            readonly property string k: er.modelData.k
                            readonly property var cur: ad.econ[er.k] || ({})
                            readonly property string src: ad.resets[er.k] ? "default" : (ad.edits[er.k] !== undefined ? "space" : String(er.cur.source || "default"))
                            Layout.fillWidth: true
                            spacing: 8
                            Controls.Label { text: ad.say("admin_key_" + er.k); textFormat: Text.PlainText; color: Th.textPrimary; font.pointSize: ad.fBody * 0.92; Layout.fillWidth: true }
                            Controls.SpinBox {
                                visible: !er.modelData.bool
                                objectName: "adminEcon-" + er.k
                                from: er.modelData.min || 0; to: er.modelData.max || 100
                                editable: true
                                value: Number(ad.valueOf(er.k) || 0)
                                onValueModified: { var e = Object.assign({}, ad.edits); e[er.k] = value; ad.edits = e; var r = Object.assign({}, ad.resets); delete r[er.k]; ad.resets = r }
                                Accessible.name: ad.say("admin_key_" + er.k)
                            }
                            Controls.Switch {
                                visible: !!er.modelData.bool
                                checked: ad.valueOf(er.k) === true
                                onToggled: { var e = Object.assign({}, ad.edits); e[er.k] = checked; ad.edits = e; var r = Object.assign({}, ad.resets); delete r[er.k]; ad.resets = r }
                                Accessible.name: ad.say("admin_key_" + er.k)
                            }
                            Rectangle {   // where the value comes from
                                radius: height / 2
                                color: er.src === "space" ? Th.accentDim : Th.surfaceSunken
                                implicitWidth: srcL.implicitWidth + 12
                                implicitHeight: srcL.implicitHeight + 4
                                Controls.Label { id: srcL; anchors.centerIn: parent; text: ad.say("admin_src_" + er.src); textFormat: Text.PlainText; color: Th.textSecondary; font.pointSize: ad.fBody * 0.72 }
                            }
                            ThButton {
                                app: ad.app; flat: true; implicitHeight: 26
                                enabled: er.src === "space"
                                text: ad.say("admin_reset_key")
                                onClicked: { var r = Object.assign({}, ad.resets); r[er.k] = true; ad.resets = r; var e = Object.assign({}, ad.edits); delete e[er.k]; ad.edits = e }
                            }
                        }
                    }
                    ThButton {
                        objectName: "adminSaveEconomy"
                        app: ad.app
                        Layout.alignment: Qt.AlignRight
                        highlighted: true
                        enabled: Object.keys(ad.edits).length > 0 || Object.keys(ad.resets).length > 0
                        text: ad.say("admin_save")
                        onClicked: ad.doEconomy()
                    }
                }
                // ── 2. People ──
                Card {
                    title: ad.say("admin_people")
                    RowLayout {
                        Layout.fillWidth: true
                        Controls.TextField {
                            Layout.fillWidth: true
                            placeholderText: ad.say("admin_search")
                            onTextChanged: ad.filter = text
                            Accessible.name: placeholderText
                        }
                        Controls.CheckBox {
                            objectName: "adminEveryone"
                            text: LinkGate.plain(ad.say("admin_everyone"))
                            checked: ad.everyone
                            onToggled: ad.everyone = checked
                        }
                    }
                    Repeater {
                        model: ad.wallets
                        RowLayout {
                            id: pr
                            required property var modelData
                            Layout.fillWidth: true
                            spacing: 6
                            opacity: ad.everyone ? 0.6 : 1
                            Controls.CheckBox {
                                enabled: !ad.everyone
                                checked: !!ad.picked[pr.modelData.u]
                                onToggled: { var p = Object.assign({}, ad.picked); p[pr.modelData.u] = checked; ad.picked = p }
                                Accessible.name: ad.nameOf(pr.modelData.u)
                            }
                            PrestigeToken { mxid: String(pr.modelData.u || ""); roomId: ad.spaceId; gap: false; Layout.rightMargin: -2 }
                            Controls.Label {
                                text: ad.nameOf(pr.modelData.u)
                                textFormat: Text.PlainText; color: Th.textPrimary; elide: Text.ElideRight
                                Layout.fillWidth: true
                                font.pointSize: ad.fBody * 0.92
                            }
                            Rectangle {
                                visible: pr.modelData.at_table === true
                                radius: height / 2; color: Th.surfaceSunken
                                implicitWidth: atL.implicitWidth + 10; implicitHeight: atL.implicitHeight + 4
                                Controls.Label { id: atL; anchors.centerIn: parent; text: ad.say("admin_at_table"); textFormat: Text.PlainText; color: Th.textSecondary; font.pointSize: ad.fBody * 0.72 }
                            }
                            Small { text: "● " + pr.modelData.balance + "  ·  " + ad.say("admin_rank_n", { n: pr.modelData.rank || 0 }) }
                        }
                    }
                    RowLayout {   // the action bar
                        Layout.fillWidth: true
                        spacing: 6
                        Repeater {
                            model: ["grant", "remove", "set", "reset", "rank"]
                            ThButton {
                                required property var modelData
                                objectName: "adminAct-" + modelData
                                app: ad.app
                                implicitHeight: 28
                                highlighted: ad.action === modelData
                                flat: ad.action !== modelData
                                text: ad.say("admin_act_" + modelData)
                                onClicked: ad.action = modelData
                            }
                        }
                        Controls.SpinBox {
                            objectName: "adminAmount"
                            visible: ad.needsAmount
                            from: ad.amountMin(ad.action)
                            to: ad.amountMax(ad.action)
                            editable: true
                            value: ad.amount
                            onValueModified: ad.amount = value
                            Accessible.name: ad.say("admin_amount")
                        }
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        Controls.TextField {
                            Layout.fillWidth: true
                            maximumLength: 140
                            placeholderText: ad.say("admin_reason")
                            onTextChanged: ad.reason = text
                            Accessible.name: placeholderText
                        }
                        ThButton {
                            objectName: "adminApply"
                            app: ad.app
                            highlighted: true
                            enabled: ad.everyone || ad.nPicked > 0
                            text: ad.say("admin_apply")
                            onClicked: confirm.visible = true
                        }
                    }
                }
                // ── 3. Log ──
                Card {
                    title: ad.say("admin_log")
                    Small { visible: ad.log.length === 0; text: ad.say("admin_log_empty") }
                    Repeater {
                        model: ad.log
                        Small { required property var modelData; Layout.fillWidth: true; wrapMode: Text.WordWrap; color: Th.textPrimary; text: ad.logLine(modelData) }
                    }
                    ThButton {
                        app: ad.app; flat: true
                        visible: ad.logMore && ad.log.length > 0
                        text: ad.say("admin_more")
                        onClicked: ad.loadLog()
                    }
                }
            }
        }
    }
    // the confirm sheet: exactly what will happen, then Do it / Cancel
    Rectangle {
        id: confirm
        objectName: "adminConfirm"
        visible: false
        anchors.fill: parent
        radius: ad.radius
        color: Th.scrim
        MouseArea { anchors.fill: parent; onClicked: confirm.visible = false }
        Rectangle {
            anchors.centerIn: parent
            width: Math.min(420, parent.width - 40)
            height: cc.implicitHeight + 28
            radius: 10
            color: Th.surfaceRaised
            border.width: 1; border.color: Th.borderSubtle
            MouseArea { anchors.fill: parent }
            ColumnLayout {
                id: cc
                anchors.fill: parent; anchors.margins: 14
                spacing: 10
                Controls.Label { objectName: "adminConfirmLine"; Layout.fillWidth: true; wrapMode: Text.WordWrap; textFormat: Text.PlainText; color: Th.textPrimary; font.pointSize: ad.fBody; text: ad.confirmLine }
                RowLayout {
                    Layout.alignment: Qt.AlignRight
                    ThButton { app: ad.app; flat: true; text: ad.say("admin_cancel"); onClicked: confirm.visible = false }
                    ThButton { objectName: "adminDoIt"; app: ad.app; highlighted: true; text: ad.say("admin_do_it"); onClicked: { confirm.visible = false; ad.doWallets() } }
                }
            }
        }
    }
}
