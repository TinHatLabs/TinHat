// OverlayWindow — the voice HUD: a small stack of "who's talking" pills pinned to a
// screen corner, visible over fullscreen games.
//
// HOW IT STAYS OUT OF THE GAME'S WAY (the whole point of this design):
//   * It is a wlr-layer-shell surface on the OVERLAY layer, composited by KWin ON TOP of
//     fullscreen windows. We never touch, hook, or even look at the game process — no
//     injection, no process/window scanning — so there is nothing for an anti-cheat to
//     object to and no code running inside the game's render loop.
//   * Click-through + focusless: Qt.WindowTransparentForInput (empty Wayland input
//     region) and KeyboardInteractivityNone. The game keeps every input event.
//   * No continuously-running animations, and nothing binds to a raw audio LEVEL — only
//     to the derived speaking BOOL — so the surface only damages (and only costs the
//     compositor anything) when someone starts/stops talking, not 60×/sec.
//   * In "speaking" mode the Window turns invisible when nobody talks, which UNMAPS the
//     layer surface entirely: KWin can go back to direct scanout of the fullscreen game,
//     the zero-overhead path. ("always" mode keeps the roster up for the whole call.)
//
// Data flows in via `app` (Main.qml root): the overlay reuses the very same mirrors and
// speaking threshold that drive the in-call tile waveforms (mVideoTiles, mAudioLevels,
// tileAudioLevel()), so a pill lights up exactly when the in-app tile does.
//
// Wayland lifecycle notes: flags are CONSTANT (changing them recreates the platform
// surface — see FloatingChats.qml). Unlike a frameless xdg_toplevel, a LAYER surface is
// designed for map/unmap cycling (Plasma's own OSDs do it), which "speaking" mode relies
// on. On X11 or a compositor without layer-shell the attached properties are inert and
// the window degrades to a best-effort frameless always-on-top HUD.
import QtQuick
import QtQuick.Window
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the overlay.* tokens
import org.kde.layershell as LayerShell

