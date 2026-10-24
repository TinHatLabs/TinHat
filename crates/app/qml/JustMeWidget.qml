// JustMeWidget.qml — widgets W2: "Is it just me?" (`justMe`, contract WIDGETS.md §20, WIDGETS-UI.md §20; the web's
// features/widgets/justme/JustMe.tsx). Is the server down, or is it your connection? A green / amber / red light per
// service measured from THIS device — Chat (the homeserver + whether sync is flowing), Calls (the call JWT service),
// Media (the media repo), the Tin Hat server — plus your ping (the median of 3 requests to the homeserver).
// M = a row per service ("Calls · reachable · 84 ms") + the friendly sentence when something is red + "Checked 2:41 PM ·
// Check again"; L adds the last 10 minutes as a small bar sparkline per row (redrawn only when a check lands); S /
// collapsed = the verdict ("All good · 42 ms", "Calls look down").
//
// Data: the host's JustMeController (justme_ctl.rs: the checker — 60 s focused, 5 min unfocused, never hidden; the
// verdict is crates/widgets justme.rs = the web's justme.ts). Samples (the gallery's preview without data, and the
// gates' TINHAT_WIDGETS_SCENARIO=justMe=<key>) go through the same checkOf, so they read exactly like live checks.
// Motion (only while the host animates): stateFade (a dot's colour eases when its band changes). No timer.
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
    readonly property var ctl: w.hostRef ? w.hostRef.justMe : null
    readonly property string scenario: w.wc ? String(w.wc.scenarioOf("justMe") || "") : ""
    readonly property bool large: w.size === "L" && !w.preview

    // the checker runs while this widget exists (the gallery's preview shows live data too); a sample asks nothing
    Component.onCompleted: if (w.ctl && w.scenario.length === 0) w.ctl.acquire()
    Component.onDestruction: if (w.ctl && w.scenario.length === 0) w.ctl.release()

    // ── the model (the web's JustMeModel) ──
    readonly property var liveCheck: {
        var t = w.ctl ? String(w.ctl.checkJson) : ""
        try { return t.length > 0 ? JSON.parse(t) : null } catch (e) { return null }
    }
    readonly property var liveHistory: {
        var t = w.ctl ? String(w.ctl.historyJson) : ""
        try { return t.length > 0 ? JSON.parse(t) : ({}) } catch (e) { return ({}) }
    }
    readonly property bool hasData: w.liveCheck !== null
    readonly property bool sample: w.scenario.length > 0 || (w.preview && !w.hasData)
    readonly property var model: w.sample ? w.sampleModel(w.scenario.length > 0 ? w.scenario : "preview")
        : ({ check: w.liveCheck, at: w.liveCheck ? w.liveCheck.at : null, running: !!w.ctl && w.ctl.running, history: w.liveHistory })
    readonly property var rows: w.model.check ? w.model.check.rows.concat([w.model.check.ping]) : w.waitingRows
    readonly property var waitingRows: ["Chat", "Calls", "Media", "Extras", "Your ping"].map(function (n, i) {
        return { id: ["chat", "calls", "media", "tinhat", "ping"][i], name: n, band: "", reason: "checking…" }
    })
    readonly property bool live: false

    readonly property Component summaryComponent: Component { Verdict {} }
    readonly property Component extraComponent: Component { Verdict {} }

    function bandColor(b) {
        return b === "good" ? Th.widgetQualityGoodFill : b === "ok" ? Th.widgetQualityOkFill
             : b === "poor" ? Th.widgetQualityPoorFill : Th.widgetQualityUnknownFill
    }
    // the app's clock setting (12 h / 24 h); "System" = the locale's short time WITHOUT seconds (Up next's timeOf)
    function timeOf(ms) {
        var d = new Date(ms)
        var mode = w.app ? String(w.app.clockMode || "") : ""
        if (mode === "12") return Qt.formatTime(d, "h:mm AP")
        if (mode === "24") return Qt.formatTime(d, "HH:mm")
        return Qt.formatTime(d, String(Qt.locale().timeFormat(Locale.ShortFormat)).replace(/[:.]ss/, ""))
    }
    function slotsOf(id) {
        if (!w.ctl || w.model.at === null || w.model.at === undefined) return [null, null, null, null, null, null, null, null, null, null]
        var list = (w.model.history && w.model.history[id]) ? w.model.history[id] : []
        try { return JSON.parse(String(w.ctl.sparkSlots(JSON.stringify(list), w.model.at))) } catch (e) { return [] }
    }

    implicitHeight: bodyCol.implicitHeight

    Column {
        id: bodyCol
        x: 6; width: parent.width - 12
        Repeater {
            model: w.rows.length
            delegate: LightRow { required property int index; width: bodyCol.width; r: w.rows[index] }
        }
        // the friendly sentence (something is red / the connection is slow): a 3 px edge in the verdict's colour
        Item {
            // the height follows the condition, never `visible`: the effective visibility follows the frame's fold state,
            // which follows this height (a binding loop in the gate's calls-down / all-down states)
            readonly property bool shown: !!w.model.check && String(w.model.check.note || "").length > 0
            visible: shown
            width: parent.width
            height: shown ? noteText.implicitHeight + 10 + 6 : 0
            Rectangle {
                x: 0; y: 4
                width: 3; height: parent.height - 6
                radius: 2
                color: w.model.check && w.model.check.verdict.tone === "ok" ? Th.widgetQualityOkFill : Th.widgetQualityPoorFill
            }
            WidgetText {
                id: noteText
                x: 13; y: 4 + 5
                width: parent.width - 13 - 8
                role: "meta"
                text: w.model.check ? String(w.model.check.note || "") : ""
                color: Th.widgetFg
                wrapMode: Text.Wrap
                maximumLineCount: 4
            }
        }
        // footer: "Checked 6:00 PM" · "Check again"
        Item {
            width: parent.width
            height: 24
            WidgetText {
                anchors.left: parent.left; anchors.leftMargin: 8
                anchors.right: again.left; anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                role: "meta"
                text: w.model.running ? "Checking…" : (w.model.at !== null && w.model.at !== undefined ? "Checked " + w.timeOf(w.model.at) : "")
                color: Th.widgetMutedFg
            }
            Controls.AbstractButton {
                id: again
                anchors.right: parent.right; anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                implicitWidth: againTxt.implicitWidth; implicitHeight: againTxt.implicitHeight
                enabled: !w.preview && !w.sample && !w.model.running
                // drawn like the web's button: disabled (0.5) in the gallery's preview and while a check runs; a gate sample
                // only swallows the click (it must not start a real check)
                opacity: w.preview || w.model.running ? 0.5 : 1
                hoverEnabled: true
                focusPolicy: Qt.TabFocus
                Accessible.name: "Check again"
                contentItem: WidgetText { id: againTxt; role: "meta"; semibold: true; text: "Check again"; color: Th.accentBright; font.underline: again.hovered || again.visualFocus }
                onClicked: if (w.ctl) w.ctl.checkNow()
            }
        }
    }

    // ── a light: 24 (L 28), padding 0 8, radius 6: dot 8 · gap 8 · "Name · reason" · L: the 58 × 14 sparkline ──
    component LightRow: Item {
        id: lr
        property var r: ({ id: "", name: "", band: "", reason: "" })
        readonly property bool waiting: !w.model.check
        height: w.large ? 28 : 24
        Accessible.role: Accessible.StaticText
        Accessible.name: lr.r.name + " · " + lr.r.reason
        // the whole line as a tooltip (the web's title), not before the first check
        HoverHandler { id: lrHover }
        Controls.ToolTip.visible: lrHover.hovered && !lr.waiting
        Controls.ToolTip.text: lr.r.name + " · " + lr.r.reason
        Controls.ToolTip.delay: 500
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 8
            spacing: 0
            Rectangle {
                Layout.preferredWidth: 8; Layout.preferredHeight: 8
                Layout.alignment: Qt.AlignVCenter
                radius: 4
                color: lr.waiting ? Th.widgetQualityUnknownFill : w.bandColor(lr.r.band)
                Behavior on color { enabled: !!w.hostRef && w.hostRef.durLong > 0; ColorAnimation { duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic } }
            }
            WidgetText {
                id: nameTxt
                Layout.leftMargin: 8
                Layout.alignment: Qt.AlignBaseline
                role: "text"
                elide: Text.ElideNone
                text: lr.r.name
                color: Th.widgetFg
            }
            WidgetText {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignBaseline
                role: "num"
                text: " · " + lr.r.reason
                color: lr.waiting ? Th.widgetMutedFg : Th.widgetMetaFg
            }
            // L: the last 10 minutes, one bar per minute (bottom-aligned, 4 wide, 2 apart; an empty minute = a 2 px tick)
            Row {
                visible: w.large
                Layout.leftMargin: 8
                Layout.preferredWidth: 58; Layout.preferredHeight: 14
                Layout.alignment: Qt.AlignVCenter
                spacing: 2
                Repeater {
                    model: w.large ? w.slotsOf(lr.r.id) : []
                    delegate: Item {
                        required property var modelData
                        width: 4; height: 14
                        Rectangle {
                            anchors.bottom: parent.bottom
                            width: 4
                            height: modelData ? 3 + Math.round(modelData.h / 1000 * 11) : 2
                            radius: 1
                            color: modelData ? w.bandColor(modelData.band) : Th.widgetQualityUnknownFill
                            opacity: modelData ? 1 : 0.4
                        }
                    }
                }
            }
        }
    }

    // the header: a 6 px dot in the verdict's colour + its text ("All good · 42 ms"); none yet: "Checking…"
    component Verdict: Item {
        readonly property bool has: !!w.model.check
        implicitWidth: (has ? 11 : 0) + vt.implicitWidth
        implicitHeight: vt.implicitHeight
        Rectangle {
            visible: parent.has
            anchors.verticalCenter: parent.verticalCenter
            width: 6; height: 6; radius: 3
            color: parent.has ? w.bandColor(w.model.check.verdict.tone) : "transparent"
        }
        WidgetHeaderText {
            id: vt
            x: parent.has ? 11 : 0
            width: parent.width - x
            anchors.verticalCenter: parent.verticalCenter
            text: parent.has ? String(w.model.check.verdict.text) : "Checking…"
        }
    }

    // ── the sample states (the gallery's preview, the review screenshots, the gates): the web's samples, the same
    //    inputs through the same checkOf; "Checked 6:00 PM" at a fixed local hour so a capture reads the same all day ──
    function todayAt(h, m) { var d = new Date(); d.setHours(h, m, 0, 0); return d.getTime() }
    function sampleModel(key) {
        var P = function (s, ms) { return { status: s, ms: ms } }
        var good = { pings: [P(200, 40), P(200, 42), P(200, 45)], syncAgoMs: 5000, calls: P(200, 84), media: P(200, 51), tinhat: P(401, 30) }
        var input = good, hist = {}
        var with_ = function (k, v) { var o = {}; for (var x in good) o[x] = good[x]; o[k] = v; return o }
        if (key === "slow") {
            input = with_("pings", [P(200, 210), P(200, 230), P(200, 250)]); input.calls = P(200, 1300)
            hist = { ping: ["gggggooooo", [42, 44, 40, 41, 43, 210, 220, 240, 230, 230]], calls: ["gggggggooo", [84, 86, 90, 88, 85, 87, 90, 1100, 1250, 1300]] }
        } else if (key === "callsdown") {
            input = with_("calls", P(null, null)); hist = { calls: ["gggggggppp", [84, 86, 90, 88, 85, 87, 90, 0, 0, 0]] }
        } else if (key === "error") {
            input = with_("tinhat", P(502, 20)); hist = { tinhat: ["ggggggggpp", [30, 28, 32, 31, 29, 30, 33, 30, 0, 0]] }
        } else if (key === "stuck") {
            input = with_("syncAgoMs", 180000); hist = { chat: ["gggggggopp", [38, 41, 40, 44, 39, 42, 40, 43, 0, 0]] }
        } else if (key === "alldown") {
            input = { pings: [P(null, null), P(null, null), P(null, null)], syncAgoMs: 200000, calls: P(null, null), media: P(null, null), tinhat: P(null, null) }
            hist = { chat: ["ggggggpppp", []], calls: ["ggggggpppp", []], media: ["ggggggpppp", []], tinhat: ["ggggggpppp", []], ping: ["ggggggpppp", []] }
        } else if (key === "checking") {
            return { check: null, at: null, running: true, history: {} }
        }
        var at = w.todayAt(18, 0)
        var base = {
            chat: ["gggggggggg", [38, 41, 40, 44, 39, 42, 40, 43, 41, 42]],
            calls: ["gggggggggg", [80, 90, 84, 86, 82, 88, 85, 83, 87, 84]],
            media: ["gggggggggg", [50, 48, 55, 52, 49, 51, 53, 50, 52, 51]],
            tinhat: ["gggggggggg", [30, 28, 32, 31, 29, 30, 33, 30, 31, 30]],
            ping: ["gggggggggg", [38, 41, 40, 44, 39, 42, 40, 43, 41, 42]]
        }
        for (var k in hist) base[k] = hist[k]
        var history = {}
        for (var id in base) {
            var bands = base[id][0], ms = base[id][1], list = []
            for (var i = 0; i < bands.length; ++i) {
                var c = bands.charAt(i)
                list.push({ at: at - (bands.length - 1 - i) * 60000, band: c === "g" ? "good" : (c === "o" ? "ok" : "poor"), ms: c === "p" ? null : (ms[i] !== undefined ? ms[i] : 40) })
            }
            history[id] = list
        }
        var check = null
        try { check = w.ctl ? JSON.parse(String(w.ctl.checkOf(JSON.stringify(input)))) : null } catch (e) { check = null }
        return { check: check, at: at, running: false, history: history }
    }
}
