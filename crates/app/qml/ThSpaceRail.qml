// ThSpaceRail.qml — the Discord-style vertical SPACES rail (far left).
// Big round bubbles, one per Space: the meta-spaces (Home / Favourites / People /
// Rooms) pinned up top, then a divider, then the real Matrix spaces — which the user
// can DRAG (press-and-hold to grab) to reorder. The chosen order is persisted via
// navController.setSpaceOrder so it survives refreshes/restarts. The active space gets
// a Discord-style left pill + squircle morph; unread rolls up to a red badge.
//
// Data via app.mNavSpaces / app.mNavActiveSpace (the CXX-Qt mirror); selection acts
// through navController.selectSpace(key); reorder commits through setSpaceOrder.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the rail.* / space.* / dm.* tokens
import QtQml.Models

Item {
    id: rail
    required property var app
    property var navController
    readonly property var _a: app
    readonly property string activeKey: app ? app.mNavActiveSpace : "home"
    readonly property var _meta: (app ? app.mNavSpaces : []).filter(function (s) { return s.kind === "meta" })
    readonly property var _real: (app ? app.mNavSpaces : []).filter(function (s) { return s.kind === "space" })

    implicitWidth: 68

    /// The "+" at the foot of the rail: join something with a link, or make a space.
    signal addRequested()
    /// 0.8.27: right-click on a real space bubble → Main.qml's space menu (invite / link / leave).
    signal spaceMenuRequested(string key, string label)
    /// RBAC §7.3: an INVITED space's right-click offers only Join / Decline (join=false = decline).
    signal spaceInviteRequested(string key, bool join)
    /// 10.6 (REACH §1.3): a DM bubble was clicked — select People and open the DM (Main.qml).
    signal dmOpenRequested(string roomId)
    /// 10.6 (REACH §1.3): a DM invite's right-click answer (accept=false = decline).
    signal dmInviteRequested(string roomId, bool accept)

    // ── 10.6 (REACH §1): the DM strip — unread DMs + DM invites, newest first, never more than
    // five (two of them invites) plus a "+N" disc. Built in nav.rs (`dm_strip`, shared rules with
    // the web); here it is a ListModel synced by room id, so a bubble that stays keeps its
    // delegate and only arrivals / departures animate.
    readonly property int _dmOverflow: (app && app.mNavDmStrip) ? Number(app.mNavDmStrip.overflow || 0) : 0
    ListModel { id: dmModel }
    function _syncDm() {
        var es = (rail.app && rail.app.mNavDmStrip && rail.app.mNavDmStrip.entries) ? rail.app.mNavDmStrip.entries : []
        for (var i = dmModel.count - 1; i >= 0; i--) {
            var rid = dmModel.get(i).rid, keep = false
            for (var j = 0; j < es.length; j++) if (es[j] && es[j].id === rid) { keep = true; break }
            if (!keep) dmModel.remove(i)
        }
        for (var k = 0; k < es.length; k++) {
            var e = es[k]
            if (!e || !e.id) continue
            var raw = JSON.stringify(e), at = -1
            for (var m = k; m < dmModel.count; m++) if (dmModel.get(m).rid === e.id) { at = m; break }
            if (at === -1) {
                dmModel.insert(Math.min(k, dmModel.count), { rid: e.id, e: e, raw: raw })
            } else {
                if (at !== k) dmModel.move(at, k, 1)
                if (dmModel.get(k).raw !== raw) dmModel.set(k, { rid: e.id, e: e, raw: raw })
            }
        }
    }
    Connections {
        target: rail.app
        function onMNavDmStripChanged() { rail._syncDm() }
    }
    Component.onCompleted: rail._syncDm()
    // 10.6.0 review: a tooltip was auto-detected rich text — a status or a name with markup in it would render (and load
    // an <img>). Security review 2026-10-26: every tooltip is PlainText (AppCore._plainTooltips), so the text goes in as it
    // is — but only once that worked (LinkGate.tooltipsPlain); until then, or if it couldn't, it is escaped (review #7).
    function _tipSafe(s) {
        return LinkGate.tipText(s)
    }
    function _dmTip(e, pres) {
        var name = String((e && e.name) || "")
        if (e && e.invite === true) return name + " wants to chat — click to look"
        var n = Number((e && e.unread) || 0)
        return name + " — " + (n === 1 ? "1 new message" : n + " new messages") + (pres ? " · " + String(pres.label || "") : "")
    }

    // "Invited by <name>" — the display name, else the localpart of the inviter's id, else "someone".
    function inviterLabel(sp) {
        if (!sp) return "someone"
        if (sp.inviter && String(sp.inviter).length > 0) return String(sp.inviter)
        var m = /^@([^:]+):/.exec(String(sp.inviterMxid || ""))
        return m ? m[1] : "someone"
    }

    function _metaIcon(key) {
        switch (key) {
        case "home": return "go-home-symbolic"
        // no rating-symbolic in breeze; starred-symbolic is the symbolic star
        case "favourites": return "starred-symbolic"
        case "people": return "system-users-symbolic"
        case "orphans": return "view-list-details-symbolic"
        default: return "folder-symbolic"
        }
    }

    // The element key under a DelegateModel group item (array models expose the object
    // under .model.modelData; fall back to a direct .key role).
    function _keyOf(item) {
        var m = item ? item.model : null
        if (!m) return ""
        if (m.modelData && m.modelData.key !== undefined) return m.modelData.key
        if (m.key !== undefined) return m.key
        return ""
    }
    // Commit the real-space rail's current visual order to the backend.
    function _commitOrder() {
        var keys = []
        for (var i = 0; i < realModel.items.count; i++) {
            var k = _keyOf(realModel.items.get(i))
            if (k.length > 0) keys.push(k)
        }
        if (rail.navController && keys.length > 0)
            rail.navController.setSpaceOrder(JSON.stringify(keys))
    }

    // 11.0 (islands C6/C7): the rail draws nothing any more — the window's dock host places its three islands (nav · dms
    // · spaces, each its own frame) and hands them this rail (railRef) with its models and menus.
    readonly property alias dmModelObj: dmModel
    readonly property alias realModelObj: realModel
    readonly property alias dmMenuObj: dmMenu
    readonly property int dmCount: dmModel.count

    // 10.6 (REACH §1.3): a DM bubble's menu — Open / Mark as read, or Accept / Decline for an
    // invite. Hoisted at the rail so a strip update cannot close it under the pointer.
    Controls.Menu {
        id: dmMenu
        modal: true
        margins: 6
        property string rid: ""
        property string label: ""
        property bool invite: false
        MenuSectionLabel { app: rail._a; text: dmMenu.label; width: Math.min(implicitWidth, 220); elide: Text.ElideRight }
        Controls.MenuItem {
            text: "Open"
            icon.name: "go-next-symbolic"
            visible: !dmMenu.invite
            height: visible ? implicitHeight : 0
            onTriggered: rail.dmOpenRequested(dmMenu.rid)
        }
        Controls.MenuItem {
            text: "Mark as read"
            icon.name: "checkmark-symbolic"   // the room list's own "Mark as read" glyph
            visible: !dmMenu.invite
            height: visible ? implicitHeight : 0
            onTriggered: if (rail.navController) rail.navController.markRoomRead(dmMenu.rid)
        }
        Controls.MenuItem {
            text: "Accept"
            icon.name: "dialog-ok-apply-symbolic"
            visible: dmMenu.invite
            height: visible ? implicitHeight : 0
            onTriggered: rail.dmInviteRequested(dmMenu.rid, true)
        }
        Controls.MenuItem {
            text: "Decline"
            icon.name: "dialog-close-symbolic"
            visible: dmMenu.invite
            height: visible ? implicitHeight : 0
            onTriggered: rail.dmInviteRequested(dmMenu.rid, false)
        }
    }

    // RBAC §7.3: an invited space's only two answers. Hoisted at the rail (the bubble delegates
    // are rebuilt on every nav refresh); pops at the pointer.
    Controls.Menu {
        id: inviteMenu
        modal: true
        margins: 6
        property string key: ""
        property string label: ""
        MenuSectionLabel { app: rail._a; text: inviteMenu.label; width: Math.min(implicitWidth, 220); elide: Text.ElideRight }
        Controls.MenuItem {
            text: "Join space"
            icon.name: "list-add-user"
            onTriggered: rail.spaceInviteRequested(inviteMenu.key, true)
        }
        Controls.MenuItem {
            text: "Decline"
            icon.name: "dialog-close-symbolic"
            onTriggered: rail.spaceInviteRequested(inviteMenu.key, false)
        }
    }

    DelegateModel {
        id: realModel
        model: rail._real
        delegate: Component {
            Item {
                id: wrapper
                required property var modelData
                // 11.0: in a row (the Side monitor preset) a slot is one bubble wide
                readonly property bool inRow: !!ListView.view && ListView.view.orientation === ListView.Horizontal
                width: (ListView.view && !wrapper.inRow) ? ListView.view.width : 68
                height: 52

                MouseArea {
                    id: dragArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: drag.active ? Qt.ClosedHandCursor : Qt.PointingHandCursor

                    // Drag starts as soon as the pointer moves past the threshold (no
                    // press-and-hold — moving cancels a hold, which made dragging fail).
                    // A tap that doesn't move emits onClicked → select the space.
                    drag.target: (wrapper.modelData.membership === "invite" || wrapper.inRow) ? null : bubble   // an invite has no place in the order yet; a row: no reorder (11.0, as on the web)
                    drag.axis: Drag.YAxis
                    drag.threshold: 6
                    acceptedButtons: Qt.LeftButton | Qt.RightButton   // 0.8.27: right-click = space menu

                    // Mirror drag.active so we can react to start/end.
                    property bool dragging: drag.active
                    onDraggingChanged: {
                        if (dragging) {
                            rail.app.navReordering = true   // pause the spaces mirror
                        } else {
                            rail._commitOrder()             // persist the new order
                            rail.app.navReordering = false
                        }
                    }
                    onClicked: (mouse) => {
                        if (mouse.button === Qt.RightButton) {
                            if (wrapper.modelData.membership === "invite") {
                                inviteMenu.key = String(wrapper.modelData.key)
                                inviteMenu.label = String(wrapper.modelData.label || "")
                                inviteMenu.popup()
                            } else {
                                rail.spaceMenuRequested(wrapper.modelData.key, wrapper.modelData.label)
                            }
                        } else if (rail.navController) {
                            rail.navController.selectSpace(wrapper.modelData.key)
                        }
                    }

                    SpaceBubble {
                        id: bubble
                        railRef: rail
                        anchors.horizontalCenter: parent.horizontalCenter
                        anchors.verticalCenter: parent.verticalCenter
                        sp: wrapper.modelData
                        pillBelow: wrapper.inRow
                        selected: wrapper.modelData.key === rail.activeKey
                        hovered: dragArea.containsMouse && !dragArea.drag.active
                        dragging: dragArea.drag.active

                        Drag.active: dragArea.drag.active
                        Drag.source: wrapper
                        Drag.keys: ["tinhat/space"]   // 10.0.1: only the rail's DropArea takes this drag
                        Drag.hotSpot.x: width / 2
                        Drag.hotSpot.y: height / 2

                        // While dragging, lift out of the list and move freely; when the
                        // drag ends the State reverts and the bubble snaps into its slot.
                        states: State {
                            when: dragArea.drag.active
                            ParentChange { target: bubble; parent: rail }
                            AnchorChanges {
                                target: bubble
                                anchors.horizontalCenter: undefined
                                anchors.verticalCenter: undefined
                            }
                        }
                    }
                    Controls.ToolTip.text: wrapper.modelData.membership === "invite"
                        ? "Invited by " + rail.inviterLabel(wrapper.modelData) + " — click to look"   // RBAC §7.3
                        : wrapper.modelData.label + (wrapper.modelData.kind === "space" ? "  ·  drag to reorder" : "")   // 10.1 (TH-NAV-24)
                    Controls.ToolTip.visible: dragArea.containsMouse && !dragArea.drag.active
                    Controls.ToolTip.delay: 400
                }

                // Reorder live as the grabbed bubble passes over a neighbour.
                DropArea {
                    anchors.fill: parent
                    keys: ["tinhat/space"]
                    onEntered: (drag) => {
                        if (!drag.source) return
                        var from = drag.source.DelegateModel.itemsIndex
                        var to = wrapper.DelegateModel.itemsIndex
                        if (from !== to) realModel.items.move(from, to)
                    }
                }
            }
        }
    }
}
