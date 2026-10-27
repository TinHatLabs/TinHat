// GameReelPanel — "What do we play tonight" (Game Reel, 9.0). One component for BOTH shells:
// GameReelWindow.qml hosts it. It only speaks to
// the GamesController (`ctl`) and draws with the Th tokens (reel.*) and the host's sizes (`app.radius` …),
// so it takes each shell's theme without a second implementation. Design: DESIGN.md §4.2/4.3.
import QtQuick
import QtQuick.Layouts
import QtQuick.Templates as T
import dev.tinhat.app

Item {
    id: panel
    required property var app           // shell root: radius, dur, localpart(), mUserId
    required property var ctl            // GamesController (gamesCtl)
    property string catalogBase: ctl ? ctl.catalogBase : ""
    property bool showEditor: false      // "My games" editor open
    property bool showAdmin: false       // hidden admin panel (gated on ctl.isAdmin)
    readonly property var snap: {
        try { return JSON.parse(ctl.snapshotJson || "{}") } catch (e) { return {} }
    }
    readonly property var session: snap.session || ({})
    readonly property var roster: snap.roster || ({ expected: [], present: [], flexible: [] })
    readonly property var tiers: snap.tiers || ({ main: [], stretch: [], removed: [] })
    readonly property var spin: snap.spin || null
    readonly property var names: snap.names || ({})
    readonly property var log: snap.log || []
    readonly property var mainPool: tiers.main || []
    // 10.1 (TH-REEL-06): once a spin exists the strip scrolls the IMMUTABLE pool the draw walked
    // (same covers, same odds on every screen); the live main tier is only the idle preview.
    readonly property var stripPool: (spin && spin.pool && spin.pool.length) ? spin.pool : mainPool
    readonly property bool hasResult: !!spin && !!spin.winner && !spin.superseded && !spin.voided
    // 10.5.2 admin "Reset spins" (the web's, 2026-09-23: an admin hit the re-spin limit while testing and had
    // no way back). Voids tonight's spins and stops their re-spins counting (ctl.resetSpins → games.rs
    // do_reset_spins / games_model::reset_writes) in a form every client already reads — desktop 10.5.0 /
    // 10.5.1 included: the result clears, Spin is enabled, and the next spin starts with the full re-spin +
    // veto allowance. (Tune's "Void tonight's spin" only voids the standing record, which falls back to the
    // previous result with the count unchanged.) Shown next to the count while a result stands, and also when
    // nothing stands but used re-spins still count (Tune voids, or a reset that failed half-way) — so there
    // is always a way back.
    readonly property bool canReset: !!ctl && ctl.isAdmin && (hasResult || (!!spin && (spin.respinsUsed || 0) > 0))
    function nameOf(mxid) { return names[mxid] || app.localpart(mxid) }
    function art(id) { return catalogBase + "/art/" + encodeURIComponent(id) + "/portrait" }
    function pct(o) { return Math.round((o || 0) * 100) + "%" }

    // ---- reel animation ---------------------------------------------------------------------
    // The strip = the pool repeated `laps` times plus a partial lap that ends on the winner; one
    // NumberAnimation eases the row to the winner's slot. Every client computes the same winner
    // from the spin record, so everyone lands on the same cover (DESIGN §8).
    // 10.1 (TH-REEL-07): the admin's "Spin length" / laps are honoured (they were constants).
    property int laps: (snap.reelSettings && snap.reelSettings.laps > 0) ? snap.reelSettings.laps : 5
    property int durationMs: (snap.reelSettings && snap.reelSettings.durationMs > 0) ? snap.reelSettings.durationMs : 6000
    property real coverW: 132
    property real coverGap: 14
    property string spinningFor: ""      // spin id currently animating
    property string landedOn: ""         // winner id once the animation finished
    readonly property bool spinning: reelAnim.running
    function startReel(spinId, winnerId) {
        if (!stripPool.length) return
        var idx = -1
        for (var i = 0; i < stripPool.length; i++) if (stripPool[i].id === winnerId) idx = i
        if (idx < 0) { landedOn = winnerId; return }
        var slot = laps * stripPool.length + idx
        spinningFor = spinId; landedOn = ""
        var target = -(slot * (coverW + coverGap)) + (reelViewport.width - coverW) / 2
        reelAnim.stop()
        strip.x = (reelViewport.width - coverW) / 2
        reelAnim.to = target
        // 11.0 (GPU.md R4): the spin (the one long motion left — a one-shot the user started) runs only while the motion
        // switch is on; otherwise it jumps to its result
        reelAnim.duration = Motion.animate ? durationMs : 0
        reelAnim.start()
    }
    // … and the moment the window loses the focus (or Reduce motion comes on) mid-roll, it lands at once
    Connections {
        target: Motion
        function onAnimateChanged() { if (!Motion.animate && reelAnim.running) reelAnim.complete() }
    }
    Connections {
        target: panel
        function onSpinChanged() {
            if (!panel.spin || !panel.spin.winner) return
            if (panel.spin.id !== panel.spinningFor && panel.spin.id !== panel._seenSpin) { panel._seenSpin = panel.spin.id; panel.startReel(panel.spin.id, panel.spin.winner) }
        }
    }
    property string _seenSpin: ""
    // Session: the next session today-ish without a game, unless the host set one (ctl.sessionId).
    readonly property var upcoming: app.mCalUpcoming || []
    onUpcomingChanged: pickSession()
    Component.onCompleted: pickSession()
    // 10.5.1: only a session that starts before tonight's game night ends (the next 14:00 UTC, a
    // fixed day boundary — games_model::night_end_ms) auto-picks, and an auto-pick is dropped once it no
    // longer is tonight's (moved, canceled, over). With none the reel spins for tonight on its own
    // (the controller's ad-hoc `night-…` session) instead of writing tonight's pick into next
    // week's event. An explicit pick (the calendar's "Pick tonight's game") stands while its event
    // exists, isn't canceled and hasn't ended (a proposal without a time counts as open). The web's
    // gamesStore pickSession is the same rule.
    property string _autoPicked: ""
    readonly property var calEvents: app.mCalEvents || []
    onCalEventsChanged: pickSession()
    function nightEndMs(now) { var off = 14 * 3600000, day = 86400000; return (Math.floor(Math.max(0, now - off) / day) + 1) * day + off }
    function pickSession() {
        if (!ctl) return
        var cur = ctl.sessionId
        var now = Date.now(), nightEnd = nightEndMs(now)
        function tonights(e) { return !!e && !e.canceled && e.kind !== "focus" && e.effStart > 0 && e.effStart < nightEnd }
        var i
        if (cur.length > 0 && cur === _autoPicked) {
            for (i = 0; i < upcoming.length; i++) if (upcoming[i] && upcoming[i].id === cur && tonights(upcoming[i])) return
        } else if (cur.length > 0) {
            for (i = 0; i < upcoming.length; i++) if (upcoming[i] && upcoming[i].id === cur) return   // still upcoming
            for (i = 0; i < calEvents.length; i++) {
                var ev = calEvents[i]
                if (ev && ev.id === cur && !ev.canceled && (!(ev.effStart > 0) || ev.effEnd >= now)) return   // not over yet
            }
        }
        var pick = null
        for (var k = 0; k < upcoming.length; k++) {
            var e = upcoming[k]
            if (!tonights(e)) continue
            if (!e.game || !String(e.game).length) { pick = e; break }
            if (!pick) pick = e
        }
        if (pick) { _autoPicked = pick.id; if (pick.id !== cur) ctl.setSession(pick.id) }
        else if (cur.length > 0) { _autoPicked = ""; ctl.setSession("") }
    }
    // 10.5.1 open roster: everyone here (in the call, RSVP or not) solid, then RSVP'd people who
    // aren't here yet dimmed. Who is HERE is the snapshot's `roster.live`: outside a call the open
    // roster is a preview of the RSVP'd + me, so only the viewer is (it said "4 here" with nobody
    // in a call). Older snapshots without `live`: the actual roster.
    readonly property var liveIds: roster.live || (roster.present || []).map(function (p) { return p.mxid })
    readonly property var faces: {
        var pr = roster.present || [], out = [], seen = {}, i
        for (i = 0; i < pr.length; i++) if (liveIds.indexOf(pr[i].mxid) >= 0) { out.push(pr[i]); seen[pr[i].mxid] = true }
        for (i = 0; i < pr.length; i++) if (!seen[pr[i].mxid]) { out.push(pr[i]); seen[pr[i].mxid] = true }
        var ex = roster.expected || []
        for (var k = 0; k < ex.length; k++) if (!seen[ex[k].mxid]) out.push(ex[k])
        return out
    }
    function isHere(mxid) { return liveIds.indexOf(mxid) >= 0 }
    function hereText() {
        var ex = roster.expected || []
        if (roster.presence && roster.presence !== "open") return (roster.present || []).length + " of " + ex.length + " here"
        var coming = 0
        for (var k = 0; k < ex.length; k++) if (!isHere(ex[k].mxid)) coming++
        return liveIds.length + " here" + (coming > 0 ? " · " + coming + " more coming" : "")
    }
    // 10.1 (TH-REEL-17): the landing gets a payoff — a blip through the app's sound route (honours
    // the sound switch + volume) and the winner cover pulses (ReelCover). Ticks per cover are
    // still open (they need an eased timer).
    NumberAnimation { id: reelAnim; target: strip; property: "x"; easing.type: Easing.OutCubic; onFinished: { panel.landedOn = panel.spin ? panel.spin.winner : ""; panel.spinningFor = ""; if (panel.landedOn.length > 0 && panel.app && panel.app.playUiSound) panel.app.playUiSound("receive") } }

    // A host may put the panel on a glass surface; painting the canvas colour over it left an
    // opaque black slab in the middle of the page.
    Rectangle { anchors.fill: parent; color: Th.reelBg }

    // 10.1 (TH-REEL-20): while the Matrix state + catalog lookups run, say so — the page used to
    // render "No session picked · Game TBD / Nobody's here yet" until the answer landed.
    Rectangle {
        anchors.fill: parent
        z: 5
        color: Th.reelLoadingBg
        visible: ctl.busy && !snap.session && !panel.hasResult
        EmptyState {
            anchors.centerIn: parent
            app: panel.app
            loading: true
            title: "Loading tonight's reel…"
        }
    }
    T.ScrollView {
        id: scroll
        anchors.fill: parent
        contentWidth: availableWidth
        T.ScrollBar.vertical: FoxScrollBar { app: panel.app; policy: T.ScrollBar.AsNeeded }   // 10.1 (TH-REEL-14): a Templates ScrollView draws no bar at all
        contentItem: Flickable {
            contentHeight: body.implicitHeight + 32
            clip: true
            ColumnLayout {
                id: body
                x: 16; y: 16
                width: scroll.availableWidth - 32
                spacing: 14

                // ---- header ----
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 12
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Text { textFormat: Text.PlainText; text: session.title && session.title.length ? session.title : "Tonight"; color: Th.reelFg; font.pixelSize: 22; font.weight: Font.DemiBold; elide: Text.ElideRight; Layout.fillWidth: true }
                        Text {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            text: (session.adhoc ? "No calendar session" : session.start ? Qt.formatDateTime(new Date(session.start), "ddd d MMM · h:mm AP") : "No session picked")
                                  + (session.game && session.game.length ? " · " + session.game : " · Game TBD")
                            color: Th.reelMetaFg; font.pixelSize: 13; elide: Text.ElideRight; Layout.fillWidth: true
                        }
                    }
                    Text { textFormat: Text.PlainText; text: panel.hereText(); color: Th.reelMetaFg; font.pixelSize: 13 }
                    Row {
                        spacing: -8
                        Repeater {
                            model: panel.faces
                            delegate: Avatar {
                                required property var modelData
                                app: panel.app
                                size: 28
                                name: modelData.name || panel.app.localpart(modelData.mxid)
                                seed: modelData.mxid
                                imageSource: modelData.avatar || ""
                                opacity: panel.isHere(modelData.mxid) ? 1 : 0.4
                            }
                        }
                    }
                    ReelButton { text: "My games"; onClicked: panel.showEditor = !panel.showEditor; radius: panel.app.radius; checked: panel.showEditor }
                    ReelButton { visible: ctl.isAdmin; text: "Tune"; quiet: true; onClicked: panel.showAdmin = !panel.showAdmin; radius: panel.app.radius; checked: panel.showAdmin }
                }
                RowLayout {
                    visible: ctl.lastError.length > 0
                    Layout.fillWidth: true
                    spacing: 8
                    Text { textFormat: Text.PlainText; text: ctl.lastError; color: Th.reelErrorFg; font.pixelSize: 13; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                    // 10.1 (TH-GAP-19): the error line used to stick for the whole session.
                    ReelButton { text: "✕"; quiet: true; implicitWidth: 26; implicitHeight: 24; onClicked: ctl.clearError(); radius: panel.app.radiusSm; hint: "Dismiss" }
                }

                // ---- my games editor ----
                MyGamesEditor { visible: panel.showEditor; Layout.fillWidth: true; app: panel.app; ctl: panel.ctl; catalogBase: panel.catalogBase }

                // ---- admin ----
                GameNightAdmin { visible: panel.showAdmin && ctl.isAdmin; Layout.fillWidth: true; app: panel.app; ctl: panel.ctl }

                // ---- the reel ----
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: 292
                    radius: app.radiusLg
                    color: Th.reelStripBg
                    border.color: Th.reelStripBorder
                    clip: true
                    Text {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        visible: !panel.mainPool.length
                        anchors.centerIn: parent
                        width: parent.width - 48
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                        color: Th.reelMetaFg; font.pixelSize: 14
                        // 10.1 (TH-REEL-03): the snapshot's own `notice` says exactly why — use it.
                        text: (snap.notice && snap.notice.length > 0) ? snap.notice
                              : (roster.present || []).length === 0 ? "Nobody’s here yet — the reel fills in as people show up."
                              : (tiers.stretch && tiers.stretch.length) ? "No game everyone here owns. The ‘most of us own’ tier below can be spun if the admin allows it."
                              : "No game everyone here owns. Add games in My games."
                    }
                    Item {
                        id: reelViewport
                        anchors { left: parent.left; right: parent.right; top: parent.top; bottom: parent.bottom; topMargin: 14; bottomMargin: 14 }
                        visible: panel.stripPool.length > 0
                        Row {
                            id: strip
                            spacing: panel.coverGap
                            x: (reelViewport.width - panel.coverW) / 2 - (panel.landedOn.length ? 0 : 0)
                            // 10.1 (TH-REEL-17): a tick each time a cover crosses the centre marker while the
                            // reel spins (the app's sound route throttles a kind to ~3/s, so the fast start
                            // is a purr and the slow finish is single ticks — the suspense curve for free).
                            property int _tickIdx: -1
                            onXChanged: {
                                if (!panel.spinning) return
                                var centre = (reelViewport.width - panel.coverW) / 2
                                var idx = Math.floor((centre - x) / (panel.coverW + panel.coverGap) + 0.5)
                                if (idx !== _tickIdx) { _tickIdx = idx; if (panel.app && panel.app.playUiSound) panel.app.playUiSound("send") }
                            }
                            Repeater {
                                model: panel.stripPool.length ? panel.stripPool.length * (panel.laps + 1) : 0
                                delegate: ReelCover {
                                    required property int index
                                    readonly property var g: panel.stripPool[index % panel.stripPool.length]
                                    app: panel.app
                                    game: g
                                    artUrl: panel.art(g.id)
                                    width: panel.coverW
                                    height: reelViewport.height
                                    winner: panel.landedOn.length > 0 && g.id === panel.landedOn && index === panel.laps * panel.stripPool.length + (panel.stripPool.map(function (x) { return x.id }).indexOf(panel.landedOn))
                                }
                            }
                        }
                        // centre marker
                        Rectangle { anchors.horizontalCenter: parent.horizontalCenter; anchors.top: parent.top; width: 3; height: 10; radius: 1.5; color: Th.reelStripMarkerFill }
                        Rectangle { anchors.horizontalCenter: parent.horizontalCenter; anchors.bottom: parent.bottom; width: 3; height: 10; radius: 1.5; color: Th.reelStripMarkerFill }
                    }
                }

                // ---- controls ----
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 10
                    // 10.1 (TH-REEL-08): once a spin stands the primary button IS the re-spin (respin() keeps
                    // tonight's vetoes and counts against the limit) — it was a permanently grey "Spin again"
                    // next to a "2 re-spins left" label with no button for them.
                    ReelButton {
                        text: panel.spinning ? "Spinning…" : (panel.hasResult ? "Re-spin" : "Spin")
                        primary: true
                        enabled: !panel.spinning && !ctl.busy && !ctl.resetting && (panel.hasResult ? (snap.respinsLeft > 0) : snap.canSpin === true)
                        hint: panel.hasResult ? "Spin again, keeping tonight's vetoes — " + snap.respinsLeft + " left" : ""
                        onClicked: panel.hasResult ? ctl.respin() : ctl.spin()
                        radius: panel.app.radius
                    }
                    ReelButton {
                        visible: panel.hasResult
                        text: "Veto"
                        enabled: snap.canVeto === true && !panel.spinning && !ctl.busy && !ctl.resetting
                        hint: "One per person per night; the reel re-spins without this game" + (snap.myVetoesLeft !== undefined ? " — " + snap.myVetoesLeft + " left" : "")
                        onClicked: ctl.veto()
                        radius: panel.app.radius
                    }
                    Text { textFormat: Text.PlainText; visible: panel.hasResult && (snap.respinsLeft !== undefined); text: snap.respinsLeft + " re-spin" + (snap.respinsLeft === 1 ? "" : "s") + " left"; color: Th.reelMetaFg; font.pixelSize: 12 }
                    ReelButton {
                        visible: panel.canReset
                        text: "Reset spins"
                        quiet: true
                        enabled: !panel.spinning && !ctl.busy && !ctl.resetting
                        hint: "Admin: void tonight's spins so the next spin starts fresh — everyone gets their re-spins and vetoes back"
                        onClicked: resetConfirm.open()
                        radius: panel.app.radius
                    }
                    // TH-REEL-03: why Spin is grey, in the snapshot's own words.
                    Text { textFormat: Text.PlainText; visible: !panel.hasResult && !!(snap.notice && snap.notice.length > 0) && panel.mainPool.length > 0; text: snap.notice ? String(snap.notice) : ""; color: Th.reelMetaFg; font.pixelSize: 12; elide: Text.ElideRight; Layout.maximumWidth: 360 }
                    // 10.5.2 settings credits.mode "free" (the web's "Free spins" chip)
                    Rectangle {
                        visible: snap.creditsMode === "free"
                        implicitWidth: freeText.implicitWidth + 16
                        implicitHeight: 22
                        radius: 11
                        color: "transparent"
                        border.color: Th.reelFreeChipBorder
                        Text { id: freeText; anchors.centerIn: parent; text: "Free spins"; color: Th.reelFreeChipFg; font.pixelSize: 11 }
                        HoverHandler { id: freeHover }
                        T.ToolTip.visible: freeHover.hovered
                        T.ToolTip.text: "An admin turned credits off in Tune: spins from now on don't earn or spend credit, and credit doesn't weight the reel. Each result says whether it counts."
                        T.ToolTip.delay: 500
                    }
                    Item { Layout.fillWidth: true }
                    // TH-REEL-09: the race / void explanation the snapshot writes (`spin.note`); the old
                    // hard-coded "two people spun at once" text was unreachable (superseded is always false).
                    Text { textFormat: Text.PlainText; visible: !!(panel.spin && panel.spin.note && panel.spin.note.length > 0); text: (panel.spin && panel.spin.note) ? String(panel.spin.note) : ""; color: Th.reelWarningFg; font.pixelSize: 12; wrapMode: Text.WordWrap; Layout.maximumWidth: 420 }
                    Text { visible: !!(panel.spin && panel.spin.verified === false); text: "Could not verify this spin’s winner on this device."; color: Th.reelWarningFg; font.pixelSize: 12 }
                }

                // ---- result ----
                Rectangle {
                    visible: panel.hasResult && panel.landedOn.length > 0
                    Layout.fillWidth: true
                    implicitHeight: 118
                    radius: app.radiusLg
                    color: Th.reelResultBg
                    border.color: Th.reelResultBorder
                    RowLayout {
                        anchors.fill: parent
                        anchors.margins: 12
                        spacing: 14
                        Image { source: panel.spin ? panel.art(panel.spin.winner) : ""; Layout.preferredWidth: 62; Layout.preferredHeight: 93; fillMode: Image.PreserveAspectCrop; asynchronous: true }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 3
                            Text { textFormat: Text.PlainText; text: "Tonight: " + panel.nameOfGame(panel.spin ? panel.spin.winner : ""); color: Th.reelFg; font.pixelSize: 18; font.weight: Font.DemiBold; elide: Text.ElideRight; Layout.fillWidth: true }
                            Text { textFormat: Text.PlainText; text: panel.ownersLine(panel.spin ? panel.spin.winner : ""); color: Th.reelMetaFg; font.pixelSize: 12; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                            Text { textFormat: Text.PlainText; text: (session.adhoc ? "Just for tonight (no calendar session)" : session.gameId === (panel.spin ? panel.spin.winner : "-") ? "✓ Set on the calendar" : "Setting on the calendar…") + (panel.spin && panel.spin.by ? " · spun by " + panel.nameOf(panel.spin.by) : "") + (panel.spin && panel.spin.credit === false ? " · not counted for credit" : ""); color: Th.reelMetaFg; font.pixelSize: 12; elide: Text.ElideRight; Layout.fillWidth: true }
                        }
                        // 10.5.2 admin: tonight's result stays, only credit ignores it (and back)
                        ReelButton {
                            visible: !!ctl && ctl.isAdmin
                            readonly property bool uncounted: !!panel.spin && panel.spin.credit === false
                            text: uncounted ? "Count for credit" : "Don’t count for credit"
                            quiet: true
                            implicitHeight: 28
                            enabled: !panel.spinning && !ctl.busy && !ctl.resetting
                            hint: uncounted ? "Admin: this spin counts for credit again (a lost favourite earns, a winning one spends)" : "Admin: tonight's result stands, but nobody's credit changes because of it"
                            onClicked: if (panel.spin) ctl.setSpinCredit(panel.spin.id, uncounted)
                            radius: panel.app.radiusSm
                        }
                    }
                }

                // ---- tiers ----
                TierList { Layout.fillWidth: true; app: panel.app; panel: panel; title: "Everyone here owns"; games: tiers.main || []; artFn: panel.art }
                TierList { visible: (tiers.stretch || []).length > 0; Layout.fillWidth: true; app: panel.app; panel: panel; title: "Most of us own"; subtitle: "Shown, not spun, unless the admin turns the stretch tier on."; games: tiers.stretch || []; artFn: panel.art; dim: true }
                TierList { visible: (tiers.removed || []).length > 0; Layout.fillWidth: true; app: panel.app; panel: panel; title: "Removed tonight"; games: tiers.removed || []; artFn: panel.art; dim: true; showReason: true }

                // ---- log ----
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 6
                    visible: panel.log.length > 0
                    Text { text: "Past nights"; color: Th.reelSectionFg; font.pixelSize: 12; font.letterSpacing: 1; font.capitalization: Font.AllUppercase }
                    Repeater {
                        model: panel.log.filter(function (e) { return e.current !== true }).slice(0, 12)   // 10.1 (TH-REEL-24): tonight is above, not in "past nights"
                        // 10.5.2: "not counted for credit" + (admins) "Don't count for credit" / "Count for credit"
                        delegate: RowLayout {
                            required property var modelData
                            readonly property bool uncounted: !modelData.voided && modelData.credit === false
                            Layout.fillWidth: true
                            spacing: 8
                            Text {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.fillWidth: true
                                wrapMode: Text.WordWrap
                                color: Th.reelMetaFg; font.pixelSize: 12
                                text: (modelData.date ? Qt.formatDate(new Date(modelData.date), "d MMM") : "") + " · " + (modelData.winnerName || modelData.winner) + " · "
                                      + (modelData.present || []).map(function (m) { return panel.nameOf(m) }).join(", ")
                                      + ((modelData.creditChanges || []).length ? " · credit " + modelData.creditChanges.map(function (c) { return panel.nameOf(c.mxid) + (c.delta > 0 ? " +" : " ") + c.delta }).join(", ") : "")
                                      + (uncounted ? " · not counted for credit" : "")
                            }
                            ReelButton {
                                visible: !!ctl && ctl.isAdmin && !modelData.voided && !!modelData.spinId
                                text: uncounted ? "Count for credit" : "Don’t count for credit"
                                quiet: true
                                implicitHeight: 22
                                enabled: !panel.spinning && !ctl.busy && !ctl.resetting
                                hint: uncounted ? "Admin: this night counts for credit again" : "Admin: this night's result stays, but nobody's credit changes because of it"
                                onClicked: ctl.setSpinCredit(modelData.spinId, uncounted)
                                radius: panel.app.radiusSm
                            }
                        }
                    }
                }
            }
        }
    }
    ConfirmDialog {
        id: resetConfirm
        app: panel.app
        dialogTitle: "Reset tonight's spins?"
        message: "Tonight's spins are voided and the next spin starts fresh — everyone gets their re-spins and vetoes back."
        confirmText: "Reset spins"
        destructive: true
        onConfirmed: panel.ctl.resetSpins()
    }
    function nameOfGame(id) {
        var all = (tiers.main || []).concat(tiers.stretch || [], tiers.removed || [])
        for (var i = 0; i < all.length; i++) if (all[i].id === id) return all[i].name
        return id
    }
    function ownersLine(id) {
        var all = (tiers.main || []).concat(tiers.stretch || [], tiers.removed || [])
        for (var i = 0; i < all.length; i++) if (all[i].id === id) {
            var o = (all[i].owners || []).map(function (m) { return panel.nameOf(m) })
            return o.length ? "Owned by " + o.join(", ") : ""
        }
        return ""
    }
}
