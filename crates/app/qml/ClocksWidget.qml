// ClocksWidget.qml — widgets W1 (2026-09-28): Friends' clocks (`clocks`, contract WIDGETS.md §15, WIDGETS-UI.md §15;
// the web's features/widgets/clocks/Clocks.tsx).
//
// The local time of everyone who shares a room with you and shares their time zone (REACH A7: opt-out, on by default),
// sorted west to east, your own row highlighted, a sun by day and a moon at night (22:00–07:00). Header (M / L): the
// spread — your zone, then the furthest one ("PT · ET +3 h"). S / collapsed: faces and times on one line, one group per
// zone. L adds the weekday ("Tue") when a friend is on another day. The times tick once a minute, only while the window
// is in view. The zones come with the presence read (PresenceController.wantZones → `?tz=1`, zonesJson); the offsets
// are QTimeZone's, the rules shared (WidgetsController.clockRows → tinhat_widgets::clocks). You switched it off →
// "You're hidden · Share again". Click a friend's row → their DM.
// Motion: rowSlide (a friend who appears). Gate / review: TINHAT_WIDGETS_SCENARIO "clocks=<key>" (data, many, alone,
// hidden, empty, loading) — the web's samples (a fixed moment, fixed offsets).
import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
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
    readonly property var pc: w.app ? w.app.presenceCtl : null
    readonly property int limit: w.size === "L" ? 12 : 5

    Component.onCompleted: { if (w.pc && !w.preview) w.pc.wantZones(true); w.prevKeys = w.keys }
    Component.onDestruction: if (w.pc && !w.preview) w.pc.wantZones(false)

    // ── the minute (ticks at each minute boundary, only while the window is in view) ──
    property double now: Date.now()
    Timer {
        running: !!w.hostRef && w.hostRef.winState !== "hidden" && !w.sample
        interval: Math.max(1000, 60000 - (Date.now() % 60000) + 20)
        repeat: false
        onTriggered: { w.now = Date.now(); restart() }
    }
    Connections {   // back from hidden: catch up at once
        target: w.hostRef
        function onWinStateChanged() { if (w.hostRef.winState !== "hidden") w.now = Date.now() }
    }

    // ── the model (the web's useClocks) ──
    readonly property string zonesRaw: w.pc ? String(w.pc.zonesJson || "{}") : "{}"
    onZonesRawChanged: if (w.wc && !w.preview) w.wc.setZones(w.zonesRaw)
    readonly property var tz: {
        var raw = w.pc ? String(w.pc.tzJson || "") : ""
        try { return raw.length > 0 ? JSON.parse(raw) : ({}) } catch (e) { return ({}) }
    }
    readonly property string myTz: (w.tz.share === true && w.tz.tz) ? String(w.tz.tz) : String(w.tz.device || "")
    readonly property bool ready: !!w.app && w.app.mPresenceKnown === true && w.readOnce
    // the first answer with zones: zonesJson changes from its initial "{}", or a read has come back at all (rev bumps)
    property bool readOnce: false
    Connections {
        target: w.pc
        function onRevChanged() { w.readOnce = true }
        function onZonesJsonChanged() { w.readOnce = true }
    }
    readonly property var realModel: {
        var r = { rows: [], spread: "", myOffset: 0 }
        if (w.wc) { try { r = JSON.parse(String(w.wc.clockRows(w.zonesRaw, w.myTz, w.now))) } catch (e) {} }
        r.ready = w.ready
        r.hidden = w.tz.share === false
        return r
    }
    readonly property bool hasData: w.realModel.ready && w.realModel.rows.length > 0
    readonly property string scenario: w.wc ? String(w.wc.scenarioOf("clocks") || "") : ""
    readonly property bool sample: w.scenario.length > 0 || (w.preview && !w.hasData)
    readonly property var model: {
        if (!w.sample) return w.realModel
        try { return JSON.parse(String(w.wc.sampleClockRows(w.scenario.length > 0 ? w.scenario : "data"))) } catch (e) { return ({ rows: [], spread: "", ready: true, hidden: false }) }
    }
    readonly property var shown: w.model.rows.slice(0, w.limit)
    readonly property bool live: false
    readonly property bool busy: w.tz.busy === true

    function person(mxid) {
        var a = w.wc ? w.wc.aroundJson : ""
        var j = null
        try { j = String(a || "").length > 1 ? JSON.parse(String(a)) : null } catch (e) {}
        var p = (j && j.people) ? j.people[mxid] : null
        var local = String(mxid).replace(/^@/, "").split(":")[0]
        if (mxid.indexOf(":sample.invalid") > 0) return { mxid: mxid, name: local === "me" ? "You" : local.charAt(0).toUpperCase() + local.slice(1), avatar: "", dm: "" }
        return { mxid: mxid, name: p ? p.name : local, avatar: p ? (p.avatar || "") : "", dm: p ? (p.dm || "") : "" }
    }
    readonly property int aroundRev: w.wc ? w.wc.rev : 0
    function dm(who) {
        if (w.preview || w.sample || !w.app) return
        if (who.dm && who.dm.length > 0) w.app.openDmFromRail(who.dm)
        else if (w.app.login) w.app.login.createDm(who.mxid)
    }
    function shareAgain() { if (!w.preview && !w.sample && w.pc) w.pc.tzShare(true) }
    // the app's clock setting (12 h / 24 h / the system's short format without seconds) for HH:MM of a row
    function fmtMinutes(m) {
        var d = new Date(2000, 0, 1, Math.floor(m / 60), m % 60)
        var mode = w.app ? String(w.app.clockMode || "") : ""
        if (mode === "12") return Qt.formatTime(d, "h:mm AP")
        if (mode === "24") return Qt.formatTime(d, "HH:mm")
        return Qt.formatTime(d, String(Qt.locale().timeFormat(Locale.ShortFormat)).replace(/[:.]ss/, ""))
    }

    // S / collapsed (the web's summaryGroups): one group per zone in row order, your own zone left out while anyone
    // else is listed; up to 2 faces and the time each; a group that doesn't fit whole is left out (a clipped Flow)
    readonly property var groups: {
        var out = []
        var rows = w.model.rows
        for (var i = 0; i < rows.length; ++i) {
            var g = out.length ? out[out.length - 1] : null
            if (g && g.key === rows[i].offsetMin) { g.ids.push(rows[i].id); g.mine = g.mine || rows[i].me === true }
            else out.push({ key: rows[i].offsetMin, minutes: rows[i].minutes, ids: [rows[i].id], mine: rows[i].me === true })
        }
        var others = out.filter(function (x) { return !(x.mine && x.ids.length === 1) })
        return others.length > 0 ? others : out
    }
    readonly property Component summaryComponent: Component {
        Item {
            // the whole line's width (every group side by side): the frame gives min(this, the slot)
            implicitWidth: {
                if (!sumRow.visible) return sumEmpty.implicitWidth
                var s = 0, n = 0
                for (var i = 0; i < sumRow.children.length; ++i) {
                    var c = sumRow.children[i]
                    if (c.width > 0) { s += c.width + (n > 0 ? 10 : 0); n++ }
                }
                return s
            }
            implicitHeight: 20
            clip: true
            WidgetHeaderText {
                id: sumEmpty
                visible: !w.model.ready || w.model.rows.length === 0
                anchors.verticalCenter: parent.verticalCenter
                text: !w.model.ready ? "Checking…" : (w.model.hidden ? "You're hidden" : "No clocks yet")
            }
            Flow {
                id: sumRow
                visible: w.model.ready && w.model.rows.length > 0
                width: parent.width
                spacing: 10
                Repeater {
                    model: w.groups.length
                    delegate: Row {
                        required property int index
                        readonly property var g: w.groups[index]
                        height: 20
                        spacing: 4
                        // a group the Flow wrapped onto the second line doesn't fit whole: not shown (the avatars'
                        // layers would escape the clip; opacity, not visible — the Flow keeps its layout)
                        opacity: y < 1 ? 1 : 0
                        Row {
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: -4
                            Repeater {
                                model: Math.min(2, g.ids.length)
                                delegate: Item {
                                    required property int index
                                    readonly property var who: { var _r = w.aroundRev; return w.person(g.ids[index]) }
                                    width: 16; height: 16
                                    Rectangle { x: -1.5; y: -1.5; width: 19; height: 19; radius: 9.5; color: Th.widgetBg }
                                    Avatar { anchors.fill: parent; app: w.app; size: 16; seed: parent.who.mxid; name: parent.who.name; imageSource: parent.who.avatar || "" }
                                }
                            }
                        }
                        WidgetText { anchors.verticalCenter: parent.verticalCenter; role: "text"; text: w.fmtMinutes(g.minutes); color: Th.widgetFg; font.features: { "tnum": 1 } }
                    }
                }
            }
        }
    }
    // M / L header: the spread
    readonly property Component extraComponent: Component { WidgetHeaderText { text: w.model.ready && w.model.rows.length > 0 ? String(w.model.spread || "") : "" } }

    // rowSlide: a friend who newly appears (after the first fill) — opacity 0→1, y +4→0 (long, OutCubic)
    readonly property var keys: w.shown.map(function (r) { return String(r.id) })
    property var prevKeys: null
    onKeysChanged: Qt.callLater(function () { w.prevKeys = w.keys })
    function isFresh(k) { return w.prevKeys !== null && w.prevKeys.indexOf(k) < 0 }

    implicitHeight: bodyCol.implicitHeight

    Column {
        id: bodyCol
        x: 6; width: parent.width - 12
        WidgetText {
            visible: !w.model.ready
            width: parent.width; height: visible ? 56 : 0
            horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter
            role: "text"
            text: "Checking…"
            color: Th.widgetMetaFg
        }
        WidgetText {
            visible: w.model.ready && w.model.rows.length === 0
            width: parent.width; height: visible ? 56 : 0
            horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter
            role: "text"
            text: "No clocks yet"
            color: Th.widgetMetaFg
        }
        Repeater {
            model: w.model.ready ? w.shown.length : 0
            delegate: ClockRow { required property int index; width: bodyCol.width; r: w.shown[index] }
        }
        WidgetText {
            visible: w.model.ready && w.model.rows.length > w.shown.length
            x: 8
            width: parent.width - 16; height: visible ? 20 : 0
            verticalAlignment: Text.AlignBottom
            role: "meta"
            text: "+" + (w.model.rows.length - w.shown.length) + " more"
            color: Th.widgetMetaFg
        }
        // "● You're hidden · Share again" (Who's around's dot + link)
        Item {
            visible: w.model.ready && w.model.hidden === true
            width: parent.width; height: visible ? 4 + 22 : 0
            Row {
                x: 8; y: 4
                height: 22
                spacing: 4
                Rectangle { anchors.verticalCenter: parent.verticalCenter; width: 6; height: 6; radius: 3; color: Th.accountAskDot }
                WidgetText { anchors.verticalCenter: parent.verticalCenter; role: "meta"; text: "You're hidden ·"; color: Th.widgetMetaFg }
                Controls.AbstractButton {
                    id: againBtn
                    anchors.verticalCenter: parent.verticalCenter
                    implicitWidth: againT.implicitWidth + 12; implicitHeight: againT.implicitHeight + 2
                    hoverEnabled: true
                    focusPolicy: Qt.TabFocus
                    enabled: !w.busy && !w.preview
                    opacity: w.busy ? 0.6 : 1
                    Accessible.name: "Share my time zone again"
                    background: Rectangle { radius: 5; color: (againBtn.hovered || againBtn.visualFocus) && againBtn.enabled ? Th.widgetUpNextTbdBgHover : "transparent" }
                    contentItem: WidgetText { id: againT; role: "meta"; semibold: true; text: "Share again"; color: Th.widgetUpNextTbdFg; horizontalAlignment: Text.AlignHCenter }
                    onClicked: w.shareAgain()
                }
            }
        }
    }

    // ── a clock: 28, padding 0 8, radius 6: avatar 20 · name (fills) · zone + difference · sun / moon 12 · [L: day] · time ──
    component ClockRow: Rectangle {
        id: cr
        property var r: ({ id: "", tz: "", label: "", offsetMin: 0, minutes: 0, night: false, me: false, day: "", diff: "" })
        readonly property var who: { var _r = w.aroundRev; return w.person(String(cr.r.id)) }
        readonly property string time: w.fmtMinutes(cr.r.minutes)
        readonly property string zone: cr.r.me ? String(cr.r.label) : String(cr.r.label) + " " + String(cr.r.diff)
        readonly property string key: String(cr.r.id)
        onKeyChanged: if (w.isFresh(cr.key)) crIn.play()
        Component.onCompleted: if (w.isFresh(cr.key)) crIn.play()
        SlideIn { id: crIn; target: cr }
        height: 28
        radius: 6
        color: cr.r.me ? Th.widgetClocksMeBg : (crHover.hovered && !w.preview ? Th.widgetRowBgHover : "transparent")
        Accessible.role: cr.r.me ? Accessible.StaticText : Accessible.Button
        Accessible.name: cr.who.name + ", " + cr.zone + ", " + (cr.r.day ? cr.r.day + " " : "") + cr.time
        HoverHandler { id: crHover; enabled: !w.preview && !cr.r.me }
        MouseArea {
            anchors.fill: parent
            enabled: !w.preview && !cr.r.me
            cursorShape: Qt.PointingHandCursor
            onClicked: w.dm(cr.who)
        }
        Controls.ToolTip.text: cr.who.name + " · " + cr.zone + " · " + (cr.r.day ? cr.r.day + " " : "") + cr.time
        Controls.ToolTip.visible: crHover.hovered
        Controls.ToolTip.delay: 500
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 8
            spacing: 8
            Avatar {
                Layout.preferredWidth: 20; Layout.preferredHeight: 20
                Layout.alignment: Qt.AlignVCenter
                app: w.app
                size: 20
                seed: cr.who.mxid
                name: cr.who.name
                imageSource: cr.who.avatar || ""
            }
            WidgetText { Layout.fillWidth: true; Layout.minimumWidth: 0; role: "text"; text: cr.who.name; color: Th.widgetFg }
            WidgetText { role: "meta"; text: cr.zone; color: Th.widgetMetaFg }
            DayNight { Layout.preferredWidth: 12; Layout.preferredHeight: 12; Layout.alignment: Qt.AlignVCenter; night: cr.r.night === true }
            WidgetText { visible: w.size === "L" && String(cr.r.day || "").length > 0; role: "meta"; text: String(cr.r.day || ""); color: Th.widgetMetaFg }
            WidgetText {
                Layout.minimumWidth: 36
                horizontalAlignment: Text.AlignRight
                role: "text"; semibold: true
                font.features: { "tnum": 1 }
                text: cr.time
                color: Th.widgetFg
            }
        }
    }

    // sun (day) / moon (night), 12 px — the same paths as the web's DayNight SVG (viewBox 0 0 12 12)
    component DayNight: Item {
        id: dn
        property bool night: false
        implicitWidth: 12; implicitHeight: 12
        Shape {
            visible: dn.night
            anchors.fill: parent
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                strokeWidth: 0; strokeColor: "transparent"
                fillColor: Th.widgetClocksNightIcon
                PathSvg { path: "M 7.5 1.2 A 5 5 0 1 0 10.8 8.2 A 4 4 0 0 1 7.5 1.2 Z" }
            }
        }
        Shape {
            visible: !dn.night
            anchors.fill: parent
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                strokeWidth: 0; strokeColor: "transparent"
                fillColor: Th.widgetClocksDayIcon
                PathSvg { path: "M 3.5 6 A 2.5 2.5 0 1 0 8.5 6 A 2.5 2.5 0 1 0 3.5 6 Z" }
            }
            ShapePath {
                strokeWidth: 1.2; strokeColor: Th.widgetClocksDayIcon
                fillColor: "transparent"
                capStyle: ShapePath.RoundCap
                PathSvg { path: "M 6 0.5 L 6 2 M 6 10 L 6 11.5 M 0.5 6 L 2 6 M 10 6 L 11.5 6 M 2.1 2.1 L 3.2 3.2 M 8.8 8.8 L 9.9 9.9 M 2.1 9.9 L 3.2 8.8 M 8.8 3.2 L 9.9 2.1" }
            }
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
}