Window {
    id: hud
    required property var app

    // ---- settings (all persisted via SettingsController; mirrored on app root) ----
    readonly property bool hudEnabled: app.mOverlayEnabled
    readonly property string mode: app.mOverlayMode === "always" ? "always" : "speaking"
    readonly property string corner: app.mOverlayCorner || "tl"
    readonly property bool hideWhenSharing: app.mOverlayHideShare

    // ---- preview (settings "Preview" button; TINHAT_OVERLAY_DEMO=1 pins it for benchmarks) ----
    property bool _manualPreview: false
    readonly property bool previewing: app.mOverlayDemo || _manualPreview
    property int _previewTick: 0
    function startPreview() { _manualPreview = true; previewEnd.restart(); recompute() }

    // ---- tuning ----
    // Speaking is decided by the SFU's active-speaker list (app.mSpeakers) — server-side
    // voice detection from the RTP audio-level extension. NOT a local RMS floor: with
    // AGC on, ambient noise alone reads ~0.03-0.08 RMS and a fixed floor would pin the
    // self pill up forever. The talking-while-muted warning (the muted track sends nothing,
    // so the server can't see it) follows the streamer's detector since 10.5 (hear.rs:
    // sustained speech against YOUR learned speech level, never the call's own echo).
    readonly property int lingerMs: 1800      // pill stays up briefly after the last word
    readonly property int maxPills: 8

    // ---- the pill model: [{key,label,speaking,muted,deafened,self,sharing,more,warn}] ----
    // Recomputed on data changes, but ASSIGNED only when it truly changed (house pattern:
    // stringify + diff) so bindings — and therefore surface damage — stay quiet between
    // speaking-state transitions.
    property var pills: []
    property string _pillsRaw: ""
    property var _lastSpoke: ({})   // pill key -> Date.now() of last above-floor sample

    visible: pills.length > 0
             && (previewing
                 || (hudEnabled && app.mInCall && !(hideWhenSharing && app.mCallSharing)))
    flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.WindowTransparentForInput
           | Qt.WindowDoesNotAcceptFocus
    color: "transparent"
    title: "Tin Hat overlay"
    objectName: "voice-overlay"

    // KWin: overlay layer (above fullscreen), no keyboard, hug the true screen corner
    // (exclusionZone -1 ignores panel reservations, like a fullscreen window does), and
    // follow the ACTIVE screen — the compositor decides where the action is, we never ask.
    LayerShell.Window.layer: LayerShell.Window.LayerOverlay
    LayerShell.Window.keyboardInteractivity: LayerShell.Window.KeyboardInteractivityNone
    LayerShell.Window.exclusionZone: -1
    LayerShell.Window.wantsToBeOnActiveScreen: true
    LayerShell.Window.scope: "tinhat-voice-hud"
    LayerShell.Window.anchors: corner === "tr" ? (LayerShell.Window.AnchorTop | LayerShell.Window.AnchorRight)
                             : corner === "bl" ? (LayerShell.Window.AnchorBottom | LayerShell.Window.AnchorLeft)
                             : corner === "br" ? (LayerShell.Window.AnchorBottom | LayerShell.Window.AnchorRight)
                             : (LayerShell.Window.AnchorTop | LayerShell.Window.AnchorLeft)

    // Content-sized: the transparent window is exactly the pill stack + an edge inset.
    readonly property int edgePad: Kirigami.Units.largeSpacing * 2
    width: Math.max(1, pillCol.implicitWidth + edgePad * 2)
    height: Math.max(1, pillCol.implicitHeight + edgePad * 2)


    // ---- data triggers: recompute when the mirrors move (all cheap, all change-guarded) ----
    readonly property var trigTiles: app.mVideoTiles
    onTrigTilesChanged: recompute()
    readonly property var trigSpeakers: app.mSpeakers
    onTrigSpeakersChanged: recompute()
    // 10.5: talking into a muted mic, as hear.rs measures it (the old per-sample streak here used a
    // fixed 0.12 RMS bar that a quiet mic never reached and loud game audio did).
    readonly property bool trigMutedTalk: !!(app.mHear && app.mHear.mutedTalk)
    onTrigMutedTalkChanged: recompute()
    readonly property bool trigMicMuted: app.mMicMuted
    onTrigMicMutedChanged: {
        if (!app.mMicMuted) {
            // Unmuting forgives the warning immediately — otherwise a quick
            // unmute-say-a-word-remute rhythm resurrects a stale "muted!" nag.
            _lastSpoke["__mutedtalk"] = 0
        }
        recompute()
    }
    readonly property bool trigDeafened: app.mDeafened
    onTrigDeafenedChanged: recompute()
    readonly property bool trigInCall: app.mInCall
    onTrigInCallChanged: recompute()
    onModeChanged: recompute()
    onHudEnabledChanged: recompute()
    onPreviewingChanged: recompute()

    // Linger decay: a pill stays 1.8 s after the last word, and the muted warning expires 1.8 s after the last muted
    // speech. 11.0 (GPU.md §2.8, R14): no clock any more — ONE timeout at the next linger end (recompute() arms it), on
    // top of the data triggers above; it ran a 300 ms clock whenever pills showed (over a game, for the whole call).
    Timer { id: lingerEnd; repeat: false; onTriggered: hud.recompute() }
    Timer { id: previewEnd; interval: 10000; onTriggered: hud._manualPreview = false }
    Timer { // preview: rotate which fake pill "speaks" so positioning/readability can be judged
        interval: 1100; repeat: true; running: hud.previewing
        onTriggered: { hud._previewTick = (hud._previewTick + 1) % 3; hud.recompute() }
    }

    function fakePills() {
        // Exercises every badge: self, speaking (rotates, with a "nobody" beat), a
        // screen-sharer, and a muted peer (who therefore never speaks).
        return [
            { key: "you", label: "You", speaking: _previewTick === 0, muted: false,
              deafened: false, self: true, sharing: false, more: false, warn: false },
            { key: "p1", label: "Alex", speaking: _previewTick === 1, muted: false,
              deafened: false, self: false, sharing: true, more: false, warn: false },
            { key: "p2", label: "Ripley", speaking: false, muted: true,
              deafened: false, self: false, sharing: false, more: false, warn: false }
        ]
    }

    // Is `identity` in the SFU's active-speaker list? Speakers carry the full LiveKit
    // identity (@user:server:DEVICE); tiles may carry either form — match by mxid
    // prefix exactly like tileAudioLevel / the #5 gain path.
    function speakingNow(identity) {
        var act = (app.mSpeakers && app.mSpeakers.active) ? app.mSpeakers.active : []
        var p = identity || ""
        for (var i = 0; i < act.length; ++i) {
            var a = act[i]
            if (a === p || p.indexOf(a + ":") === 0 || a.indexOf(p + ":") === 0) return true
        }
        return false
    }

    function recompute() {
        var active = previewing || (hudEnabled && app.mInCall)
        if (!active) {
            if (_pillsRaw !== "") { _pillsRaw = ""; pills = []; _lastSpoke = ({}) }
            return
        }
        var out
        if (previewing) {
            out = fakePills()
        } else {
            var now = Date.now()
            var speakingOnly = (mode === "speaking")
            out = []

            // --- self pill ---
            var me = (app.mSpeakers && app.mSpeakers.me) ? app.mSpeakers.me : ""
            var selfSpeaking = !app.mMicMuted && me.length > 0 && speakingNow(me)
            // Talking into a muted mic: the reminder every voice app should have had.
            // The server can't see a muted track, so this one IS local level — but with
            // a high bar, sustained (streak advances per fresh sample in onTrigMicChanged).
            if (selfSpeaking) _lastSpoke["__self"] = now
            if (app.mMicMuted && trigMutedTalk) _lastSpoke["__mutedtalk"] = now
            var selfLinger = (_lastSpoke["__self"] || 0) + lingerMs > now
            var warnLinger = app.mMicMuted && (_lastSpoke["__mutedtalk"] || 0) + lingerMs > now
            if (!speakingOnly || selfSpeaking || selfLinger || warnLinger) {
                out.push({ key: "__self", label: "You", speaking: selfSpeaking,
                           muted: app.mMicMuted, deafened: app.mDeafened, self: true,
                           sharing: app.mCallSharing, more: false, warn: warnLinger })
            }

            // --- remote pills (dedupe mVideoTiles by identity; skip the self-view entry) ---
            var seen = {}
            var remotes = []
            var tiles = app.mVideoTiles || []
            for (var i = 0; i < tiles.length; ++i) {
                var t = tiles[i]
                var p = t.participant || ""
                if (!p || p === "You" || t.id === "vid_selfview") continue
                if (seen[p] === undefined) {
                    seen[p] = remotes.length
                    remotes.push({ key: p, label: t.name || app.localpart(p),
                                   muted: !!t.muted, sharing: !!t.screenshare })
                } else {
                    var r0 = remotes[seen[p]]
                    if (t.screenshare) r0.sharing = true
                    if (t.kind !== "screenshare") r0.muted = !!t.muted  // mic state rides the non-share entry
                }
            }
            for (var j = 0; j < remotes.length; ++j) {
                var r = remotes[j]
                var speaking = !r.muted && speakingNow(r.key)
                if (speaking) _lastSpoke[r.key] = now
                var linger = (_lastSpoke[r.key] || 0) + lingerMs > now
                if (!speakingOnly || speaking || linger) {
                    out.push({ key: r.key, label: r.label, speaking: speaking, muted: r.muted,
                               deafened: false, self: false, sharing: r.sharing,
                               more: false, warn: false })
                }
            }

            // self first, then alphabetical (identity tiebreak) — a CONSISTENT comparator
            // (returns 0 for true ties), so equal labels can't shuffle between sorts
            out.sort(function (a, b) {
                if (a.self !== b.self) return a.self ? -1 : 1
                var la = a.label.toLowerCase(), lb = b.label.toLowerCase()
                if (la !== lb) return la < lb ? -1 : 1
                if (a.key !== b.key) return a.key < b.key ? -1 : 1
                return 0
            })
            if (out.length > maxPills) {
                var extra = out.length - (maxPills - 1)
                out = out.slice(0, maxPills - 1)
                out.push({ key: "__more", label: "+" + extra, speaking: false, muted: false,
                           deafened: false, self: false, sharing: false, more: true, warn: false })
            }
        }
        var json = JSON.stringify(out)
        if (json !== _pillsRaw) { _pillsRaw = json; pills = out }
        hud._armLinger()
    }
    // the next moment a lingering pill (or the muted warning) must go: the earliest last-spoke + lingerMs still ahead
    function _armLinger() {
        var now = Date.now(), next = 0
        if (!previewing && pills.length > 0) {
            for (var k in _lastSpoke) {
                var end = (_lastSpoke[k] || 0) + lingerMs
                if (end > now && (next === 0 || end < next)) next = end
            }
        }
        if (next === 0) { lingerEnd.stop(); return }
        lingerEnd.interval = Math.max(1, next - now + 5)
        lingerEnd.restart()
    }

    Component.onCompleted: {
        console.log("[overlay] created: enabled=" + hudEnabled + " demo=" + app.mOverlayDemo
                    + " previewing=" + previewing + " mode=" + mode + " corner=" + corner)
        recompute()
    }
    // Transition-only, count-only: enough to diagnose "overlay never shows / never hides"
    // from a friend's diagnostics report without writing identities into the journal.
    onVisibleChanged: console.log("[overlay] visible=" + visible + " pills=" + pills.length)

    Column {
        id: pillCol
        x: hud.edgePad
        // bottom corners stack upward from the screen edge (newest visual weight stays low)
        y: (hud.corner === "bl" || hud.corner === "br")
           ? hud.height - hud.edgePad - implicitHeight : hud.edgePad
        width: implicitWidth
        spacing: Kirigami.Units.smallSpacing

        Repeater {
            model: hud.pills
            delegate: Rectangle {
                id: pill
                required property var modelData
                readonly property bool lit: modelData.speaking === true
                readonly property bool warn: modelData.warn === true

                // Natural (content) width — a positioner Column derives ITS implicit width
                // from children's actual widths, so pills must not bind back to it (that
                // fixed-point resolves to 0). Right corners right-align via x instead.
                x: (hud.corner === "tr" || hud.corner === "br") ? pillCol.width - width : 0
                implicitWidth: row.implicitWidth + Kirigami.Units.largeSpacing * 2
                implicitHeight: Kirigami.Units.gridUnit * 1.9
                radius: height / 2
                // near-black translucent so it reads over bright AND dark game scenes
                color: warn ? Th.overlayPillBgError
                     : lit ? Th.overlayPillBgSpeaking
                     : Th.overlayPillBg
                border.width: 1
                border.color: warn ? Th.overlayPillBorderError
                            : lit ? Th.overlayPillBorderSpeaking
                            : Th.overlayPillBorder

                Row {
                    id: row
                    anchors.verticalCenter: parent.verticalCenter
                    x: Kirigami.Units.largeSpacing
                    spacing: Kirigami.Units.smallSpacing + 2

                    Rectangle { // avatar dot: initial letter, accent ring while speaking
                        width: Kirigami.Units.gridUnit * 1.3
                        height: width
                        radius: width / 2
                        anchors.verticalCenter: parent.verticalCenter
                        visible: !pill.modelData.more
                        color: pill.modelData.self ? Th.overlayPillAvatarBgOwn
                                                   : Th.overlayPillAvatarBg
                        border.width: pill.lit ? 2 : 0
                        border.color: Th.overlayPillAvatarRingSpeaking
                        Text {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            anchors.centerIn: parent
                            text: (pill.modelData.label || "?").charAt(0).toUpperCase()
                            color: Th.overlayPillAvatarFg
                            font.pixelSize: parent.height * 0.52
                            font.bold: true
                        }
                    }

                    Text {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        anchors.verticalCenter: parent.verticalCenter
                        text: pill.warn ? pill.modelData.label + " — muted!" : pill.modelData.label
                        color: pill.warn ? Th.overlayPillFgError
                             : pill.lit ? Th.overlayPillFgSpeaking : Th.overlayPillFg
                        font.bold: pill.lit || pill.warn
                        font.pixelSize: Kirigami.Units.gridUnit * 0.72
                        width: Math.min(implicitWidth, Kirigami.Units.gridUnit * 9)
                        elide: Text.ElideRight
                    }

                    Kirigami.Icon {
                        visible: pill.modelData.muted === true && !pill.modelData.more
                        anchors.verticalCenter: parent.verticalCenter
                        width: Kirigami.Units.gridUnit * 0.9; height: width
                        source: "microphone-sensitivity-muted-symbolic"
                        color: pill.warn ? Th.overlayPillIconError : Th.overlayPillIconMuted
                    }
                    Kirigami.Icon {
                        visible: pill.modelData.deafened === true
                        anchors.verticalCenter: parent.verticalCenter
                        width: Kirigami.Units.gridUnit * 0.9; height: width
                        source: "audio-volume-muted-symbolic"
                        color: Th.overlayPillIconMuted
                    }
                    Kirigami.Icon {
                        visible: pill.modelData.sharing === true
                        anchors.verticalCenter: parent.verticalCenter
                        width: Kirigami.Units.gridUnit * 0.9; height: width
                        source: "monitor-symbolic"
                        color: Th.overlayPillIconLive
                    }
                }
            }
        }
    }
}
