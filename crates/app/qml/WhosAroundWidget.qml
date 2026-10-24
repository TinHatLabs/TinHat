// WhosAroundWidget.qml — widgets: Who's around (`whosAround`, contract WIDGETS.md §8 + its 2026-09-27 amendment,
// WIDGETS-UI.md §6; the web's features/widgets/around/WhosAround.tsx).
//
// Friends who SHARE their status (the Reach presence service — never Synapse presence, never you, never the
// recorder), grouped Playing (a call room with a now-playing game: its cover, the players, Join), In a call (the
// other call rooms; someone in a call in a room you are not in = a person row "In a call" without Join), Online,
// Away (the `away` setting). M: Online / Away with more than 3 people become an avatar strip; L: every group as rows.
// Click → DM, hover 500 ms → the profile card, right-click → Message · Call · View profile. Viewing is
// non-reciprocal (REACH §2.3): when you don't share (and answered the one-time question) a footer says so + Turn on.
// Data: app.mPresence (PresenceController) + WidgetsController.aroundJson (names, avatars, DM rooms, the call rooms'
// activity and cover). No timer of its own. Motion (only while the host animates): personSlide, coverFade, countRoll.
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
    readonly property bool showAway: !w.settings || w.settings.away !== false
    readonly property bool large: w.size === "L" && !w.preview

    // the presence map → WidgetsController (names, rooms) on every change
    readonly property var presence: (w.app && w.app.mPresence) ? w.app.mPresence : ({})
    onPresenceChanged: if (w.wc) w.wc.setPresence(JSON.stringify(w.presence))
    readonly property var around: {
        var r = w.wc ? w.wc.rev : 0
        var j = w.wc ? String(w.wc.aroundJson || "") : ""
        if (j.length < 2) return null
        try { return JSON.parse(j) } catch (e) { return null }
    }
    readonly property string me: w.app ? String(w.app.mUserId || "") : ""
    readonly property bool sharing: !!w.app && w.app.mPresenceSharing === true && w.app.mPresenceTurningOff !== true
    readonly property bool asked: !!w.app && w.app.mPresenceAsked === true
    readonly property bool busy: !!w.app && w.app.mPresenceBusy === true
    readonly property bool ready: !!w.app && w.app.mPresenceKnown === true

    function person(mxid, pres) {
        var p = (w.around && w.around.people) ? w.around.people[mxid] : null
        return { mxid: mxid, name: p ? p.name : (w.app ? w.app.localpart(mxid) : mxid), avatar: p ? (p.avatar || "") : "", dm: p ? (p.dm || "") : "", pres: pres || null }
    }
    // a presence entry's dot: its view colour (the custom status colour when it has one, computed in Rust)
    function dotOf(pres) {
        if (!pres) return Th.presenceOnlineDot
        if (pres.dot && String(pres.dot).charAt(0) === "#") return pres.dot
        return pres.state === "away" ? Th.presenceAwayDot : (pres.state === "call" ? Th.presenceIncallDot : Th.presenceOnlineDot)
    }
    function roomInfo(rid) {
        var info = (w.around && w.around.rooms) ? w.around.rooms[rid] : null
        return info ? info : { name: w.app ? w.app.roomNameOf(rid) : "", game: "", emoji: "", cover: "" }
    }

    // ── the model (the web's useAround): the shared groupAround (WIDGETS.md §10.6, people.rs — the web's people.ts)
    //    over the presence map; ids back to people for the rows ──
    readonly property var model: {
        if (w.sample) return w.sampleModel
        var m = w.presence
        var people = [], activities = ({})
        var ids = Object.keys(m)
        for (var i = 0; i < ids.length; ++i) {
            var pr = m[ids[i]]
            if (!pr) continue
            var room = pr.room ? String(pr.room) : ""
            people.push({ id: ids[i], name: String(w.person(ids[i], pr).name || ""), state: String(pr.state || ""), room: room })
            // the call room's now-playing activity (game + emoji as written; people.rs trims them)
            if (room && pr.state === "call" && !(room in activities)) {
                var inf = w.roomInfo(room)
                activities[room] = { game: String(inf.game || ""), emoji: String(inf.emoji || "") }
            }
        }
        var g = null
        try { g = JSON.parse(String(w.wc ? w.wc.groupAround(JSON.stringify({ me: w.me, people: people, activities: activities, showAway: w.showAway })) : "")) } catch (e) { g = null }
        if (!g) return { playing: [], inCall: [], callElsewhere: [], online: [], away: [], total: 0 }
        var P = function (x) { return w.person(x, m[x]) }
        var R = function (r) {
            var info = w.roomInfo(r.room)
            return { room: r.room, people: r.people.map(P), name: info.name || "a room", game: r.game, emoji: r.emoji, cover: info.cover || "" }
        }
        return { playing: g.playing.map(R), inCall: g.inCall.map(R), callElsewhere: g.callElsewhere.map(P), online: g.online.map(P), away: g.away.map(P), total: g.total }
    }
    readonly property bool hasData: w.ready && w.model.total > 0
    readonly property bool sample: w.preview && !(w.ready && Object.keys(w.presence).some(function (k) { return k !== w.me && !(w.app && w.app.isRecorder && w.app.isRecorder(k)) }))
    readonly property var everyone: {
        var out = []
        var m = w.model
        m.playing.forEach(function (g) { g.people.forEach(function (p) { out.push(p) }) })
        m.inCall.forEach(function (g) { g.people.forEach(function (p) { out.push(p) }) })
        return out.concat(m.callElsewhere, m.online, m.away)
    }
    readonly property bool live: w.model.playing.length > 0 || w.model.inCall.length > 0 || w.model.callElsewhere.length > 0

    // the S / folded summary: up to 6 faces (20 px, status rings, 8 apart) + "+n"; nobody: "Nobody around"
    readonly property Component summaryComponent: Component {
        Row {
            spacing: 8
            WidgetText { visible: w.everyone.length === 0; role: "meta"; text: "Nobody around"; color: Th.widgetSummaryFg }
            Repeater {
                model: Math.min(6, w.everyone.length)
                delegate: RingFace { required property int index; who: w.everyone[index]; size: 20 }
            }
            Rectangle {
                visible: w.everyone.length > 6
                anchors.verticalCenter: parent.verticalCenter
                height: 16; radius: 4; width: plusT.implicitWidth + 10
                color: Th.widgetChipBg
                WidgetText { id: plusT; anchors.centerIn: parent; role: "chip"; text: "+" + (w.everyone.length - 6); color: Th.widgetChipFg }
            }
            Item { width: 3 - 8; height: 1 }   // padding-right 3 (the row's spacing already adds 8)
        }
    }
    readonly property Component extraComponent: null

    // personSlide: a row whose key is new (after the first fill) slides in from above (the web's tw-enter-down)
    readonly property var keys: {
        var m = w.model
        return m.playing.map(function (g) { return "r:" + g.room }).concat(m.inCall.map(function (g) { return "r:" + g.room }),
            m.callElsewhere.map(function (p) { return p.mxid }), m.online.map(function (p) { return p.mxid }), m.away.map(function (p) { return p.mxid }))
    }
    property var prevKeys: null
    onKeysChanged: Qt.callLater(function () { w.prevKeys = w.keys })
    Component.onCompleted: {
        if (w.wc) w.wc.setPresence(JSON.stringify(w.presence))
        w.prevKeys = w.keys
    }
    function isFresh(k) { return w.prevKeys !== null && w.prevKeys.indexOf(k) < 0 }

    // ── actions ──
    function dm(who) {
        if (w.preview || !w.app) return
        if (who.dm && who.dm.length > 0) w.app.openDmFromRail(who.dm)
        else if (w.app.login) w.app.login.createDm(who.mxid)
    }
    function callWith(who) {
        if (w.preview || !w.app) return
        if (who.dm && who.dm.length > 0) { w.app.openRoomId(who.dm); w.app.callctl.switchCall(who.dm) }
        else if (w.app.login) w.app.login.createDm(who.mxid)
    }
    function join(roomId) { if (!w.preview && w.app) { w.app.openRoomId(roomId); w.app.callctl.switchCall(roomId) } }
    function inRoomCall(roomId) { return !!w.app && w.app.mInCall && w.app.mCallRoomId === roomId }

    implicitHeight: bodyCol.implicitHeight

    Column {
        id: bodyCol
        x: 6; width: parent.width - 12
        // not read yet
        WidgetText {
            visible: !w.ready && !w.sample
            width: parent.width; height: 44
            horizontalAlignment: Text.AlignHCenter
            role: "meta"
            text: "Checking who's around…"
            color: Th.widgetMutedFg
        }
        // empty
        Item {
            visible: (w.ready || w.sample) && w.model.total === 0
            width: parent.width; height: visible ? Math.max(56, emptyCol.implicitHeight) : 0
            Column {
                id: emptyCol
                anchors.centerIn: parent
                spacing: 2
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "text"; text: "Nobody's around right now"; color: Th.widgetMetaFg }
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "meta"; text: "Friends who share their status show up here."; color: Th.widgetMutedFg }
            }
        }
        // Playing
        Group {
            visible: (w.ready || w.sample) && w.model.playing.length > 0
            label: "Playing"
            count: w.model.playing.reduce(function (n, g) { return n + g.people.length }, 0)
            Repeater {
                model: w.model.playing.length
                delegate: RoomRow { required property int index; width: bodyCol.width; g: w.model.playing[index]; playing: true }
            }
        }
        // In a call
        Group {
            visible: (w.ready || w.sample) && (w.model.inCall.length > 0 || w.model.callElsewhere.length > 0)
            label: "In a call"
            count: w.model.inCall.reduce(function (n, g) { return n + g.people.length }, 0) + w.model.callElsewhere.length
            Repeater {
                model: w.model.inCall.length
                delegate: RoomRow { required property int index; width: bodyCol.width; g: w.model.inCall[index]; playing: false }
            }
            Repeater {
                model: w.model.callElsewhere.length
                delegate: PersonRow { required property int index; width: bodyCol.width; who: w.model.callElsewhere[index]; elsewhere: true }
            }
        }
        // Online
        Group {
            visible: (w.ready || w.sample) && w.model.online.length > 0
            label: "Online"
            count: w.model.online.length
            Strip { visible: !w.large && w.model.online.length > 3; people: w.model.online }
            Repeater {
                model: !w.large && w.model.online.length > 3 ? 0 : w.model.online.length
                delegate: PersonRow { required property int index; width: bodyCol.width; who: w.model.online[index] }
            }
        }
        // Away
        Group {
            visible: (w.ready || w.sample) && w.model.away.length > 0
            label: "Away"
            count: w.model.away.length
            Strip { visible: !w.large && w.model.away.length > 3; people: w.model.away }
            Repeater {
                model: !w.large && w.model.away.length > 3 ? 0 : w.model.away.length
                delegate: PersonRow { required property int index; width: bodyCol.width; who: w.model.away[index]; isAway: true }
            }
        }
        // not sharing (and the one-time question answered): the footer + Turn on
        Item {
            visible: w.ready && !w.sharing && w.asked && !w.preview
            width: parent.width; height: visible ? 4 + 26 : 0
            RowLayout {
                x: 8; y: 4
                width: parent.width - 16; height: 26
                spacing: 6
                Rectangle { Layout.preferredWidth: 8; Layout.preferredHeight: 8; radius: 4; color: Th.accountAskDot }
                WidgetText { Layout.fillWidth: true; role: "meta"; text: "You're hidden from others"; color: Th.widgetMutedFg }
                Controls.AbstractButton {
                    id: turnOn
                    implicitWidth: turnT.implicitWidth + 12; implicitHeight: turnT.implicitHeight + 2
                    hoverEnabled: true
                    focusPolicy: Qt.TabFocus
                    enabled: !w.busy
                    opacity: w.busy ? 0.6 : 1
                    background: Rectangle { radius: 5; color: (turnOn.hovered || turnOn.visualFocus) && turnOn.enabled ? Th.widgetUpNextTbdBgHover : "transparent" }
                    contentItem: WidgetText { id: turnT; role: "meta"; semibold: true; text: w.busy ? "Turning on…" : "Turn on"; color: Th.widgetUpNextTbdFg; horizontalAlignment: Text.AlignHCenter }
                    onClicked: if (w.app && w.app.presenceCtl) w.app.presenceCtl.chooseSharing(true)
                }
            }
        }
    }

    // a group: its label (20 px, W-label on the bottom, padding 0 8 2) "PLAYING · n" (countRoll) over its rows
    component Group: Column {
        id: grp
        property string label: ""
        property int count: 0
        default property alias rows: rowsCol.data
        width: bodyCol.width
        Item {
            width: parent.width; height: 20
            Row {
                x: 8; y: 20 - 2 - height
                spacing: 3
                WidgetText { role: "label"; text: grp.label + " ·"; color: Th.widgetAroundGroupFg }
                CountRoll { value: grp.count }
            }
        }
        Column { id: rowsCol; width: parent.width }
    }
    // countRoll: the new number slides in from below (70 % → 0, opacity 0 → 1, long OutCubic); the old one is gone at once
    component CountRoll: Item {
        id: cnt
        property int value: 0
        implicitWidth: cntT.implicitWidth
        implicitHeight: cntT.implicitHeight
        clip: true
        WidgetText { id: cntT; role: "label"; text: String(cnt.value); color: Th.widgetAroundGroupFg; transform: Translate { id: rollShift; y: 0 } }
        property bool _ready: false
        Component.onCompleted: cnt._ready = true
        onValueChanged: {
            var d = w.hostRef ? w.hostRef.durLong : 0
            if (!cnt._ready || d <= 0) return
            rollAnim.stop()
            rollY.duration = d; rollO.duration = d
            rollAnim.start()
        }
        ParallelAnimation {
            id: rollAnim
            NumberAnimation { id: rollY; target: rollShift; property: "y"; from: cntT.implicitHeight * 0.7; to: 0; easing.type: Easing.OutCubic }
            NumberAnimation { id: rollO; target: cntT; property: "opacity"; from: 0; to: 1; easing.type: Easing.OutCubic }
        }
    }

    // ── a room row (Playing, In a call): 40, padding 4 8, gap 8: tile · name + players · Join ──
    component RoomRow: Rectangle {
        id: rr
        property var g: ({ room: "", people: [], name: "", game: "", emoji: "", cover: "" })
        property bool playing: false
        readonly property bool here: w.inRoomCall(rr.g.room)
        readonly property string key: "r:" + rr.g.room
        onKeyChanged: if (w.isFresh(rr.key)) rrIn.play()
        Component.onCompleted: if (w.isFresh(rr.key)) rrIn.play()
        EnterDown { id: rrIn; target: rr }
        height: 40
        radius: 8
        color: rrMa.containsMouse && !w.preview ? Th.widgetRowBgHover : "transparent"
        MouseArea { id: rrMa; anchors.fill: parent; hoverEnabled: true; enabled: !w.preview; cursorShape: Qt.PointingHandCursor; onClicked: if (w.app) w.app.openRoomId(rr.g.room) }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 8
            spacing: 8
            // Playing: the cover 24 × 36 (or the activity's emoji on the tile); In a call: the headset tile 24 × 24
            WidgetCover {
                visible: rr.playing
                Layout.preferredWidth: 24; Layout.preferredHeight: 36
                radius: 3
                source: rr.playing && !w.preview ? (rr.g.cover || "") : ""
                emoji: rr.g.emoji || "🎮"
                fadeDur: w.hostRef ? w.hostRef.durVeryLong : 0
                behind: rrMa.containsMouse && !w.preview ? Qt.tint(Th.widgetBg, Th.widgetRowBgHover) : Th.widgetBg
            }
            Rectangle {
                visible: !rr.playing
                Layout.preferredWidth: 24; Layout.preferredHeight: 24
                radius: 6
                color: Th.widgetAroundCallBg
                ThIcon { anchors.centerIn: parent; width: 14; height: 14; app: w.app; name: "headphones-symbolic"; color: Th.widgetAroundCallIcon }
            }
            Column {
                Layout.fillWidth: true
                WidgetText { width: parent.width; role: "strong"; text: rr.playing ? rr.g.game : rr.g.name; color: Th.widgetFg }
                RowLayout {
                    width: parent.width
                    spacing: 6
                    WidgetText { visible: rr.playing; Layout.fillWidth: false; Layout.maximumWidth: parent.width - facesRow.width - 6; role: "meta"; text: "in " + rr.g.name; color: Th.widgetMetaFg }
                    Faces { id: facesRow; people: rr.g.people; size: 16; max: 4 }
                    Item { Layout.fillWidth: true }
                }
            }
            // (preview: shown but not clickable, like the web's disabled Join)
            ThButton {
                visible: !rr.here
                enabled: !w.preview
                Layout.preferredHeight: 24
                app: w.app
                highlighted: true
                text: "Join"
                implicitHeight: 24
                leftPadding: 10; rightPadding: 10
                font.pointSize: (Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10) * 0.85
                font.weight: Font.DemiBold
                onClicked: w.join(rr.g.room)
            }
            WidgetText { visible: rr.here; role: "meta"; text: "You're here"; color: Th.widgetMutedFg }
        }
    }
    // players: 16 px faces 12 apart (1.5 px ring), max 4 + "+n" (margin-left 4)
    component Faces: Row {
        id: fc
        property var people: []
        property int size: 16
        property int max: 4
        spacing: -4
        Repeater {
            model: Math.min(fc.max, fc.people.length)
            delegate: Item {
                required property int index
                readonly property var who: fc.people[index]
                width: fc.size; height: fc.size
                Rectangle { x: -1.5; y: -1.5; width: fc.size + 3; height: fc.size + 3; radius: width / 2; color: Th.widgetBg }
                Avatar { anchors.fill: parent; app: w.app; size: fc.size; seed: parent.who.mxid; name: parent.who.name; imageSource: parent.who.avatar || "" }
            }
        }
        Item { visible: fc.people.length > fc.max; width: 8; height: 1 }
        Rectangle {
            visible: fc.people.length > fc.max
            anchors.verticalCenter: parent.verticalCenter
            height: 16; radius: 4; width: moreT.implicitWidth + 10
            color: Th.widgetChipBg
            WidgetText { id: moreT; anchors.centerIn: parent; role: "chip"; text: "+" + (fc.people.length - fc.max); color: Th.widgetChipFg }
        }
    }

    // ── a person row (26, padding 0 8, gap 8): avatar 20 + the presence dot · name · the status chip / "In a call" ──
    component PersonRow: Rectangle {
        id: pr
        property var who: ({ mxid: "", name: "", avatar: "", pres: null })
        property bool isAway: false
        property bool elsewhere: false
        readonly property var chip: pr.who.pres && pr.who.pres.chip ? { text: String(pr.who.pres.chip), color: String(pr.who.pres.chipColor || "") } : null
        readonly property string key: pr.who.mxid
        onKeyChanged: if (w.isFresh(pr.key)) prIn.play()
        Component.onCompleted: if (w.isFresh(pr.key)) prIn.play()
        EnterDown { id: prIn; target: pr }
        height: 26
        radius: 6
        color: prMa.containsMouse && !w.preview ? Th.widgetRowBgHover : "transparent"
        MouseArea {
            id: prMa
            anchors.fill: parent
            hoverEnabled: true
            enabled: !w.preview
            acceptedButtons: Qt.LeftButton | Qt.RightButton
            cursorShape: Qt.PointingHandCursor
            onClicked: (m) => { if (m.button === Qt.RightButton) personMenu.openFor(pr.who); else w.dm(pr.who) }
            onContainsMouseChanged: if (containsMouse) hoverCard.later(pr, pr.who); else hoverCard.cancel(pr)
        }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 8
            spacing: 8
            Avatar {
                Layout.preferredWidth: 20; Layout.preferredHeight: 20
                app: w.app
                size: 20
                seed: pr.who.mxid
                name: pr.who.name
                imageSource: pr.who.avatar || ""
                presence: pr.who.pres ? String(pr.who.pres.state || "") : ""
                presenceColor: w.dotOf(pr.who.pres)
                presenceRing: Th.widgetBg
            }
            WidgetText {
                Layout.fillWidth: true
                role: "text"
                text: pr.who.name
                color: pr.isAway ? Th.widgetAroundAwayFg : Th.widgetFg
            }
            StatusChip {
                visible: !!pr.chip
                Layout.maximumWidth: pr.width * 0.5
                app: w.app
                text: pr.chip ? pr.chip.text : ""
                chipColor: pr.chip && pr.chip.color.length > 0 ? pr.chip.color : Th.presenceOnlineDot
                maxWidth: pr.width * 0.5
                pointSize: (Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10) * 0.85
            }
            WidgetText { visible: !pr.chip && pr.elsewhere; role: "meta"; text: "In a call"; color: Th.widgetMetaFg }
        }
    }
    // the M strip (> 3 people): 26 px faces with a 2 px presence ring outside a 1 px gap, gap 6, wrapping, at most two
    // lines (the web keeps 14 faces in 72 px), then "+n"
    component Strip: Flow {
        id: st
        property var people: []
        readonly property var shown: st.people.slice(0, 14)
        width: bodyCol.width
        leftPadding: 8; rightPadding: 8; topPadding: 4; bottomPadding: 6
        spacing: 6
        clip: true
        height: visible ? Math.min(implicitHeight, 72) : 0
        Repeater {
            model: st.shown.length
            delegate: RingFace { required property int index; who: st.shown[index]; size: 26; clickable: true }
        }
        Rectangle {
            visible: st.people.length > st.shown.length
            height: 26; color: "transparent"; width: stripMore.implicitWidth + 10
            Rectangle { anchors.centerIn: parent; width: parent.width; height: 16; radius: 4; color: Th.widgetChipBg }
            WidgetText { id: stripMore; anchors.centerIn: parent; role: "chip"; text: "+" + (st.people.length - st.shown.length); color: Th.widgetChipFg }
        }
    }
    // a face with its status ring (the strip and the summary): the ring (2 px, the presence colour) sits outside a
    // 1 px `widget.bg` gap around the avatar — the face keeps its size, the ring draws outside it
    component RingFace: Item {
        id: rf
        property var who: ({ mxid: "", name: "", avatar: "", pres: null })
        property int size: 26
        property bool clickable: false
        width: rf.size; height: rf.size
        Rectangle { x: -3; y: -3; width: rf.size + 6; height: rf.size + 6; radius: width / 2; color: w.dotOf(rf.who.pres) }
        Rectangle { x: -1; y: -1; width: rf.size + 2; height: rf.size + 2; radius: width / 2; color: Th.widgetBg }
        Avatar { anchors.fill: parent; app: w.app; size: rf.size; seed: rf.who.mxid; name: rf.who.name; imageSource: rf.who.avatar || "" }
        HoverHandler { id: rfHover; enabled: rf.clickable && !w.preview }
        TapHandler { enabled: rf.clickable && !w.preview; onTapped: w.dm(rf.who) }
        Controls.ToolTip.text: rf.who.name + " · " + (rf.who.pres && rf.who.pres.chip ? rf.who.pres.chip : (rf.who.pres ? String(rf.who.pres.label || "") : ""))
        Controls.ToolTip.visible: rfHover.hovered
        Controls.ToolTip.delay: 300
    }
    // enter motion for rows (personSlide): opacity 0→1, y −4→0 (long, OutCubic), through a Translate
    component EnterDown: ParallelAnimation {
        id: ed
        property Item target
        property Translate shift: Translate { y: 0 }
        Component.onCompleted: if (ed.target) ed.target.transform = [ed.shift]
        NumberAnimation { target: ed.target; property: "opacity"; from: 0; to: 1; duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic }
        NumberAnimation { target: ed.shift; property: "y"; from: -4; to: 0; duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic }
        function play() { if (w.hostRef && w.hostRef.durLong > 0) { ed.stop(); ed.start() } }
    }

    // ── right-click: Message · Call · View profile ──
    Controls.Menu {
        id: personMenu
        modal: false
        property var who: null
        function openFor(p) { personMenu.who = p; personMenu.popup() }
        Controls.MenuItem { text: "Message"; icon.name: "dialog-messages-symbolic"; onTriggered: if (personMenu.who) w.dm(personMenu.who) }
        Controls.MenuItem { text: "Call"; icon.name: "call-start-symbolic"; onTriggered: if (personMenu.who) w.callWith(personMenu.who) }
        Controls.MenuItem { text: "View profile"; icon.name: "user-identity-symbolic"; onTriggered: if (personMenu.who && w.app) w.app.openProfileFlyout(personMenu.who.mxid) }
    }

    // ── hover 500 ms → the profile card (the house Popover: avatar 48 · name, id, their presence line) ──
    Controls.Popup {
        id: hoverCard
        parent: Controls.Overlay.overlay
        padding: 12
        modal: false
        focus: false
        closePolicy: Controls.Popup.CloseOnPressOutside | Controls.Popup.CloseOnEscape
        property var who: null
        property var anchorItem: null
        function later(item, p) { hoverCard.anchorItem = item; hoverCard.who = p; hoverTimer.restart() }
        function cancel(item) { if (hoverCard.anchorItem === item) { hoverTimer.stop(); hoverCard.close() } }
        Timer {
            id: hoverTimer
            interval: 500
            onTriggered: {
                if (!hoverCard.anchorItem || w.preview) return
                var pt = hoverCard.anchorItem.mapToItem(Controls.Overlay.overlay, hoverCard.anchorItem.width + 8 + 6, 0)
                hoverCard.x = Math.min(pt.x, Controls.Overlay.overlay.width - hoverCard.implicitWidth - 6)
                hoverCard.y = Math.max(6, Math.min(pt.y, Controls.Overlay.overlay.height - hoverCard.implicitHeight - 6))
                hoverCard.open()
            }
        }
        background: Rectangle { color: Th.popoverBg; border.width: 1; border.color: Th.popoverBorder }
        contentItem: RowLayout {
            spacing: 12
            Avatar { Layout.preferredWidth: 48; Layout.preferredHeight: 48; app: w.app; size: 48; seed: hoverCard.who ? hoverCard.who.mxid : ""; name: hoverCard.who ? hoverCard.who.name : ""; imageSource: hoverCard.who ? (hoverCard.who.avatar || "") : "" }
            Column {
                Layout.minimumWidth: 240 - 48 - 12 - 24
                spacing: 2
                WidgetText { role: "strong"; text: hoverCard.who ? hoverCard.who.name : ""; color: Th.widgetFg }
                WidgetText { role: "meta"; text: hoverCard.who ? hoverCard.who.mxid : ""; color: Th.widgetMetaFg }
                PresenceLine {
                    app: w.app
                    pres: hoverCard.who ? hoverCard.who.pres : null
                    roomName: hoverCard.who && hoverCard.who.pres && hoverCard.who.pres.room && w.app ? w.app.roomNameOf(hoverCard.who.pres.room) : ""
                }
            }
        }
    }

    // ── the sample state (the gallery's preview without real data; the web's sample) ──
    // the gallery / preview sample: the web's sample() (WhosAround.tsx) — the same people, statuses and rooms through
    // the same groupAround, so both apps show the same preview
    readonly property var sampleModel: {
        var none = { playing: [], inCall: [], callElsewhere: [], online: [], away: [], total: 0 }
        if (!w.wc) return none
        var lounge = "!lounge:sample.invalid", squad = "!squad:sample.invalid"
        var st = function (s, text, emoji, color) {
            var o = { state: s, label: s === "call" ? "In a call" : (s === "away" ? "Away" : "Online") }
            if (text) { o.chip = emoji + " " + text; o.chipColor = color; o.dot = color }
            return o
        }
        var rows = {
            "@ana:sample.invalid": { name: "Ana", room: lounge, pres: st("call") },
            "@bo:sample.invalid": { name: "Bo", room: lounge, pres: st("call", "Carrying", "🛡️", "#5865F2") },
            "@cy:sample.invalid": { name: "Cy", room: lounge, pres: st("call") },
            "@dee:sample.invalid": { name: "Dee", room: squad, pres: st("call") },
            "@eli:sample.invalid": { name: "Eli", room: "", pres: st("call") },
            "@fay:sample.invalid": { name: "Fay", room: "", pres: st("online", "Looking for group", "🎯", "#3BA55C") },
            "@gus:sample.invalid": { name: "Gus", room: "", pres: st("online") },
            "@hal:sample.invalid": { name: "Hal", room: "", pres: st("away", "BRB · pizza", "🍕", "#FAA81A") }
        }
        var people = Object.keys(rows).map(function (id) { return { id: id, name: rows[id].name, state: rows[id].pres.state, room: rows[id].room } })
        var g = null
        try { g = JSON.parse(String(w.wc.groupAround(JSON.stringify({ me: "@me:sample.invalid", people: people, activities: { "!lounge:sample.invalid": { game: "Valheim", emoji: "⚔️" } }, showAway: true })))) } catch (e) { g = null }
        if (!g) return none
        var names = { "!lounge:sample.invalid": "Game lounge", "!squad:sample.invalid": "Squad chat" }
        var P = function (id) { return { mxid: id, name: rows[id].name, avatar: "", pres: rows[id].pres } }
        var R = function (r) { return { room: r.room, people: r.people.map(P), name: names[r.room], game: r.game, emoji: r.emoji, cover: "" } }
        return { playing: g.playing.map(R), inCall: g.inCall.map(R), callElsewhere: g.callElsewhere.map(P), online: g.online.map(P), away: g.away.map(P), total: g.total }
    }
}
