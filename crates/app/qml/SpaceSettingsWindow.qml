// SpaceSettingsWindow.qml — a space's settings (RBAC, docs/RBAC.md §7.2): Overview / Members /
// Roles / Rooms behind a Settings-style rail. Discord's "Server settings", in Tin Hat's clothes.
//
// Props in, signals out — exactly like RightPanel / CalendarWindow: AppCore mirrors
// SpaceController into `mSpace*` (parsed once per `rev`), Main.qml hands them in here and wires
// every signal below to `spaceCtl.*`. Nothing in this file reads a controller.
//
// Roles are the UI; ranks (0–100) decide who may act on whom. Users never see a power level.
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Dialogs
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the spaceSettings.* / roles.* / invite.* tokens

Window {
    id: win
    required property var app

    // ---- in (SpaceController mirror) -------------------------------------------------------
    property string spaceId: ""
    property bool busy: false
    property var summary: ({})          // {id,name,topic,avatar,alias,link,joinRule,memberCount,invitedCount,bannedCount,roomCount,encrypted,membership,isMember,canManageSpace,canManageRoles,canManageRooms,canInvite,canKick,canBan,outOfSyncRooms}
    property var members: []            // [{mxid,name,avatar,membership,rank,roles:[id],topRole,isMe,canKick,canBan,canEditRoles,reason}]
    property var roles: []              // [{id,name,color,rank,permissions:[key],memberCount,editable,deletable,isEveryone}]
    property var perms: ({})            // {"<key>": bool}
    property var rooms: []              // [{id,name,avatar,joined,synced,canSync}]
    property var userSearch: []         // [{mxid,name,avatar,alreadyMember}]
    property bool rolesActive: false
    property int myRank: 0
    property var permissionCatalog: []  // [{key,label,group,description,enforcement}]
    property string myMxid: ""
    // Last success / failure text with a revision so the same sentence can show twice.
    property string statusText: ""
    property int statusRev: 0
    property string errorText: ""
    property int errorRev: 0

    // ---- out (→ spaceCtl.* in Main.qml) ----------------------------------------------------
    signal openRequested(string spaceId)
    signal closeRequested()
    signal refreshRequested()
    signal applyName(string name)
    signal applyTopic(string topic)
    signal applyPhoto(string fileUrl)        // "" removes
    signal applyJoinRule(string rule)        // "public" | "invite"
    signal invite(string mxid)
    signal searchUsers(string query)
    signal kick(string mxid, string reason)
    signal ban(string mxid, string reason)
    signal unban(string mxid)
    signal setMemberRoles(string mxid, string rolesJson)
    signal saveRole(string roleJson)
    signal deleteRole(string roleId)
    signal setupRoles()
    signal syncRooms()
    signal syncRoom(string roomId)           // v1: the host maps this onto syncRooms()
    signal leave()
    signal viewProfile(string mxid)
    signal messageUser(string mxid)
    signal copyLink(string link)

    // ---- theme -----------------------------------------------------------------------------
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app ? app : fallbackTheme   // sizes and motion; colours are Th's spaceSettings.* / roles.*
    readonly property int dur: Motion.dur   // 11.0 (GPU.md R3): was `theme.dur ? theme.dur : 120` — Reduce motion's 0 became 120
    function esc(s) { return String(s || "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;") }
    function localpart(s) { return s ? String(s).replace(/^@/, "").split(":")[0] : "" }
    function senderColor(m) { return app.senderColor(m) }   // `app` is required (the names algorithm is content)
    function toast(text) { if (app && app.showPassiveNotification) app.showPassiveNotification(LinkGate.plain(text), 3000) }

    // ---- window ----------------------------------------------------------------------------
    readonly property string spaceName: (summary && summary.name) ? String(summary.name) : ""
    // NEVER exactly "Tin Hat": the boot smoke asserts the main window by that exact title.
    title: (spaceName.length > 0 ? spaceName + " · " : "") + "Space settings"
    width: 1040
    height: 700
    minimumWidth: 880
    minimumHeight: 600
    color: Th.spaceSettingsBg
    visible: false
    WindowMemory { target: win; key: "spacesettings"; store: (win.app && win.app.prefStore) ? win.app.prefStore : null }

    readonly property var tabs: [
        { k: "overview", t: "Overview", i: "configure-symbolic",         d: "Name, photo, topic and who can join" },
        { k: "members",  t: "Members",  i: "system-users-symbolic",      d: "Everyone in the space — roles, invites and bans" },
        { k: "roles",    t: "Roles",    i: "security-medium-symbolic",   d: "Named groups with permissions, applied to every room" },
        { k: "rooms",    t: "Rooms",    i: "view-list-details-symbolic", d: "The rooms in this space and whether their permissions match" }
    ]
    property int tab: 0
    function tabIndexOf(name) {
        for (var i = 0; i < tabs.length; i++) if (tabs[i].k === name) return i
        return -1
    }
    function openOrRaise(tabName) {
        var idx = tabIndexOf(tabName || "")
        if (idx >= 0) win.tab = idx
        if (!win.visible) win.visible = true
        win.raise(); win.requestActivate()
        rail.forceActiveFocus()
        win._syncOverview()
        if (win.rolesActive && win.editRoleId.length === 0 && !win.editIsNew) win._selectFirstRole()
    }
    onVisibleChanged: if (!visible) win.closeRequested()
    onSpaceIdChanged: {
        // A different space: no draft survives (name/topic/role edits belong to the old one).
        win.editIsNew = false; win.editRoleId = ""; win.roleDirtyGuard = false
        win.memberQuery = ""; memberSearch.text = ""; win.memberFilter = "all"
        win._seedName = ""; win._seedTopic = ""
        win._syncOverview()
    }
    Shortcut {
        sequence: "Escape"
        onActivated: {
            if (invitePopover.opened) invitePopover.close()
            else if (memberMenu.opened) memberMenu.close()
            else if (rolesMenu.opened) rolesMenu.close()
            else win.close()
        }
    }

    // ---- inline status strip (mirrors the toast, in THIS window) ---------------------------
    property string stripText: ""
    property bool stripError: false
    Timer { id: stripTimer; interval: 4000; onTriggered: win.stripText = "" }
    onStatusRevChanged: if (statusText.length > 0) { stripText = statusText; stripError = false; stripTimer.restart() }
    onErrorRevChanged: if (errorText.length > 0) { stripText = errorText; stripError = true; stripTimer.restart() }

    // ---- helpers ---------------------------------------------------------------------------
    function roleById(id) {
        for (var i = 0; i < roles.length; i++) if (roles[i].id === id) return roles[i]
        return null
    }
    function roleName(id) { var r = roleById(id); return r ? String(r.name || id) : String(id) }
    function roleColor(id) { var r = roleById(id); return r ? String(r.color || "") : "" }
    function plural(name) {
        if (name === "Admin") return "Admins"
        if (name === "Moderator") return "Moderators"
        return name
    }
    readonly property int joinedCount: {
        var n = 0
        for (var i = 0; i < members.length; i++) if (!members[i].membership || members[i].membership === "join") n++
        return n
    }
    readonly property int invitedCount: {
        var n = 0
        for (var i = 0; i < members.length; i++) if (members[i].membership === "invite") n++
        return n
    }
    readonly property int bannedCount: {
        var n = 0
        for (var i = 0; i < members.length; i++) if (members[i].membership === "ban") n++
        return n
    }
    readonly property int outOfSync: Number((summary && summary.outOfSyncRooms) ? summary.outOfSyncRooms : 0)
    readonly property bool canManageSpace: !!(summary && summary.canManageSpace === true)
    readonly property bool canManageRoles: !!(summary && summary.canManageRoles === true)
    readonly property bool canInvite: !!(summary && summary.canInvite === true)

    // ======================================================================================
    // Overview draft state
    property string _seedName: ""
    property string _seedTopic: ""
    // 11.0.1: `summary` can be undefined for a moment at start (seen on Windows: a TypeError in _syncOverview once per
    // start, since 10.2.0); the reads below tolerate it, like every other read of it in this file
    readonly property bool overviewDirty: nameField.text !== ((summary && summary.name) || "") || topicField.text !== ((summary && summary.topic) || "")
    function _syncOverview() {
        var n = String((summary && summary.name) || ""), t = String((summary && summary.topic) || "")
        var userDirty = nameField.text !== win._seedName || topicField.text !== win._seedTopic
        if (!userDirty || (nameField.text === n && topicField.text === t)) {
            nameField.text = n; topicField.text = t; win._seedName = n; win._seedTopic = t
        }
        win._syncJoinRadios()
    }
    readonly property string joinRule: String((summary && summary.joinRule) ? summary.joinRule : "invite")
    function _syncJoinRadios() {
        ruleInvite.checked = win.joinRule !== "public"
        rulePublic.checked = win.joinRule === "public"
    }
    onSummaryChanged: _syncOverview()
    function saveOverview() {
        if (nameField.text !== ((summary && summary.name) || "")) win.applyName(nameField.text.trim())
        if (topicField.text !== ((summary && summary.topic) || "")) win.applyTopic(topicField.text)
        win._seedName = nameField.text; win._seedTopic = topicField.text
    }
    function discardOverview() {
        nameField.text = String((summary && summary.name) || ""); topicField.text = String((summary && summary.topic) || "")
        win._seedName = nameField.text; win._seedTopic = topicField.text
    }

    // ======================================================================================
    // Members state
    property string memberQuery: ""
    property string memberFilter: "all"      // all | invited | banned
    readonly property var visibleMembers: {
        var q = memberQuery.trim().toLowerCase()
        var out = []
        for (var i = 0; i < members.length; i++) {
            var m = members[i]
            var ms = String(m.membership || "join")
            if (memberFilter === "all" && ms !== "join") continue
            if (memberFilter === "invited" && ms !== "invite") continue
            if (memberFilter === "banned" && ms !== "ban") continue
            if (q.length > 0 && String(m.name || "").toLowerCase().indexOf(q) < 0 && String(m.mxid || "").toLowerCase().indexOf(q) < 0) continue
            var row = {}
            for (var k in m) row[k] = m[k]
            row.section = ms === "invite" ? "Invited" : (ms === "ban" ? "Banned" : (m.topRole && m.topRole.name ? plural(String(m.topRole.name)) : "Members"))
            row.sectionColor = ms === "invite" ? Th.rolesBadgeInvitedFg : (ms === "ban" ? Th.rolesBadgeBannedFg : (m.topRole && m.topRole.color ? String(m.topRole.color) : Th.rolesBadgeDefaultFg))
            out.push(row)
        }
        return out
    }
    function sectionCount(name) {
        var n = 0
        for (var i = 0; i < visibleMembers.length; i++) if (visibleMembers[i].section === name) n++
        return n
    }
    function sectionColor(name) {
        for (var i = 0; i < visibleMembers.length; i++) if (visibleMembers[i].section === name) return visibleMembers[i].sectionColor
        return Th.rolesBadgeDefaultFg
    }
    // The member a menu / dialog acts on (captured when the menu opens, like RightPanel.actorMxid).
    property var actor: ({})
    readonly property string actorName: (actor && actor.name && String(actor.name).length > 0) ? String(actor.name) : localpart(actor ? actor.mxid : "")
    readonly property string actorMembership: (actor && actor.membership) ? String(actor.membership) : "join"
    function openMemberMenu(row, item) {
        win.actor = row
        rolesMenu.actorRoles = (row && row.roles) ? row.roles.slice() : []
        // Popped at the window content, not the delegate: the list is rebuilt on every filter
        // keystroke / refresh and a Menu whose parent dies closes with it.
        var pt = item.mapToItem(win.contentItem, Kirigami.Units.gridUnit * 2.5, item.height)
        memberMenu.popup(win.contentItem, pt.x, pt.y)
    }
    function inviteFromQuery() {
        var q = inviteQuery.text.trim()
        if (/^@[^:\s]+:[^\s]+$/.test(q)) { win.invite(q); inviteQuery.text = ""; return }
        for (var i = 0; i < userSearch.length; i++) if (userSearch[i].alreadyMember !== true) { win.invite(String(userSearch[i].mxid)); inviteQuery.text = ""; return }
    }

    // ======================================================================================
    // Roles editor state
    property string editRoleId: ""
    property bool editIsNew: false
    property bool roleDirtyGuard: false      // a Discard-changes prompt is up
    property string pendingRoleId: ""
    property string pendingSelectName: ""
    property string dName: ""
    property string dColor: ""
    property int dRank: 0
    property var dPerms: []
    property bool dHoist: true       // 10.4: "show as its own group in the member list"
    readonly property var editRole: editIsNew ? null : roleById(editRoleId)
    readonly property bool editIsEveryone: !!(editRole && editRole.isEveryone === true)
    // 10.4: the template roles never colour a name (tags are for the fun roles people make up)
    readonly property bool editIsBuiltin: !editIsNew && !!editRole && (editRole.id === "admin" || editRole.id === "moderator" || editRole.id === "everyone")
    readonly property bool editEditable: editIsNew ? canManageRoles : !!(editRole && editRole.editable === true)
    readonly property bool editIsAdminRank: !editIsNew && !!editRole && Number(editRole.rank) >= 100
    // role colours are WIRE data (a role's colour on the server): the theme's accent role as text, then the fixed swatches
    readonly property var swatches: [String(Th.accentBase), "#e8b923", "#4da3ff", "#ff5d73", "#4ad295", "#c084fc", "#f97316", "#8a8f98"]   // theme-lint: ok content: role colour swatches
    function sameSet(a, b) {
        a = (a || []).slice().sort(); b = (b || []).slice().sort()
        if (a.length !== b.length) return false
        for (var i = 0; i < a.length; i++) if (a[i] !== b[i]) return false
        return true
    }
    readonly property bool roleDirty: editIsNew
        ? (dName.trim().length > 0 || dPerms.length > 0)
        : (!!editRole && (dName !== String(editRole.name || "") || dColor.toLowerCase() !== String(editRole.color || "").toLowerCase()
                          || dRank !== Number(editRole.rank) || !sameSet(dPerms, editRole.permissions)
                          || dHoist !== (editRole.hoist !== false)))
    readonly property bool roleValid: dName.trim().length > 0 && /^#[0-9a-fA-F]{6}$/.test(dColor) && dRank >= 0 && dRank <= 100
    readonly property var permGroups: {
        var groups = [], byName = {}
        for (var i = 0; i < permissionCatalog.length; i++) {
            var p = permissionCatalog[i]
            var g = String(p.group || "Other")
            if (!byName[g]) { byName[g] = { group: g, items: [] }; groups.push(byName[g]) }
            byName[g].items.push(p)
        }
        return groups
    }
    function hasPerm(key) { return dPerms.indexOf(key) >= 0 }
    function togglePerm(key, on) {
        var next = dPerms.filter(function (k) { return k !== key })
        if (on) next.push(key)
        dPerms = next
    }
    function _loadRole(r) {
        dName = String(r.name || ""); dColor = String(r.color || Th.accentBase); dRank = Number(r.rank || 0)
        dPerms = (r.permissions || []).slice()
        dHoist = r.hoist !== undefined ? (r.hoist !== false) : true
        roleNameField.text = dName; roleHexField.text = dColor; roleRankField.text = String(dRank)
    }
    function _selectFirstRole() { if (roles.length > 0) { editIsNew = false; editRoleId = String(roles[0].id); _loadRole(roles[0]) } }
    function selectRole(id) {
        if (id === editRoleId && !editIsNew) return
        if (roleDirty) { pendingRoleId = id; roleDirtyGuard = true; discardRoleConfirm.open(); return }
        var r = roleById(id); if (!r) return
        editIsNew = false; editRoleId = id; _loadRole(r)
    }
    function newRole() {
        if (roleDirty) { pendingRoleId = "__new__"; roleDirtyGuard = true; discardRoleConfirm.open(); return }
        editIsNew = true; editRoleId = ""
        _loadRole({ name: "", color: String(Th.accentBright), rank: Math.max(1, Math.min(99, myRank - 1)), permissions: [], hoist: true })
    }
    function saveRoleNow() {
        if (!roleValid) return
        var payload = { id: editIsNew ? "" : editRoleId, name: dName.trim(), color: dColor.toLowerCase(), rank: dRank, permissions: dPerms.slice(), hoist: dHoist }
        win.saveRole(JSON.stringify(payload))
        if (editIsNew) pendingSelectName = payload.name
    }
    function discardRoleNow() {
        if (editIsNew) { editIsNew = false; editRoleId = ""; _selectFirstRole() }
        else if (editRole) _loadRole(editRole)
    }
    onRolesChanged: {
        if (!rolesActive) return
        if (pendingSelectName.length > 0) {
            for (var i = 0; i < roles.length; i++) if (roles[i].name === pendingSelectName) {
                pendingSelectName = ""; editIsNew = false; editRoleId = String(roles[i].id); _loadRole(roles[i]); return
            }
        }
        if (editIsNew) return
        var r = roleById(editRoleId)
        if (!r) { _selectFirstRole(); return }
        if (!roleDirty) _loadRole(r)     // a remote edit shows up; a local draft is kept
    }

    // ======================================================================================
    // Dialogs (hoisted at the window)
    FileDialog {
        id: photoDialog
        title: "Choose a space photo"
        nameFilters: ["Images (*.png *.jpg *.jpeg *.gif *.webp)"]
        onAccepted: win.applyPhoto(String(selectedFile))
    }
    ConfirmDialog {
        id: leaveConfirm
        app: win.app
        dialogTitle: "Leave space?"
        confirmText: "Leave"
        destructive: true
        message: "Leave \"" + win.spaceName + "\"? Its rooms stay joined; you'll need a new invite to get the space (and its calendar) back."
        onConfirmed: { win.leave(); win.close() }
    }
    ConfirmDialog {
        id: kickConfirm
        app: win.app
        dialogTitle: win.actorMembership === "invite" ? "Withdraw invite?" : "Remove from the space?"
        confirmText: win.actorMembership === "invite" ? "Withdraw" : "Remove"
        destructive: true
        askReason: win.actorMembership !== "invite"
        message: win.actorMembership === "invite"
            ? "Withdraw the invite for " + win.actorName + "? They can be invited again later."
            : "Remove " + win.actorName + " from " + (win.spaceName.length > 0 ? win.spaceName : "this space") + " and its rooms? They can come back with a new invite."
        onConfirmed: win.kick(String(win.actor.mxid || ""), kickConfirm.reason)
    }
    ConfirmDialog {
        id: banConfirm
        app: win.app
        dialogTitle: "Ban from the space?"
        confirmText: "Ban"
        destructive: true
        askReason: true
        message: "Ban " + win.actorName + " from " + (win.spaceName.length > 0 ? win.spaceName : "this space") + " and its rooms? They can't rejoin until unbanned."
        onConfirmed: win.ban(String(win.actor.mxid || ""), banConfirm.reason)
    }
    ConfirmDialog {
        id: deleteRoleConfirm
        app: win.app
        dialogTitle: "Delete role?"
        confirmText: "Delete"
        destructive: true
        message: "Delete \"" + win.dName + "\"? People who have it lose it; nothing else changes. Every room's permissions are re-synced."
        onConfirmed: { if (!win.editIsNew && win.editRoleId.length > 0) win.deleteRole(win.editRoleId) }
    }
    ConfirmDialog {
        id: discardRoleConfirm
        app: win.app
        dialogTitle: "Discard changes?"
        confirmText: "Discard"
        cancelText: "Keep editing"
        destructive: true
        message: "You have unsaved changes to " + (win.editIsNew ? "the new role" : "\"" + win.dName + "\"") + "."
        onConfirmed: {
            win.roleDirtyGuard = false
            var id = win.pendingRoleId; win.pendingRoleId = ""
            if (id === "__new__") {
                win.editIsNew = true; win.editRoleId = ""
                win._loadRole({ name: "", color: String(Th.accentBright), rank: Math.max(1, Math.min(99, win.myRank - 1)), permissions: [] })
            } else {
                var r = win.roleById(id); if (!r) return
                win.editIsNew = false; win.editRoleId = id; win._loadRole(r)
            }
        }
        onCancelled: { win.roleDirtyGuard = false; win.pendingRoleId = "" }
    }

    // ---- per-member menu (hoisted; acts on win.actor) ----
    Controls.Menu {
        id: memberMenu
        modal: true
        margins: 6
        Controls.MenuItem {
            text: "View profile"
            icon.name: "user-identity"
            onTriggered: win.viewProfile(String(win.actor.mxid || ""))
        }
        Controls.MenuItem {
            text: "Message"
            icon.name: "mail-message-new-symbolic"
            visible: !!win.actor && win.actor.isMe !== true && win.actorMembership === "join"
            height: visible ? implicitHeight : 0
            onTriggered: win.messageUser(String(win.actor.mxid || ""))
        }
        // account safety (safety.rs): Ignore… / Stop ignoring · Report… (the web's PersonMenuItems)
        Controls.MenuItem {
            visible: !!win.actor && win.actor.isMe !== true && !!win.app.safety
            height: visible ? implicitHeight : 0
            text: LinkGate.plain(visible ? (win.app.safety.isIgnored(String(win.actor.mxid || "")) ? String(win.app.safety.w.unignore) : String(win.app.safety.w.ignore)) : "")
            icon.name: "dialog-cancel-symbolic"
            onTriggered: win.app.safety.openIgnore(String(win.actor.mxid || ""), win.actorName)
        }
        Controls.MenuItem {
            visible: !!win.actor && win.actor.isMe !== true && !!win.app.safety
            height: visible ? implicitHeight : 0
            text: LinkGate.plain(visible ? String(win.app.safety.w.report) : "")
            icon.name: "dialog-warning"
            onTriggered: win.app.safety.openReport(String(win.actor.mxid || ""), win.spaceId, "", win.actorName)
        }
        Controls.MenuSeparator { visible: rolesItem.visible; height: visible ? implicitHeight : 0 }
        Controls.MenuItem {
            id: rolesItem
            text: "Roles…"
            icon.name: "security-medium-symbolic"
            visible: win.rolesActive && !!win.actor && win.actor.canEditRoles === true && win.actorMembership === "join"
            height: visible ? implicitHeight : 0
            onTriggered: rolesMenu.popup(memberMenu.parent, memberMenu.x + Kirigami.Units.gridUnit, memberMenu.y + rolesItem.y)
        }
        Controls.MenuSeparator { visible: kickItem.visible || banItem.visible || unbanItem.visible; height: visible ? implicitHeight : 0 }
        Controls.MenuItem {
            id: kickItem
            text: win.actorMembership === "invite" ? "Withdraw invite…" : "Remove from space…"
            icon.name: "im-kick-user-symbolic"
            visible: !!win.actor && win.actor.canKick === true && win.actorMembership !== "ban"
            height: visible ? implicitHeight : 0
            onTriggered: kickConfirm.open()
        }
        Controls.MenuItem {
            id: banItem
            text: "Ban from space…"
            icon.name: "im-ban-user-symbolic"
            visible: !!win.actor && win.actor.canBan === true && win.actorMembership !== "ban"
            height: visible ? implicitHeight : 0
            onTriggered: banConfirm.open()
        }
        Controls.MenuItem {
            id: unbanItem
            text: "Unban"
            icon.name: "checkmark-symbolic"
            visible: !!win.actor && win.actor.canBan === true && win.actorMembership === "ban"
            height: visible ? implicitHeight : 0
            onTriggered: win.unban(String(win.actor.mxid || ""))
        }
        Controls.MenuSeparator {}
        Controls.MenuItem {
            text: "Manage roles…"
            icon.name: "configure-symbolic"
            onTriggered: win.tab = 2
        }
    }
    // Which roles this member holds — checkable, declarative (CheckMenuItem), rank-gated.
    Controls.Menu {
        id: rolesMenu
        modal: true
        margins: 6
        property var actorRoles: []
        MenuSectionLabel { app: win.app; text: "Roles for " + win.actorName; width: Math.min(implicitWidth, 260); elide: Text.ElideRight }
        Repeater {
            model: win.roles
            delegate: CheckMenuItem {
                id: roleToggle
                required property var modelData
                readonly property bool assignable: Number(modelData.rank) <= win.myRank
                visible: modelData.isEveryone !== true
                height: visible ? implicitHeight : 0
                text: String(modelData.name || modelData.id)
                opacity: assignable ? 1.0 : 0.5
                isChecked: () => rolesMenu.actorRoles.indexOf(roleToggle.modelData.id) >= 0
                Controls.ToolTip.text: assignable ? "" : "You can't give a role ranked above yours"
                Controls.ToolTip.visible: hovered && !assignable
                Controls.ToolTip.delay: 300
                onTriggered: {
                    if (!assignable) { win.toast("You can't give a role ranked above yours"); return }
                    var id = String(roleToggle.modelData.id)
                    var next = rolesMenu.actorRoles.filter(function (r) { return r !== id })
                    if (rolesMenu.actorRoles.indexOf(id) < 0) next.push(id)
                    rolesMenu.actorRoles = next
                    win.setMemberRoles(String(win.actor.mxid || ""), JSON.stringify(next))
                }
            }
        }
        Controls.MenuSeparator {}
        Controls.MenuItem {
            text: "Manage roles…"
            icon.name: "configure-symbolic"
            onTriggered: win.tab = 2
        }
    }

    // ---- invite popover ----
    Controls.Popup {
        id: invitePopover
        parent: Controls.Overlay.overlay
        modal: true
        focus: true
        anchors.centerIn: parent
        width: 440
        padding: Kirigami.Units.largeSpacing + 4
        closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
        background: Kirigami.ShadowedRectangle {
            radius: 14
            color: Th.invitePopoverBg
            border.width: 1
            border.color: Th.invitePopoverBorder
            shadow.size: 18; shadow.color: Th.invitePopoverShadow; shadow.yOffset: 4
        }
        onOpened: { inviteQuery.text = ""; inviteQuery.forceActiveFocus() }
        Timer { id: searchDebounce; interval: 250; onTriggered: win.searchUsers(inviteQuery.text.trim()) }
        ColumnLayout {
            anchors.fill: parent
            spacing: Kirigami.Units.smallSpacing
            Kirigami.Heading { textFormat: Text.PlainText; level: 3; text: "Invite to " + (win.spaceName.length > 0 ? win.spaceName : "the space"); color: Th.invitePopoverFg; elide: Text.ElideRight; Layout.fillWidth: true }
            HintLabel { app: win.app; Layout.fillWidth: true; text: "Search the directory, or paste a full @user:server id and press Enter." }
            ThTextField {
                id: inviteQuery
                Layout.fillWidth: true
                Layout.topMargin: Kirigami.Units.smallSpacing
                placeholderText: "Name or @user:server"
                onTextChanged: searchDebounce.restart()
                onAccepted: win.inviteFromQuery()
            }
            ListView {
                id: inviteResults
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(Kirigami.Units.gridUnit * 14, Math.max(Kirigami.Units.gridUnit * 4, count * (Kirigami.Units.gridUnit * 2.4) + 4))
                clip: true
                spacing: 2
                model: win.userSearch
                Controls.ScrollBar.vertical: FoxScrollBar { app: win.app }
                delegate: Item {
                    id: hit
                    required property var modelData
                    width: inviteResults.width
                    height: Kirigami.Units.gridUnit * 2.4
                    readonly property bool taken: modelData.alreadyMember === true
                    Rectangle {
                        anchors.fill: parent
                        radius: win.theme.radiusSm
                        color: hitMa.containsMouse && !hit.taken ? Th.inviteResultBgHover : "transparent"
                        Behavior on color { ColorAnimation { duration: win.dur } }
                    }
                    MouseArea {
                        id: hitMa
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: hit.taken ? Qt.ArrowCursor : Qt.PointingHandCursor
                        enabled: !hit.taken
                        onClicked: { win.invite(String(hit.modelData.mxid)); inviteQuery.text = "" }
                    }
                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: Kirigami.Units.smallSpacing
                        anchors.rightMargin: Kirigami.Units.smallSpacing
                        spacing: Kirigami.Units.smallSpacing
                        opacity: hit.taken ? 0.55 : 1.0
                        Avatar { app: win.theme; size: 28; name: hit.modelData.name || hit.modelData.mxid; seed: hit.modelData.mxid || ""; imageSource: hit.modelData.avatar || "" }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 0
                            Controls.Label { textFormat: Text.PlainText; Layout.fillWidth: true; text: hit.modelData.name || win.localpart(hit.modelData.mxid); color: Th.invitePopoverFg; font.bold: true; elide: Text.ElideRight }
                            Controls.Label { textFormat: Text.PlainText; Layout.fillWidth: true; text: hit.modelData.mxid || ""; color: Th.inviteHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; elide: Text.ElideMiddle }
                        }
                        Controls.Label { visible: hit.taken; text: "already a member"; color: Th.inviteHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; font.italic: true }
                        ThIcon { visible: !hit.taken; app: win.app; name: "list-add-user"; width: 16; height: 16; color: Th.inviteResultIcon; hovered: hitMa.containsMouse }
                    }
                }
                Controls.Label {
                    anchors.centerIn: parent
                    width: parent.width - Kirigami.Units.largeSpacing * 2
                    visible: inviteResults.count === 0
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    text: inviteQuery.text.trim().length === 0 ? "Type a name to search" : "No one by that name yet — a full @user:server id always works"
                    color: Th.inviteHintFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: Kirigami.Units.smallSpacing
                Item { Layout.fillWidth: true }
                ThButton { app: win.app; flat: true; text: "Done"; onClicked: invitePopover.close() }
            }
        }
    }

    // ======================================================================================
    RowLayout {
        anchors.fill: parent
        spacing: 0

        // ---- rail ---------------------------------------------------------------------------
        Rectangle {
            Layout.preferredWidth: Kirigami.Units.gridUnit * 11
            Layout.fillHeight: true
            color: Th.spaceSettingsRailBg
            ColumnLayout {
                anchors.fill: parent
                anchors.margins: Kirigami.Units.largeSpacing
                spacing: Kirigami.Units.smallSpacing

                Avatar {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.topMargin: Kirigami.Units.smallSpacing
                    app: win.theme
                    size: 56
                    radius: 14
                    name: win.spaceName
                    seed: win.spaceId.length > 0 ? win.spaceId : win.spaceName
                    imageSource: (win.summary && win.summary.avatar) ? String(win.summary.avatar) : ""
                }
                Kirigami.Heading {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    level: 3
                    horizontalAlignment: Text.AlignHCenter
                    text: win.spaceName.length > 0 ? win.spaceName : "Space"
                    color: Th.spaceSettingsTitleFg
                    elide: Text.ElideRight
                    maximumLineCount: 2
                    wrapMode: Text.WordWrap
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    text: Number((win.summary && win.summary.memberCount) ? win.summary.memberCount : win.joinedCount)
                          + (Number((win.summary && win.summary.memberCount) ? win.summary.memberCount : win.joinedCount) === 1 ? " member" : " members")
                          + " · " + Number((win.summary && win.summary.roomCount) ? win.summary.roomCount : win.rooms.length)
                          + (Number((win.summary && win.summary.roomCount) ? win.summary.roomCount : win.rooms.length) === 1 ? " room" : " rooms")
                    color: Th.spaceSettingsHintFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    elide: Text.ElideRight
                }
                Kirigami.Separator { Layout.fillWidth: true; Layout.topMargin: Kirigami.Units.smallSpacing; Layout.bottomMargin: Kirigami.Units.smallSpacing }

                // Category rail — the Settings sheet's idiom (themed rounded delegates; arrows cycle).
                ColumnLayout {
                    id: rail
                    Layout.fillWidth: true
                    spacing: 2
                    focus: true
                    Keys.onUpPressed: win.tab = (win.tab + win.tabs.length - 1) % win.tabs.length
                    Keys.onDownPressed: win.tab = (win.tab + 1) % win.tabs.length
                    Repeater {
                        model: win.tabs
                        delegate: Controls.ItemDelegate {
                            id: railItem
                            required property var modelData
                            required property int index
                            Layout.fillWidth: true
                            highlighted: win.tab === index
                            hoverEnabled: true
                            padding: Kirigami.Units.smallSpacing + 2
                            leftPadding: Kirigami.Units.largeSpacing
                            Accessible.name: modelData.t
                            onClicked: { win.tab = index; rail.forceActiveFocus() }
                            contentItem: RowLayout {
                                spacing: Kirigami.Units.smallSpacing + 2
                                ThIcon {
                                    Layout.preferredWidth: 18; Layout.preferredHeight: 18
                                    app: win.app
                                    name: railItem.modelData.i
                                    color: railItem.highlighted ? Th.spaceSettingsRailItemIconSelected : Th.spaceSettingsRailItemIcon
                                    hovered: railItem.hovered
                                    active: railItem.highlighted
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    Layout.fillWidth: true
                                    text: railItem.modelData.t
                                    color: Th.spaceSettingsFg
                                    font.bold: railItem.highlighted
                                    elide: Text.ElideRight
                                }
                            }
                            background: Rectangle {
                                radius: win.theme.radiusSm
                                color: railItem.highlighted ? Th.spaceSettingsRailItemBgSelected
                                     : railItem.hovered ? Th.spaceSettingsRailItemBgHover : "transparent"
                                border.width: railItem.visualFocus || (rail.activeFocus && railItem.highlighted) ? 1 : 0
                                border.color: Th.spaceSettingsRailItemBorderFocus
                                Behavior on color { ColorAnimation { duration: win.dur } }
                            }
                        }
                    }
                }
                Item { Layout.fillHeight: true }
                // Leave — danger text, confirmed.
                ThButton {
                    Layout.fillWidth: true
                    app: win.app
                    flat: true
                    text: "Leave space"
                    visible: !!(win.summary && win.summary.isMember === true)
                    onClicked: leaveConfirm.open()
                    // the flat house button paints accentBright; this one is destructive
                    contentItem: Row {
                        spacing: 6
                        ThIcon { anchors.verticalCenter: parent.verticalCenter; app: win.app; name: "go-previous-symbolic"; width: 16; height: 16; color: Th.spaceSettingsLeaveFg }
                        Text { anchors.verticalCenter: parent.verticalCenter; text: "Leave space"; color: Th.spaceSettingsLeaveFg; font.pointSize: Kirigami.Theme.defaultFont.pointSize; font.bold: true }
                    }
                }
            }
        }
        Kirigami.Separator { Layout.fillHeight: true }

        // ---- page ---------------------------------------------------------------------------
        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.margins: Kirigami.Units.largeSpacing * 2
            spacing: Kirigami.Units.largeSpacing

            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.largeSpacing
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 2
                    Kirigami.Heading { textFormat: Text.PlainText; level: 2; text: win.tabs[win.tab].t; color: Th.spaceSettingsTitleFg }
                    Controls.Label { textFormat: Text.PlainText; Layout.fillWidth: true; text: win.tabs[win.tab].d; color: Th.spaceSettingsHintFg; elide: Text.ElideRight }
                }
                // the status strip: the toast, where you are looking
                Rectangle {
                    visible: win.stripText.length > 0
                    implicitWidth: Math.min(stripLabel.implicitWidth + Kirigami.Units.largeSpacing * 2, Kirigami.Units.gridUnit * 22)
                    implicitHeight: stripLabel.implicitHeight + Kirigami.Units.smallSpacing * 2
                    radius: height / 2
                    color: win.stripError ? Th.spaceSettingsStripBgError : Th.spaceSettingsStripBg
                    border.width: 1
                    border.color: win.stripError ? Th.spaceSettingsStripBorderError : Th.spaceSettingsStripBorder
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        id: stripLabel
                        anchors.centerIn: parent
                        width: Math.min(implicitWidth, parent.width - Kirigami.Units.largeSpacing * 2)
                        text: win.stripText
                        color: win.stripError ? Th.spaceSettingsStripFgError : Th.spaceSettingsStripFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        elide: Text.ElideRight
                    }
                }
                ThBusyIndicator {   // 11.0 (GPU.md §2.5): still
                    visible: win.busy
                    running: visible
                    size: 22
                    color: Th.loaderSpinnerFg
                }
                ThToolButton {
                    app: win.app
                    iconName: "view-refresh-symbolic"
                    tip: "Refresh"
                    onClicked: win.refreshRequested()
                }
            }
            Kirigami.Separator { Layout.fillWidth: true }

            StackLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                currentIndex: win.tab

                // ================================ Overview ==================================
                Controls.ScrollView {
                    id: ovScroll
                    contentWidth: availableWidth
                    clip: true
                    Controls.ScrollBar.vertical: FoxScrollBar { app: win.app }
                    ColumnLayout {
                        width: ovScroll.availableWidth
                        spacing: Kirigami.Units.largeSpacing

                        RowLayout {
                            Layout.fillWidth: true
                            Layout.maximumWidth: Kirigami.Units.gridUnit * 40
                            spacing: Kirigami.Units.largeSpacing * 1.5
                            // photo: click to change, hover overlay, × to remove
                            Item {
                                id: photoBox
                                Layout.alignment: Qt.AlignTop
                                Layout.preferredWidth: 96; Layout.preferredHeight: 96
                                readonly property bool hasPhoto: !!(win.summary && win.summary.avatar && String(win.summary.avatar).length > 0)
                                Avatar {
                                    anchors.fill: parent
                                    app: win.theme
                                    size: 96
                                    radius: 24
                                    name: win.spaceName
                                    seed: win.spaceId.length > 0 ? win.spaceId : win.spaceName
                                    imageSource: photoBox.hasPhoto ? String(win.summary.avatar) : ""
                                }
                                Rectangle {
                                    anchors.fill: parent
                                    radius: 24
                                    color: Th.spaceSettingsPhotoOverlay
                                    opacity: win.canManageSpace && photoHover.hovered ? 1 : 0
                                    Behavior on opacity { NumberAnimation { duration: win.dur } }
                                    ColumnLayout {
                                        anchors.centerIn: parent
                                        spacing: 2
                                        ThIcon { Layout.alignment: Qt.AlignHCenter; app: win.app; name: "insert-image-symbolic"; width: 20; height: 20; color: Th.spaceSettingsPhotoIcon; hovered: photoHover.hovered }
                                        Controls.Label { text: "Change photo"; color: Th.spaceSettingsPhotoFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; font.bold: true }
                                    }
                                }
                                HoverHandler { id: photoHover; enabled: win.canManageSpace; cursorShape: Qt.PointingHandCursor }
                                TapHandler { enabled: win.canManageSpace; onTapped: photoDialog.open() }
                                Accessible.role: Accessible.Button
                                Accessible.name: "Change photo"
                                ThToolButton {
                                    anchors.top: parent.top; anchors.right: parent.right
                                    anchors.topMargin: -8; anchors.rightMargin: -8
                                    implicitWidth: 26; implicitHeight: 26
                                    visible: win.canManageSpace && photoBox.hasPhoto
                                    app: win.app
                                    iconName: "dialog-close-symbolic"
                                    iconSize: 12
                                    tip: "Remove photo"
                                    onClicked: win.applyPhoto("")
                                }
                            }
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: Kirigami.Units.smallSpacing
                                SectionLabel { app: win.app; text: "Name" }
                                ThTextField {
                                    id: nameField
                                    Layout.fillWidth: true
                                    enabled: win.canManageSpace
                                    placeholderText: "Space name"
                                    onAccepted: if (win.overviewDirty) win.saveOverview()
                                }
                                SectionLabel { app: win.app; text: "Topic"; Layout.topMargin: Kirigami.Units.smallSpacing }
                                ThTextArea {
                                    id: topicField
                                    Layout.fillWidth: true
                                    Layout.minimumHeight: Kirigami.Units.gridUnit * 4.6   // three lines
                                    enabled: win.canManageSpace
                                    placeholderText: "What this space is for"
                                }
                                HintLabel { app: win.app; Layout.fillWidth: true; visible: !win.canManageSpace; text: "Only people with Manage space can change these." }
                                // Save / Discard — only while something differs from the space.
                                RowLayout {
                                    Layout.fillWidth: true
                                    Layout.topMargin: Kirigami.Units.smallSpacing
                                    visible: win.overviewDirty && win.canManageSpace
                                    spacing: Kirigami.Units.smallSpacing
                                    GlowButton { app: win.app; text: "Save changes"; busy: win.busy; enabled: !win.busy && nameField.text.trim().length > 0; onClicked: win.saveOverview() }
                                    ThButton { app: win.app; flat: true; text: "Discard"; onClicked: win.discardOverview() }
                                    Item { Layout.fillWidth: true }
                                }
                            }
                        }

                        Kirigami.Separator { Layout.fillWidth: true; Layout.maximumWidth: Kirigami.Units.gridUnit * 40 }
                        SectionLabel { app: win.app; text: "Who can join" }
                        ThRadioButton {
                            id: ruleInvite
                            text: "Invite only — people need an invite"
                            enabled: win.canManageSpace
                            onClicked: if (win.joinRule === "public") win.applyJoinRule("invite")
                        }
                        ThRadioButton {
                            id: rulePublic
                            text: "Anyone with the link"
                            enabled: win.canManageSpace
                            onClicked: if (win.joinRule !== "public") win.applyJoinRule("public")
                        }
                        HintLabel {
                            app: win.app
                            Layout.fillWidth: true
                            Layout.maximumWidth: Kirigami.Units.gridUnit * 40
                            visible: win.joinRule === "restricted" || win.joinRule === "knock"
                            text: win.joinRule === "restricted"
                                ? "Right now members of a parent space can join on their own (restricted). Picking an option above replaces that."
                                : "Right now people can knock to ask for an invite. Picking an option above replaces that."
                        }

                        Kirigami.Separator { Layout.fillWidth: true; Layout.maximumWidth: Kirigami.Units.gridUnit * 40 }
                        SectionLabel { app: win.app; text: "Space link" }
                        RowLayout {
                            Layout.fillWidth: true
                            Layout.maximumWidth: Kirigami.Units.gridUnit * 40
                            spacing: Kirigami.Units.smallSpacing
                            ThTextField {
                                id: linkField
                                Layout.fillWidth: true
                                readOnly: true
                                text: (win.summary && win.summary.link) ? String(win.summary.link) : ""
                                placeholderText: "No link yet"
                            }
                            ThButton {
                                app: win.app
                                text: "Copy"
                                icon.name: "edit-copy-symbolic"
                                enabled: linkField.text.length > 0
                                onClicked: win.copyLink(linkField.text)
                            }
                        }
                        HintLabel {
                            app: win.app
                            Layout.fillWidth: true
                            Layout.maximumWidth: Kirigami.Units.gridUnit * 40
                            text: win.joinRule === "public" ? "Anyone with this link can join." : "People with this link still need an invite."
                        }

                        Kirigami.Separator { Layout.fillWidth: true; Layout.maximumWidth: Kirigami.Units.gridUnit * 40 }
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            Layout.fillWidth: true
                            text: {
                                var s = win.summary || {}
                                var bits = []
                                var mc = Number(s.memberCount || win.joinedCount); bits.push(mc + (mc === 1 ? " member" : " members"))
                                if (Number(s.invitedCount || 0) > 0) bits.push(Number(s.invitedCount) + " invited")
                                if (Number(s.bannedCount || 0) > 0) bits.push(Number(s.bannedCount) + " banned")
                                var rc = Number(s.roomCount || win.rooms.length); bits.push(rc + (rc === 1 ? " room" : " rooms"))
                                if (s.encrypted === true) bits.push("encrypted")
                                if (s.alias && String(s.alias).length > 0) bits.push(String(s.alias))
                                return bits.join("  ·  ")
                            }
                            color: Th.spaceSettingsHintFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                            elide: Text.ElideRight
                        }
                        Item { Layout.fillHeight: true; Layout.preferredHeight: Kirigami.Units.largeSpacing }
                    }
                }

                // ================================ Members ===================================
                ColumnLayout {
                    spacing: Kirigami.Units.smallSpacing
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Kirigami.Units.smallSpacing
                        ThTextField {
                            id: memberSearch
                            Layout.preferredWidth: Kirigami.Units.gridUnit * 16
                            placeholderText: "Search by name or id…"
                            onTextChanged: win.memberQuery = text
                            Keys.onEscapePressed: (event) => { if (text.length > 0) { text = ""; event.accepted = true } else event.accepted = false }
                        }
                        // filter pills — the CalendarWindow tab idiom
                        Row {
                            spacing: 2
                            Repeater {
                                model: [
                                    { k: "all",     l: "All",     n: win.joinedCount },
                                    { k: "invited", l: "Invited", n: win.invitedCount },
                                    { k: "banned",  l: "Banned",  n: win.bannedCount }
                                ]
                                delegate: Controls.AbstractButton {
                                    id: pill
                                    required property var modelData
                                    readonly property bool on: win.memberFilter === modelData.k
                                    hoverEnabled: true
                                    implicitHeight: 30
                                    implicitWidth: pillRow.implicitWidth + 22
                                    Accessible.name: modelData.l
                                    onClicked: win.memberFilter = modelData.k
                                    background: Rectangle {
                                        radius: 15
                                        color: pill.on ? Th.spaceSettingsFilterBgSelected : (pill.hovered ? Th.spaceSettingsFilterBgHover : "transparent")
                                        border.width: pill.on ? 1 : 0
                                        border.color: Th.spaceSettingsFilterBorderSelected
                                        Behavior on color { ColorAnimation { duration: win.dur } }
                                    }
                                    contentItem: Item {
                                        Row {
                                            id: pillRow
                                            anchors.centerIn: parent
                                            spacing: 6
                                            Controls.Label { textFormat: Text.PlainText; anchors.verticalCenter: parent.verticalCenter; text: pill.modelData.l; color: pill.on ? Th.spaceSettingsFilterFgSelected : Th.spaceSettingsFilterFg; font.bold: pill.on }
                                            Controls.Label { textFormat: Text.PlainText; anchors.verticalCenter: parent.verticalCenter; text: pill.modelData.n; color: Th.spaceSettingsFilterCountFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                        }
                                    }
                                }
                            }
                        }
                        Item { Layout.fillWidth: true }
                        GlowButton {
                            app: win.app
                            text: "Invite"
                            icon.name: "list-add-user"
                            visible: win.canInvite
                            onClicked: invitePopover.open()
                        }
                    }

                    ListView {
                        id: memberList
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        spacing: 2
                        model: win.visibleMembers
                        visible: win.visibleMembers.length > 0
                        Controls.ScrollBar.vertical: FoxScrollBar { app: win.app }
                        section.property: "section"
                        section.criteria: ViewSection.FullString
                        section.delegate: Item {
                            id: secHeader
                            required property string section
                            width: memberList.width
                            height: Kirigami.Units.gridUnit * 1.8
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                anchors.left: parent.left
                                anchors.leftMargin: Kirigami.Units.smallSpacing
                                anchors.bottom: parent.bottom
                                anchors.bottomMargin: 3
                                text: secHeader.section + " — " + win.sectionCount(secHeader.section)
                                color: win.sectionColor(secHeader.section)
                                font.bold: true
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                font.letterSpacing: 0.4
                            }
                        }
                        delegate: Item {
                            id: mrow
                            required property var modelData
                            width: memberList.width
                            height: Math.round(Kirigami.Units.gridUnit * 2.8)
                            readonly property bool isMenuTarget: memberMenu.opened && win.actor && win.actor.mxid === modelData.mxid
                            Rectangle {
                                anchors.fill: parent
                                radius: win.theme.radius
                                color: (mma.containsMouse || mrow.isMenuTarget) ? Th.spaceSettingsMemberRowBgHover : "transparent"
                                Behavior on color { ColorAnimation { duration: win.dur } }
                            }
                            MouseArea {
                                id: mma
                                anchors.fill: parent
                                hoverEnabled: true
                                acceptedButtons: Qt.LeftButton | Qt.RightButton
                                cursorShape: Qt.PointingHandCursor
                                onClicked: (mouse) => {
                                    if (mouse.button === Qt.RightButton) win.openMemberMenu(mrow.modelData, mrow)
                                    else win.viewProfile(String(mrow.modelData.mxid || ""))
                                }
                            }
                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: Kirigami.Units.smallSpacing
                                anchors.rightMargin: Kirigami.Units.smallSpacing
                                spacing: Kirigami.Units.smallSpacing + 2
                                Avatar {
                                    app: win.theme
                                    size: 36
                                    name: mrow.modelData.name || mrow.modelData.mxid
                                    seed: mrow.modelData.mxid || ""
                                    imageSource: mrow.modelData.avatar || ""
                                    opacity: mrow.modelData.membership === "ban" ? 0.5 : (mrow.modelData.membership === "invite" ? 0.75 : 1)
                                }
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 0
                                    RowLayout {
                                        Layout.fillWidth: true
                                        spacing: Kirigami.Units.smallSpacing
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            Layout.maximumWidth: parent.width
                                            text: (mrow.modelData.name && String(mrow.modelData.name).length > 0) ? mrow.modelData.name : win.localpart(mrow.modelData.mxid)
                                            // 10.4 (space tags): the custom role's colour, else the hashed hue
                                            color: (mrow.modelData.tag && mrow.modelData.tag.color && String(mrow.modelData.tag.color).length > 0)
                                                   ? String(mrow.modelData.tag.color) : win.senderColor(mrow.modelData.mxid)
                                            font.bold: true
                                            elide: Text.ElideRight
                                        }
                                        Controls.Label { visible: mrow.modelData.isMe === true; text: "you"; color: Th.spaceSettingsHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; font.italic: true }
                                        Item { Layout.fillWidth: true }
                                    }
                                    Controls.Label {
                                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                        Layout.fillWidth: true
                                        text: (mrow.modelData.membership === "ban" && mrow.modelData.reason && String(mrow.modelData.reason).length > 0)
                                              ? mrow.modelData.mxid + "  ·  " + mrow.modelData.reason
                                              : (mrow.modelData.mxid || "")
                                        color: Th.spaceSettingsHintFg
                                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                                        elide: Text.ElideMiddle
                                    }
                                }
                                // role chips
                                Row {
                                    spacing: 4
                                    visible: mrow.modelData.membership !== "ban"
                                    Repeater {
                                        model: mrow.modelData.roles || []
                                        delegate: RoleBadge {
                                            required property var modelData
                                            app: win.app
                                            compact: true
                                            name: win.roleName(String(modelData))
                                            roleColor: win.roleColor(String(modelData))
                                        }
                                    }
                                }
                                // membership state (the ThRoomList "Invited" pill idiom)
                                Rectangle {
                                    visible: mrow.modelData.membership === "invite" || mrow.modelData.membership === "ban"
                                    radius: height / 2
                                    implicitHeight: stateText.implicitHeight + 4
                                    implicitWidth: stateText.implicitWidth + 12
                                    color: mrow.modelData.membership === "invite" ? Th.spaceSettingsMemberInvitedBg : Th.spaceSettingsMemberBannedBg
                                    border.width: mrow.modelData.membership === "invite" ? 0 : 1
                                    border.color: Th.spaceSettingsMemberBannedBorder
                                    Controls.Label {
                                        id: stateText
                                        anchors.centerIn: parent
                                        text: mrow.modelData.membership === "invite" ? "Invited" : "Banned"
                                        color: mrow.modelData.membership === "invite" ? Th.spaceSettingsMemberInvitedFg : Th.spaceSettingsMemberBannedFg
                                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    }
                                }
                                ThToolButton {
                                    app: win.app
                                    iconName: "view-more-symbolic"
                                    tip: "More"
                                    opacity: (mma.containsMouse || hovered || mrow.isMenuTarget) ? 1 : 0.35
                                    onClicked: win.openMemberMenu(mrow.modelData, mrow)
                                }
                            }
                        }
                    }
                    EmptyState {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        visible: win.visibleMembers.length === 0
                        app: win.theme
                        iconName: win.memberFilter === "banned" ? "im-ban-user-symbolic" : (win.memberFilter === "invited" ? "list-add-user" : "system-users-symbolic")
                        title: win.memberQuery.trim().length > 0 ? "No matches"
                             : (win.memberFilter === "invited" ? "Nobody's invited" : (win.memberFilter === "banned" ? "Nobody's banned" : (win.members.length === 0 && win.busy ? "Loading members…" : "Nobody here yet")))
                        explanation: win.memberQuery.trim().length > 0 ? "Try part of a name, or the @user:server id."
                                   : (win.memberFilter === "invited" ? (win.canInvite ? "Invite someone with the button above." : "Pending invites show up here.")
                                   : (win.memberFilter === "banned" ? "People you ban from the space are listed here so you can unban them." : ""))
                    }
                }

                // ================================ Roles =====================================
                Item {
                    // -- not set up yet --
                    Item {
                        anchors.fill: parent
                        visible: !win.rolesActive
                        Rectangle {
                            anchors.centerIn: parent
                            width: Math.min(parent.width - Kirigami.Units.gridUnit * 2, Kirigami.Units.gridUnit * 28)
                            height: setupCol.implicitHeight + Kirigami.Units.gridUnit * 2
                            radius: 12
                            color: Th.rolesSetupBg
                            border.width: 1
                            border.color: Th.rolesSetupBorder
                            ColumnLayout {
                                id: setupCol
                                anchors.fill: parent
                                anchors.margins: Kirigami.Units.gridUnit
                                spacing: Kirigami.Units.largeSpacing
                                ThIcon { Layout.alignment: Qt.AlignHCenter; app: win.app; name: "security-medium-symbolic"; width: 40; height: 40; color: Th.rolesSetupIcon; active: true }
                                Kirigami.Heading { Layout.fillWidth: true; level: 3; horizontalAlignment: Text.AlignHCenter; text: "Roles aren't set up in this space"; color: Th.spaceSettingsTitleFg; wrapMode: Text.WordWrap }
                                Controls.Label {
                                    Layout.fillWidth: true
                                    horizontalAlignment: Text.AlignHCenter
                                    wrapMode: Text.WordWrap
                                    color: Th.spaceSettingsHintFg
                                    text: "Tin Hat will create Admin, Moderator and Everyone from the current admins and moderators. Nothing changes for anyone until you edit a role."
                                }
                                GlowButton {
                                    Layout.alignment: Qt.AlignHCenter
                                    Layout.topMargin: Kirigami.Units.smallSpacing
                                    app: win.app
                                    visible: win.canManageRoles
                                    text: "Set up roles"
                                    busy: win.busy
                                    enabled: !win.busy
                                    onClicked: win.setupRoles()
                                }
                                HintLabel { app: win.app; Layout.fillWidth: true; horizontalAlignment: Text.AlignHCenter; visible: !win.canManageRoles; text: "Ask an admin — only people with Manage roles can set this up." }
                            }
                        }
                    }

                    // -- active: list + editor + footer --
                    ColumnLayout {
                        anchors.fill: parent
                        visible: win.rolesActive
                        spacing: Kirigami.Units.largeSpacing
                        RowLayout {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            spacing: Kirigami.Units.largeSpacing

                            // role list
                            ColumnLayout {
                                Layout.preferredWidth: Kirigami.Units.gridUnit * 13
                                Layout.fillHeight: true
                                spacing: 2
                                ListView {
                                    id: roleList
                                    Layout.fillWidth: true
                                    Layout.fillHeight: true
                                    clip: true
                                    spacing: 2
                                    model: win.roles
                                    Controls.ScrollBar.vertical: FoxScrollBar { app: win.app }
                                    delegate: Controls.ItemDelegate {
                                        id: roleRow
                                        required property var modelData
                                        width: roleList.width
                                        readonly property bool on: !win.editIsNew && win.editRoleId === modelData.id
                                        hoverEnabled: true
                                        padding: Kirigami.Units.smallSpacing + 2
                                        leftPadding: Kirigami.Units.largeSpacing
                                        Accessible.name: String(modelData.name || "")
                                        onClicked: win.selectRole(String(modelData.id))
                                        contentItem: RowLayout {
                                            spacing: Kirigami.Units.smallSpacing + 2
                                            Rectangle {
                                                Layout.preferredWidth: 12; Layout.preferredHeight: 12
                                                radius: 6
                                                color: String(roleRow.modelData.color || Th.rolesBadgeDefaultFg)
                                                border.width: 1; border.color: Th.rolesDotBorder
                                            }
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                Layout.fillWidth: true
                                                text: String(roleRow.modelData.name || roleRow.modelData.id)
                                                color: Th.spaceSettingsFg
                                                font.bold: roleRow.on
                                                elide: Text.ElideRight
                                            }
                                            ThIcon {
                                                visible: roleRow.modelData.isEveryone === true
                                                Layout.preferredWidth: 14; Layout.preferredHeight: 14
                                                app: win.app; name: "system-users-symbolic"; color: Th.spaceSettingsHintFg
                                            }
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                visible: roleRow.modelData.isEveryone !== true
                                                text: Number(roleRow.modelData.memberCount || 0)
                                                color: Th.spaceSettingsHintFg
                                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                            }
                                        }
                                        background: Rectangle {
                                            radius: win.theme.radiusSm
                                            color: roleRow.on ? Th.rolesListItemBgSelected : (roleRow.hovered ? Th.rolesListItemBgHover : "transparent")
                                            border.width: roleRow.visualFocus ? 1 : 0
                                            border.color: Th.rolesListItemBorderFocus
                                            Behavior on color { ColorAnimation { duration: win.dur } }
                                        }
                                    }
                                }
                                ThButton {
                                    Layout.fillWidth: true
                                    app: win.app
                                    flat: true
                                    visible: win.canManageRoles
                                    text: "New role"
                                    icon.name: "list-add-symbolic"
                                    highlighted: win.editIsNew
                                    onClicked: win.newRole()
                                }
                            }
                            Kirigami.Separator { Layout.fillHeight: true }

                            // editor: the form scrolls, the actions stay in view underneath (10.4 — the
                            // Create / Save buttons used to sit at the END of the permissions list, off screen
                            // at the default window height, which read as "create a role does nothing").
                            ColumnLayout {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                spacing: Kirigami.Units.smallSpacing
                            Controls.ScrollView {
                                id: roleScroll
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                contentWidth: availableWidth
                                clip: true
                                Controls.ScrollBar.vertical: FoxScrollBar { app: win.app }
                                ColumnLayout {
                                    width: roleScroll.availableWidth
                                    spacing: Kirigami.Units.smallSpacing
                                    enabled: win.editIsNew || !!win.editRole

                                    Kirigami.InlineMessage {
                                        Layout.fillWidth: true
                                        visible: !win.editEditable && (win.editIsNew || !!win.editRole)
                                        type: Kirigami.MessageType.Information
                                        text: win.canManageRoles ? "You can't edit a role ranked at or above yours." : "Only people with Manage roles can edit roles. You can still look."
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        spacing: Kirigami.Units.largeSpacing
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: Kirigami.Units.smallSpacing
                                            SectionLabel { app: win.app; text: "Name" }
                                            ThTextField {
                                                id: roleNameField
                                                Layout.fillWidth: true
                                                enabled: win.editEditable && !win.editIsEveryone
                                                placeholderText: "Role name"
                                                onTextEdited: win.dName = text
                                                Controls.ToolTip.text: win.editIsEveryone ? "Everyone is the baseline role — it can't be renamed" : ""
                                                Controls.ToolTip.visible: hovered && win.editIsEveryone
                                            }
                                        }
                                        ColumnLayout {
                                            Layout.preferredWidth: Kirigami.Units.gridUnit * 9
                                            spacing: Kirigami.Units.smallSpacing
                                            SectionLabel { app: win.app; text: "Rank" }
                                            RowLayout {
                                                spacing: Kirigami.Units.smallSpacing
                                                ThTextField {
                                                    id: roleRankField
                                                    Layout.preferredWidth: Kirigami.Units.gridUnit * 4
                                                    visible: !win.editIsAdminRank && !win.editIsEveryone
                                                    enabled: win.editEditable
                                                    inputMethodHints: Qt.ImhDigitsOnly
                                                    validator: IntValidator { bottom: 0; top: 99 }
                                                    horizontalAlignment: Text.AlignHCenter
                                                    onTextEdited: { var v = parseInt(text); win.dRank = isNaN(v) ? 0 : Math.max(0, Math.min(99, v)) }
                                                }
                                                Controls.Label {
                                                    visible: win.editIsAdminRank || win.editIsEveryone
                                                    text: win.editIsAdminRank ? "100" : "0"
                                                    color: Th.spaceSettingsFg
                                                    font.bold: true
                                                    leftPadding: Kirigami.Units.smallSpacing
                                                }
                                                Controls.Label {
                                                    visible: win.editIsAdminRank || win.editIsEveryone
                                                    text: win.editIsAdminRank ? "fixed" : "everyone"
                                                    color: Th.spaceSettingsHintFg
                                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                                }
                                            }
                                        }
                                    }
                                    HintLabel { app: win.app; Layout.fillWidth: true; text: "Higher ranks can manage lower ranks. Admin is 100." }

                                    SectionLabel { app: win.app; text: "Colour"; Layout.topMargin: Kirigami.Units.smallSpacing }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        spacing: Kirigami.Units.smallSpacing
                                        Row {
                                            spacing: 6
                                            Repeater {
                                                model: win.swatches
                                                delegate: Rectangle {
                                                    id: swatch
                                                    required property var modelData
                                                    readonly property bool on: String(win.dColor).toLowerCase() === Qt.color(modelData).toString().toLowerCase()   // theme-lint: ok content: a role colour swatch (wire data)
                                                    width: 26; height: 26
                                                    radius: 13
                                                    color: modelData
                                                    border.width: on ? 3 : (swatchMa.containsMouse ? 2 : 1)
                                                    border.color: on ? Th.rolesSwatchBorderSelected : (swatchMa.containsMouse ? Th.rolesSwatchBorderHover : Th.rolesSwatchBorder)
                                                    opacity: win.editEditable ? 1 : 0.5
                                                    Behavior on border.width { NumberAnimation { duration: win.dur } }
                                                    MouseArea {
                                                        id: swatchMa
                                                        anchors.fill: parent
                                                        hoverEnabled: true
                                                        enabled: win.editEditable
                                                        cursorShape: Qt.PointingHandCursor
                                                        onClicked: { win.dColor = Qt.color(swatch.modelData).toString(); roleHexField.text = win.dColor }   // theme-lint: ok content: a role colour swatch (wire data)
                                                    }
                                                }
                                            }
                                        }
                                        ThTextField {
                                            id: roleHexField
                                            Layout.preferredWidth: Kirigami.Units.gridUnit * 6
                                            enabled: win.editEditable
                                            placeholderText: "#rrggbb"
                                            validator: RegularExpressionValidator { regularExpression: /#?[0-9a-fA-F]{0,6}/ }
                                            onTextEdited: {
                                                var t = text.trim(); if (t.length > 0 && t.charAt(0) !== "#") t = "#" + t
                                                if (/^#[0-9a-fA-F]{6}$/.test(t)) win.dColor = t.toLowerCase()
                                            }
                                        }
                                        Rectangle { width: 18; height: 18; radius: 9; color: win.roleValid ? win.dColor : "transparent"; border.width: 1; border.color: Th.rolesPreviewBorder }
                                    }

                                    // 10.4 (space tags): what the role looks like on people — always the colour + tag;
                                    // the member list grouping is the choice (Discord's "display separately").
                                    HintLabel {
                                        app: win.app; Layout.fillWidth: true; Layout.topMargin: 2
                                        visible: !win.editIsEveryone
                                        text: win.editIsBuiltin ? "Admin and Moderator keep the colour for their badge only — names in chat stay as they are. Make a role of your own to colour and tag people."
                                                                : "People with this role get their name in this colour and the role as a tag next to it, in chat and in the member list."
                                    }
                                    ThCheckBox {
                                        visible: !win.editIsEveryone
                                        enabled: win.editEditable
                                        text: "Show them as their own group in the member list"
                                        checked: win.dHoist
                                        onToggled: win.dHoist = checked
                                    }
                                    SectionLabel { app: win.app; text: "Permissions"; Layout.topMargin: Kirigami.Units.largeSpacing }
                                    HintLabel { app: win.app; Layout.fillWidth: true; visible: win.editIsEveryone; text: "Everyone's permissions are the baseline for every member." }
                                    Repeater {
                                        model: win.permGroups
                                        delegate: ColumnLayout {
                                            id: permGroup
                                            required property var modelData
                                            Layout.fillWidth: true
                                            spacing: 2
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                Layout.topMargin: Kirigami.Units.smallSpacing
                                                text: String(permGroup.modelData.group)
                                                color: Th.rolesPermGroupFg
                                                font.bold: true
                                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                                font.letterSpacing: 0.6
                                            }
                                            Repeater {
                                                model: permGroup.modelData.items
                                                delegate: Rectangle {
                                                    id: permRow
                                                    required property var modelData
                                                    readonly property string key: String(modelData.key)
                                                    readonly property bool honour: String(modelData.enforcement) === "client"
                                                    visible: win.app.permOffered(permRow.key)   // selfhost: a permission of a feature the server lacks is not offered
                                                    Layout.fillWidth: true
                                                    implicitHeight: permInner.implicitHeight + Kirigami.Units.smallSpacing * 2
                                                    radius: win.theme.radiusSm
                                                    color: permHover.hovered ? Th.rolesPermBgHover : "transparent"
                                                    HoverHandler { id: permHover }
                                                    RowLayout {
                                                        id: permInner
                                                        anchors.fill: parent
                                                        anchors.margins: Kirigami.Units.smallSpacing
                                                        spacing: Kirigami.Units.smallSpacing + 2
                                                        ThCheckBox {
                                                            id: permCheck
                                                            Layout.alignment: Qt.AlignTop
                                                            enabled: win.editEditable
                                                            checked: win.hasPerm(permRow.key)
                                                            onToggled: {
                                                                win.togglePerm(permRow.key, checked)
                                                                checked = Qt.binding(function () { return win.hasPerm(permRow.key) })
                                                            }
                                                            Controls.ToolTip.text: win.editEditable ? "" : "You can't edit a role ranked at or above yours."
                                                            Controls.ToolTip.visible: hovered && !win.editEditable
                                                        }
                                                        ColumnLayout {
                                                            Layout.fillWidth: true
                                                            spacing: 0
                                                            Controls.Label {
                                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                                Layout.fillWidth: true
                                                                text: String(permRow.modelData.label || permRow.key)
                                                                color: Th.spaceSettingsFg
                                                                elide: Text.ElideRight
                                                                MouseArea { anchors.fill: parent; enabled: win.editEditable; onClicked: permCheck.toggle() }
                                                            }
                                                            HintLabel { app: win.app; Layout.fillWidth: true; text: String(permRow.modelData.description || "") }
                                                        }
                                                        Rectangle {
                                                            Layout.alignment: Qt.AlignTop
                                                            implicitWidth: chipText.implicitWidth + 12
                                                            implicitHeight: chipText.implicitHeight + 4
                                                            radius: height / 2
                                                            color: permRow.honour ? Th.rolesEnforcementHonourBg : Th.rolesEnforcementServerBg
                                                            border.width: 1
                                                            border.color: permRow.honour ? Th.rolesEnforcementHonourBorder : Th.rolesEnforcementServerBorder
                                                            HoverHandler { id: chipHover }
                                                            Controls.ToolTip.visible: chipHover.hovered
                                                            Controls.ToolTip.delay: 300
                                                            Controls.ToolTip.text: permRow.honour
                                                                ? "Honour system: Tin Hat hides or blocks this, but a modified client could ignore it."
                                                                : "Server-enforced: the homeserver or the Tin Hat server refuses it — a modified client can't get around it."
                                                            Controls.Label {
                                                                id: chipText
                                                                anchors.centerIn: parent
                                                                text: permRow.honour ? "Honour system" : "Server-enforced"
                                                                color: permRow.honour ? Th.rolesEnforcementHonourFg : Th.rolesEnforcementServerFg
                                                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    Item { Layout.preferredHeight: Kirigami.Units.largeSpacing }
                                }
                            }
                            // Action bar — always visible while a role is open: unsaved-changes state on the
                            // left, Create / Save + Discard when dirty, Delete on the right.
                            Rectangle {
                                id: roleActions
                                Layout.fillWidth: true
                                visible: win.editIsNew || !!win.editRole
                                implicitHeight: actRow.implicitHeight + Kirigami.Units.smallSpacing * 2
                                radius: win.theme.radius
                                color: (win.roleDirty && win.editEditable) ? Th.rolesActionBarBgActive : Th.rolesActionBarBg
                                border.width: 1
                                border.color: (win.roleDirty && win.editEditable) ? Th.rolesActionBarBorderActive : Th.rolesActionBarBorder
                                Behavior on color { ColorAnimation { duration: win.dur } }
                                Behavior on border.color { ColorAnimation { duration: win.dur } }
                                RowLayout {
                                    id: actRow
                                    anchors.fill: parent
                                    anchors.leftMargin: Kirigami.Units.largeSpacing
                                    anchors.rightMargin: Kirigami.Units.smallSpacing
                                    anchors.topMargin: Kirigami.Units.smallSpacing
                                    anchors.bottomMargin: Kirigami.Units.smallSpacing
                                    spacing: Kirigami.Units.smallSpacing
                                    Controls.Label {
                                        Layout.fillWidth: true
                                        text: (win.roleDirty && win.editEditable)
                                              ? (win.editIsNew ? (win.roleValid ? "New role — not created yet" : "Give the role a name to create it")
                                                               : "Unsaved changes")
                                              : "Changes apply to every room in this space"
                                        color: (win.roleDirty && win.editEditable) ? Th.rolesActionBarFgActive : Th.rolesActionBarFg
                                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                                        elide: Text.ElideRight
                                    }
                                    ThButton { app: win.app; flat: true; visible: win.roleDirty; text: "Discard"; onClicked: win.discardRoleNow() }
                                    GlowButton {
                                        app: win.app
                                        visible: win.roleDirty && win.editEditable
                                        text: win.editIsNew ? "Create role" : "Save changes"
                                        busy: win.busy
                                        enabled: !win.busy && win.roleValid
                                        onClicked: win.saveRoleNow()
                                    }
                                    ThButton {
                                        app: win.app
                                        flat: true
                                        visible: !win.editIsNew && !!win.editRole && win.editRole.deletable === true && !win.editIsEveryone && !win.roleDirty
                                        text: "Delete role"
                                        onClicked: deleteRoleConfirm.open()
                                        contentItem: Text { text: "Delete role"; color: Th.rolesDeleteFg; font.pointSize: Kirigami.Theme.defaultFont.pointSize; verticalAlignment: Text.AlignVCenter }
                                    }
                                }
                            }
                            }
                        }
                        // footer strip
                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: footRow.implicitHeight + Kirigami.Units.smallSpacing * 2
                            radius: win.theme.radius
                            color: Th.rolesFooterBg
                            border.width: 1
                            border.color: Th.rolesFooterBorder
                            RowLayout {
                                id: footRow
                                anchors.fill: parent
                                anchors.leftMargin: Kirigami.Units.largeSpacing
                                anchors.rightMargin: Kirigami.Units.smallSpacing
                                spacing: Kirigami.Units.smallSpacing
                                ThIcon { app: win.app; name: "view-list-details-symbolic"; width: 16; height: 16; color: Th.spaceSettingsHintFg }
                                Controls.Label { Layout.fillWidth: true; text: "Roles apply to every room in this space"; color: Th.spaceSettingsHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; elide: Text.ElideRight }
                                Rectangle {
                                    visible: win.outOfSync > 0
                                    implicitWidth: oosText.implicitWidth + 14
                                    implicitHeight: oosText.implicitHeight + 6
                                    radius: height / 2
                                    color: Th.spaceSettingsSyncChipStaleBg
                                    border.width: 1
                                    border.color: Th.spaceSettingsSyncChipStaleBorder
                                    Controls.Label { textFormat: Text.PlainText; id: oosText; anchors.centerIn: parent; text: win.outOfSync + (win.outOfSync === 1 ? " room out of sync" : " rooms out of sync"); color: Th.spaceSettingsSyncChipStaleFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                }
                                ThButton {
                                    app: win.app
                                    visible: win.outOfSync > 0 && win.canManageRoles
                                    text: "Sync now"
                                    icon.name: "view-refresh-symbolic"
                                    implicitHeight: 28
                                    enabled: !win.busy
                                    onClicked: win.syncRooms()
                                }
                            }
                        }
                    }
                }

                // ================================ Rooms =====================================
                ColumnLayout {
                    spacing: Kirigami.Units.smallSpacing
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Kirigami.Units.smallSpacing
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            Layout.fillWidth: true
                            text: win.rooms.length + (win.rooms.length === 1 ? " room" : " rooms")
                                  + (win.rolesActive ? (win.outOfSync > 0 ? "  ·  " + win.outOfSync + " out of sync" : "  ·  all in sync") : "")
                            color: Th.spaceSettingsHintFg
                            elide: Text.ElideRight
                        }
                        ThButton {
                            app: win.app
                            visible: win.rolesActive && win.canManageRoles
                            enabled: !win.busy && win.outOfSync > 0
                            text: "Sync all"
                            icon.name: "view-refresh-symbolic"
                            onClicked: win.syncRooms()
                        }
                    }
                    HintLabel {
                        app: win.app
                        Layout.fillWidth: true
                        visible: !win.rolesActive
                        text: "Set up roles (Roles tab) to keep every room's permissions in step with the space."
                    }
                    ListView {
                        id: roomList
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        spacing: 2
                        model: win.rooms
                        visible: win.rooms.length > 0
                        Controls.ScrollBar.vertical: FoxScrollBar { app: win.app }
                        delegate: Item {
                            id: rrow
                            required property var modelData
                            width: roomList.width
                            height: Math.round(Kirigami.Units.gridUnit * 2.6)
                            Rectangle {
                                anchors.fill: parent
                                radius: win.theme.radius
                                color: rHover.hovered ? Th.spaceSettingsRoomRowBgHover : "transparent"
                                Behavior on color { ColorAnimation { duration: win.dur } }
                            }
                            HoverHandler { id: rHover }
                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: Kirigami.Units.smallSpacing
                                anchors.rightMargin: Kirigami.Units.smallSpacing
                                spacing: Kirigami.Units.smallSpacing + 2
                                Avatar {
                                    app: win.theme
                                    size: 32
                                    radius: 8
                                    name: rrow.modelData.name || rrow.modelData.id
                                    seed: rrow.modelData.id || ""
                                    imageSource: rrow.modelData.avatar || ""
                                    opacity: rrow.modelData.joined === false ? 0.6 : 1
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    Layout.fillWidth: true
                                    text: rrow.modelData.name || rrow.modelData.id
                                    color: rrow.modelData.joined === false ? Th.spaceSettingsRoomNameFgOff : Th.spaceSettingsRoomNameFg
                                    font.bold: true
                                    elide: Text.ElideRight
                                }
                                Rectangle {
                                    visible: rrow.modelData.joined === false
                                    radius: height / 2
                                    implicitHeight: njText.implicitHeight + 4
                                    implicitWidth: njText.implicitWidth + 12
                                    color: Th.spaceSettingsRoomNotJoinedBg
                                    border.width: 1
                                    border.color: Th.spaceSettingsRoomNotJoinedBorder
                                    Controls.Label { id: njText; anchors.centerIn: parent; text: "Not joined"; color: Th.spaceSettingsRoomNotJoinedFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                }
                                Rectangle {
                                    visible: win.rolesActive && rrow.modelData.joined !== false
                                    readonly property bool ok: rrow.modelData.synced === true
                                    implicitWidth: syncText.implicitWidth + 14
                                    implicitHeight: syncText.implicitHeight + 6
                                    radius: height / 2
                                    color: ok ? Th.spaceSettingsSyncChipOkBg : Th.spaceSettingsSyncChipStaleBg
                                    border.width: 1
                                    border.color: ok ? Th.spaceSettingsSyncChipOkBorder : Th.spaceSettingsSyncChipStaleBorder
                                    Controls.Label { id: syncText; anchors.centerIn: parent; text: parent.ok ? "Synced" : "Out of sync"; color: parent.ok ? Th.spaceSettingsSyncChipOkFg : Th.spaceSettingsSyncChipStaleFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                }
                                ThButton {
                                    app: win.app
                                    visible: win.rolesActive && rrow.modelData.canSync === true && rrow.modelData.synced !== true
                                    enabled: !win.busy
                                    text: "Sync"
                                    implicitHeight: 28
                                    onClicked: win.syncRoom(String(rrow.modelData.id || ""))
                                }
                            }
                        }
                    }
                    EmptyState {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        visible: win.rooms.length === 0
                        app: win.theme
                        iconName: "view-list-details-symbolic"
                        title: win.busy ? "Loading rooms…" : "No rooms in this space yet"
                        explanation: win.busy ? "" : "Rooms you create inside the space, or add to it, show up here."
                    }
                }
            }
        }
    }
}
