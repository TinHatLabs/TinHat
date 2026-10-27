// ParticipantVolumeMenu.qml — ONE per-person volume menu for every surface (0.8.31).
//
// The left bar (ThRoomList: right-click a participant bubble) and the call grid (right-click
// or long-press a tile) both open THIS component, parameterized by mxid/name, so the two can
// never drift. THREE labeled dials — Mic / Stream / Soundboard — each wired to the matching
// gain-registry category ("mic" / "screenshare" / "music") on AudioPrefsController: live-push
// on drag, persist on release, per-category mute with a glyph, one Reset for all three.
// fix/sliders-volume (2026-09-28, Windows): a ThMenu (as wide as its rows — Basic's Menu was a fixed 200 px and clipped
// every dial row away), ThSlider dials (Basic drew the fill + the handle in the window colour), the label column as
// the web's (max(118, 8.75 em): the longer labels ran under the slider at larger fonts).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the volume menu tokens

ThMenu {
    id: menu
    property var audioPrefs: null      // AudioPrefsController
    property var app: null             // 9.4: the app root (corners: mInCall, mCorner, callctl)
    property string participant: ""    // 9.4: their full LiveKit identity when the caller knows it
    property string mxid: ""           // whose volumes — bare @user:server (the key the store uses)
    property string myMxid: ""         // never offer to message yourself
    property string name: ""
    signal messageUser(string mxid)
    signal changed()                   // a mute / reset was committed — callers refresh their glyphs

    // ---- RBAC §7.4 (docs/RBAC.md): moderation — server-mute / disconnect (the Tin Hat server acts
    // on LiveKit), kick / ban from the call room's SPACE (spaceCtl.kickIn/banIn), Manage roles…
    readonly property bool inLiveCall: !!menu.app && (menu.app.mInCall === true || menu.app.fakeCall === true)
    readonly property bool canModerate: menu.inLiveCall && menu.app.mCanModerateCall === true
        && menu.mxid.length > 0 && menu.mxid !== menu.myMxid
    // Who server-muted them ("" = nobody): by full identity when the caller knows it, else any device of the mxid.
    readonly property string srvMutedBy: (menu.app && menu.app.serverMutedByFor) ? String(menu.app.serverMutedByFor(menu.participant, menu.mxid)) : ""
    property string callSpaceId: ""    // the call room's space, looked up as the menu opens ("" = not in a space)
    readonly property var callPerms: (menu.app && menu.app.mCallPerms) ? menu.app.mCallPerms : ({})
    readonly property bool canKickSpace: menu.callSpaceId.length > 0 && menu.callPerms.kick === true && menu.mxid.length > 0 && menu.mxid !== menu.myMxid
    readonly property bool canBanSpace: menu.callSpaceId.length > 0 && menu.callPerms.ban === true && menu.mxid.length > 0 && menu.mxid !== menu.myMxid
    readonly property bool canManageRoles: menu.callSpaceId.length > 0 && menu.callPerms.manage_roles === true
    function toast(text, ms) { if (menu.app && menu.app.showPassiveNotification) menu.app.showPassiveNotification(LinkGate.plain(text), ms || 3000) }
    // The three server actions: a local refusal comes back as text (toast it); the network
    // result lands in CallController.status / lastError, which AppCore already toasts.
    function moderate(action) {
        if (!menu.app || !menu.app.callctl) return
        var t = menu.participant.length > 0 ? menu.participant
              : (menu.app.identityForMxid ? String(menu.app.identityForMxid(menu.mxid)) : "")
        if (t.length === 0) { menu.toast("They're not in the call"); return }
        var err = action === "mute" ? menu.app.callctl.serverMute(t)
                : action === "unmute" ? menu.app.callctl.serverUnmute(t)
                : menu.app.callctl.disconnectParticipant(t)
        if (String(err || "").length > 0) menu.toast(String(err), 4000)
    }

    modal: true
    // Non-negative margins make Qt keep the popup inside the window (the default -1
    // let it hang off the bottom/right edge with the sliders cut off).
    margins: 6

    Controls.MenuItem {
        text: "Message"
        icon.name: "mail-message-new-symbolic"
        visible: menu.mxid.length > 0 && menu.mxid !== menu.myMxid
        height: visible ? implicitHeight : 0
        onTriggered: menu.messageUser(menu.mxid)
    }
    // 9.4 corners ("pull aside"): a free person can be pulled into my corner (a new one if I am in
    // none); someone already aside can be joined; someone in MY corner shows Leave.
    readonly property string targetCorner: (menu.app && menu.mxid.length > 0)
        ? (menu.participant.length > 0 ? menu.app.cornerOf(menu.participant) : menu.app.cornerOfMxid(menu.mxid)) : ""
    readonly property string myCorner: menu.app ? menu.app.mCorner.mine : ""
    readonly property bool cornerable: !!menu.app && (menu.app.mInCall || menu.app.fakeCall === true)
        && menu.mxid.length > 0 && menu.mxid !== menu.myMxid
    function cornerAction(kind) {
        if (!menu.app || !menu.app.callctl) return
        var t = menu.participant.length > 0 ? menu.participant : menu.mxid
        if (menu.app.markLocalCorner) menu.app.markLocalCorner()   // 10.1: I started this — no "pulled you aside" toast
        var err = kind === "pull" ? menu.app.callctl.pullAside(t) : menu.app.callctl.joinCorner(t)
        if (err.length > 0 && menu.app.showPassiveNotification) menu.app.showPassiveNotification(LinkGate.plain(err), 3000)
    }
    Controls.MenuItem {
        text: menu.myCorner.length > 0 ? "Pull into your corner" : "Pull aside"
        icon.name: "audio-input-microphone-symbolic"
        visible: menu.cornerable && menu.targetCorner.length === 0
        height: visible ? implicitHeight : 0
        Controls.ToolTip.text: "Just you two (and whoever joins): everyone else drops to 10 % for you both, and nobody else hears your mics. Either of you leaves with the yellow button"
        Controls.ToolTip.visible: hovered
        Controls.ToolTip.delay: 600
        onTriggered: menu.cornerAction("pull")
    }
    Controls.MenuItem {
        text: "Join their corner"
        icon.name: "list-add-user"
        visible: menu.cornerable && menu.targetCorner.length > 0 && menu.targetCorner !== menu.myCorner
        height: visible ? implicitHeight : 0
        onTriggered: menu.cornerAction("join")
    }
    Controls.MenuItem {
        text: "Leave the corner"
        icon.name: "go-previous-symbolic"
        visible: menu.cornerable && menu.targetCorner.length > 0 && menu.targetCorner === menu.myCorner
        height: visible ? implicitHeight : 0
        onTriggered: if (menu.app && menu.app.callctl) menu.app.callctl.leaveCorner()
    }
    // 11.3 same room: someone sitting next to you — link up (one voice from the room, you don't play each other).
    readonly property bool srLinked: !!menu.app && menu.participant.length > 0 && !!menu.app.mSameRoom && !!menu.app.mSameRoom.linked
        && menu.app.mSameRoom.linked.indexOf(menu.participant) >= 0
    Controls.MenuItem {
        text: "We're in the same room"
        icon.name: "audio-input-microphone-symbolic"
        visible: menu.cornerable && !menu.srLinked
        height: visible ? implicitHeight : 0
        Controls.ToolTip.text: "Two devices in one room: Tin Hat sends one voice from your room (whoever's own mic hears them loudest) and stops your devices playing each other, so nobody hears either of you twice"
        Controls.ToolTip.visible: hovered
        Controls.ToolTip.delay: 600
        onTriggered: {
            if (!menu.app || !menu.app.callctl) return
            var t = menu.participant.length > 0 ? menu.participant : menu.mxid
            var err = menu.app.callctl.sameRoomLink(t)
            if (err.length > 0 && menu.app.showPassiveNotification) menu.app.showPassiveNotification(LinkGate.plain(err), 3000)
        }
    }
    Controls.MenuItem {
        text: "Unlink — same room"
        icon.name: "go-previous-symbolic"
        visible: menu.cornerable && menu.srLinked
        height: visible ? implicitHeight : 0
        onTriggered: if (menu.app && menu.app.callctl) menu.app.callctl.sameRoomUnlink()
    }
    // ---- Moderation (RBAC §7.4) — the heading shows only when at least one row below does.
    MenuSectionLabel {
        app: menu.app
        text: "Moderation"
        visible: srvMuteItem.visible || disconnectItem.visible || kickSpaceItem.visible || banSpaceItem.visible || manageRolesItem.visible
        height: visible ? implicitHeight : 0
    }
    Controls.MenuItem {
        id: srvMuteItem
        text: menu.srvMutedBy.length > 0 ? "Unmute their mic for everyone" : "Mute their mic for everyone"
        icon.name: menu.srvMutedBy.length > 0 ? "audio-input-microphone-symbolic" : "microphone-sensitivity-muted-symbolic"
        visible: menu.canModerate
        height: visible ? implicitHeight : 0
        Controls.ToolTip.text: menu.srvMutedBy.length > 0
            ? "Muted by " + menu.srvMutedBy + " — lets them talk again"
            : "Nobody in the call hears their mic until a moderator unmutes them"
        Controls.ToolTip.visible: hovered
        Controls.ToolTip.delay: 600
        onTriggered: menu.moderate(menu.srvMutedBy.length > 0 ? "unmute" : "mute")
    }
    Controls.MenuItem {
        id: disconnectItem
        text: "Disconnect from the call…"
        icon.name: "call-stop-symbolic"
        visible: menu.canModerate
        height: visible ? implicitHeight : 0
        onTriggered: disconnectConfirm.open()
    }
    Controls.MenuSeparator {
        visible: (srvMuteItem.visible || disconnectItem.visible) && (kickSpaceItem.visible || banSpaceItem.visible || manageRolesItem.visible)
        height: visible ? implicitHeight : 0
    }
    Controls.MenuItem {
        id: kickSpaceItem
        text: "Kick from space…"
        icon.name: "im-kick-user-symbolic"
        visible: menu.canKickSpace
        height: visible ? implicitHeight : 0
        onTriggered: kickSpaceConfirm.open()
    }
    Controls.MenuItem {
        id: banSpaceItem
        text: "Ban from space…"
        icon.name: "im-ban-user-symbolic"
        visible: menu.canBanSpace
        height: visible ? implicitHeight : 0
        onTriggered: banSpaceConfirm.open()
    }
    Controls.MenuItem {
        id: manageRolesItem
        text: "Manage roles…"
        icon.name: "configure-symbolic"
        visible: menu.canManageRoles
        height: visible ? implicitHeight : 0
        onTriggered: if (menu.app && menu.app.openSpaceSettings) menu.app.openSpaceSettings(menu.callSpaceId, "members")
    }
    // The confirms live on the APP window's overlay, not this menu's: a Menu's content has no
    // window until it first opens, and a dialog parented there would have nowhere to show.
    ConfirmDialog {
        id: disconnectConfirm
        app: menu.app
        parent: (menu.app && menu.app.overlay) ? menu.app.overlay : Controls.Overlay.overlay
        dialogTitle: "Disconnect from the call?"
        confirmText: "Disconnect"
        destructive: true
        message: "Drop " + menu.name + " from this call? They can rejoin right away — use Kick or Ban to keep them out."
        onConfirmed: menu.moderate("disconnect")
    }
    ConfirmDialog {
        id: kickSpaceConfirm
        app: menu.app
        parent: (menu.app && menu.app.overlay) ? menu.app.overlay : Controls.Overlay.overlay
        dialogTitle: "Kick from the space?"
        confirmText: "Kick"
        destructive: true
        askReason: true
        message: "Remove " + menu.name + " from the space and its rooms? They can come back with a new invite."
        onConfirmed: if (menu.app && menu.app.spaceCtl) menu.app.spaceCtl.kickIn(menu.callSpaceId, menu.mxid, kickSpaceConfirm.reason)
    }
    ConfirmDialog {
        id: banSpaceConfirm
        app: menu.app
        parent: (menu.app && menu.app.overlay) ? menu.app.overlay : Controls.Overlay.overlay
        dialogTitle: "Ban from the space?"
        confirmText: "Ban"
        destructive: true
        askReason: true
        message: "Ban " + menu.name + " from the space and its rooms? They can't rejoin until unbanned."
        onConfirmed: if (menu.app && menu.app.spaceCtl) menu.app.spaceCtl.banIn(menu.callSpaceId, menu.mxid, banSpaceConfirm.reason)
    }
    Controls.MenuSeparator {}
    // The three dials, in display order. `cat` is the exact gain-registry category string.
    readonly property var dials: [
        { label: "Their mic",          cat: "mic" },
        { label: "Their screen share", cat: "screenshare" },   // 10.1 (TH-POP-14): the object, not the channel
        { label: "Their soundboard",   cat: "music" }
    ]
    // Live values (seeded on open) so the Reset enable-state and the sliders stay coherent.
    property real micVal: 1.0
    property real streamVal: 1.0
    property real soundVal: 1.0
    property bool micMuted: false
    property bool streamMuted: false
    property bool soundMuted: false
    function _valFor(cat) {
        return cat === "mic" ? micVal : cat === "screenshare" ? streamVal : soundVal
    }
    function _setVal(cat, v) {
        if (cat === "mic") micVal = v
        else if (cat === "screenshare") streamVal = v
        else soundVal = v
    }
    function _mutedFor(cat) {
        return cat === "mic" ? micMuted : cat === "screenshare" ? streamMuted : soundMuted
    }
    function _setMuted(cat, m) {
        if (cat === "mic") micMuted = m
        else if (cat === "screenshare") streamMuted = m
        else soundMuted = m
    }
    // Seed all six from the saved prefs each time the menu opens.
    onAboutToShow: {
        // RBAC: the call room's space decides whether "Kick / Ban from space" apply at all.
        menu.callSpaceId = (menu.app && menu.app.nav && menu.app.mCallRoomId && String(menu.app.mCallRoomId).length > 0)
            ? String(menu.app.nav.spaceOf(menu.app.mCallRoomId)) : ""
        if (menu.audioPrefs && menu.mxid.length > 0) {
            micVal = parseFloat(menu.audioPrefs.volumeFor(menu.mxid, "mic"))
            streamVal = parseFloat(menu.audioPrefs.volumeFor(menu.mxid, "screenshare"))
            soundVal = parseFloat(menu.audioPrefs.volumeFor(menu.mxid, "music"))
            micMuted = menu.audioPrefs.mutedFor(menu.mxid, "mic")
            streamMuted = menu.audioPrefs.mutedFor(menu.mxid, "screenshare")
            soundMuted = menu.audioPrefs.mutedFor(menu.mxid, "music")
        }
    }

    Controls.Label {
        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
        leftPadding: Kirigami.Units.smallSpacing
        rightPadding: Kirigami.Units.smallSpacing
        topPadding: Kirigami.Units.smallSpacing
        text: "Volume — " + menu.name + "  (only for you)"
        color: Th.callVolumeTitleFg
        font.bold: true
        elide: Text.ElideRight
        width: Math.min(implicitWidth, 260)
    }

    // Three compact slider rows (Mic / Stream / Soundboard).
    Repeater {
        model: menu.dials
        delegate: RowLayout {
            id: dialRow
            required property var modelData
            readonly property string cat: modelData.cat
            spacing: Kirigami.Units.smallSpacing
            // A RowLayout child of Controls.Menu isn't laid out by a QtQuick Layout, so
            // Layout.* margins are ignored — inset with explicit spacer Items.
            Item { Layout.preferredWidth: Kirigami.Units.smallSpacing }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                // the web's column (menus.module.css .volLabel: max(118px, 8.75em)) — at a 16 px small font "Their
                // screen share" is wider than 118 and used to run under the slider
                Layout.preferredWidth: Math.max(118, Math.ceil(8.75 * fontInfo.pixelSize))
                text: modelData.label
                color: menu._mutedFor(dialRow.cat) ? Th.callVolumeLabelFgMuted : Th.callVolumeLabelFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize   // 10.1 (TH-POP-20)
            }
            ThSlider {
                id: dialSlider
                callHandle: true   // the web's call menus: an accent handle with a bright ring (menus.module.css .volSlider)
                Layout.preferredWidth: 150
                from: 0.0; to: 2.0
                // the web's dial (menus.tsx VolumeRow stepSize 0.01): whole percents — an arrow key / a wheel notch = 1 %;
                // the wheel on every style (Plasma's style and the web step on it; Basic, on Windows, ignored it)
                stepSize: 0.01
                snapMode: Controls.Slider.SnapAlways
                wheelEnabled: true
                // Plasma's style draws a tick per step under a stepped slider (200 here, a dotted row the dial never
                // had, nor has the web's): no ticks. Basic ignores it.
                Kirigami.StyleHints.tickMarkStepSize: -1
                enabled: !menu._mutedFor(dialRow.cat)
                value: menu._valFor(dialRow.cat)
                // During a drag: push the gain LIVE only (no disk write), so we don't
                // rewrite+chown the JSON on every pixel. A wheel notch moves without a
                // press (ThSlider): commit it at once — it has no release.
                onMoved: {
                    menu._setVal(dialRow.cat, value)
                    if (menu.audioPrefs && menu.mxid.length > 0) {
                        menu.audioPrefs.pushVolume(menu.mxid, dialRow.cat, value)
                        if (!pressed) menu.audioPrefs.setVolume(menu.mxid, dialRow.cat, value)
                    }
                }
                // On release: commit the final value to disk once.
                onPressedChanged: {
                    if (!pressed && menu.audioPrefs && menu.mxid.length > 0)
                        menu.audioPrefs.setVolume(menu.mxid, dialRow.cat, value)
                }
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                Layout.preferredWidth: 38
                text: Math.round(menu._valFor(dialRow.cat) * 100) + "%"
                color: Th.callVolumeValueFg
                horizontalAlignment: Text.AlignRight
                font.pointSize: Kirigami.Theme.smallFont.pointSize   // 10.1 (TH-POP-20)
            }
            // Per-category mute toggle.
            Controls.ToolButton {
                icon.name: menu._mutedFor(dialRow.cat) ? "audio-volume-muted-symbolic" : "audio-volume-high-symbolic"
                Controls.ToolTip.text: menu._mutedFor(dialRow.cat) ? ("Unmute " + modelData.label) : ("Mute " + modelData.label)
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.delay: 400
                onClicked: {
                    if (menu.audioPrefs && menu.mxid.length > 0) {
                        var nm = !menu._mutedFor(dialRow.cat)
                        menu.audioPrefs.setMuted(menu.mxid, dialRow.cat, nm)
                        menu._setMuted(dialRow.cat, nm)
                        menu.changed()
                    }
                }
            }
            Item { Layout.preferredWidth: Kirigami.Units.smallSpacing }
        }
    }

    Controls.MenuSeparator {}
    Controls.MenuItem {
        text: "Reset all to 100%"
        // Enabled when ANY dial is off-unity or muted.
        enabled: Math.abs(menu.micVal - 1.0) > 0.001
            || Math.abs(menu.streamVal - 1.0) > 0.001
            || Math.abs(menu.soundVal - 1.0) > 0.001
            || menu.micMuted || menu.streamMuted || menu.soundMuted
        onTriggered: {
            if (menu.audioPrefs && menu.mxid.length > 0) {
                var cats = ["mic", "screenshare", "music"]
                for (var i = 0; i < cats.length; i++) {
                    menu.audioPrefs.setVolume(menu.mxid, cats[i], 1.0)
                    menu.audioPrefs.setMuted(menu.mxid, cats[i], false)
                    menu._setVal(cats[i], 1.0)
                    menu._setMuted(cats[i], false)
                }
                menu.changed()
            }
        }
    }
}
