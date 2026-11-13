// PeoplePicker — the one add-people picker (2026-11-12, feat/people-search): room Invite, New DM and space Invite
// (Tin Hat Web's PeoplePicker.tsx). As you type: People you know (your DMs and rooms, instant, every server), On <your
// server> (the user directory, 250 ms debounced), and for a typed username/address nobody listed, one row YOUR server
// looks up first (name, picture, server) and explains in plain words when it can't. Anyone not on your server shows
// their server next to their name. Members are dimmed. Arrows + Enter pick; Esc is the dialog's. The rules are
// people.rs (shared vectors with the web); the data is people_ctl.rs.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

ColumnLayout {
    id: picker
    required property var app          // the main window (theme tokens, copyToClipboard)
    property string verb: "Invite"     // "Invite" (room/space) or "Message" (New DM)
    property string roomId: ""         // whose members are dimmed ("" = none)
    property bool keepPicking: true    // invites: clear the field after a pick and go on; New DM: false
    property string placeholder: "Name, username or @name:their.server"
    signal picked(string mxid)

    // the row Enter would pick now (a pickable person), "" = none: the dialog's own button acts on it
    readonly property string selectedMxid: (sel >= 0 && sel < rows.length && rows[sel].pickable) ? String(rows[sel].mxid) : ""
    readonly property var rows: { try { return JSON.parse(peopleCtl.rowsJson || "[]") } catch (e) { return [] } }
    // The selection is a PERSON (and the text it was made for), never a row index: async rows (the directory, a look-up,
    // a picture) must not move Enter onto someone else. Unchosen → Enter's default (people.rs): the typed person; once
    // their look-up has failed, the fallback (your server only). The web's PeoplePicker does the same.
    property string chosenMxid: ""
    property string chosenText: ""
    readonly property int defaultSel: {
        var p = peopleCtl.pick
        if (p < 0 || p >= rows.length) return -1
        var r = rows[p]
        return (r.lookup === true && r.look !== "looking" && r.look !== "" && r.pickable !== true) ? peopleCtl.fallback : p
    }
    readonly property int sel: {
        if (chosenMxid.length > 0 && chosenText === field.text)
            for (var i = 0; i < rows.length; i++) if (String(rows[i].mxid) === chosenMxid && rows[i].member !== true) return i
        return defaultSel
    }
    function choose(i) {
        picker.chosenMxid = (i >= 0 && i < rows.length) ? String(rows[i].mxid) : ""
        picker.chosenText = field.text
    }

    spacing: Kirigami.Units.smallSpacing
    // one base size for the rows (the field's: ThTextField), the secondary lines a step smaller: name first and larger,
    // the address smaller and dimmer, as on the web (Kirigami's smallFont isn't always smaller than a Label's default)
    readonly property real baseSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale
    readonly property real smallSize: baseSize * 0.86

    /// a fresh picker (the dialog opened); `text` = what the field starts with
    function reset(text) {
        picker.chosenMxid = ""   // a new opening forgets the last one's choice
        picker.chosenText = ""
        peopleCtl.open(picker.roomId)
        field.text = text || ""
        peopleCtl.setText(field.text)
        if (field.text.length > 0) searchDebounce.restart()
        field.forceActiveFocus()
    }
    function pickSelected() { pickRow(picker.sel) }
    function pickRow(i) {
        if (i < 0 || i >= rows.length || !rows[i].pickable) return
        var id = String(rows[i].mxid)
        if (picker.keepPicking) {
            peopleCtl.picked(id)
            picker.chosenMxid = ""
            field.text = ""
            field.forceActiveFocus()
        }
        picker.picked(id)
    }
    function move(step) {
        if (rows.length === 0) return
        var i = picker.sel
        for (var n = 0; n < rows.length; n++) {
            i = (i + step + rows.length) % rows.length
            if (!rows[i].member) break
        }
        picker.choose(i)
        list.positionViewAtIndex(i, ListView.Contain)
    }

    PeopleController { id: peopleCtl }
    Timer { id: searchDebounce; interval: 250; onTriggered: peopleCtl.search() }

    ThTextField {
        id: field
        Layout.fillWidth: true
        placeholderText: picker.placeholder
        inputMethodHints: Qt.ImhNoAutoUppercase | Qt.ImhNoPredictiveText
        Accessible.name: "Name, username or address"
        Accessible.role: Accessible.ComboBox
        onTextChanged: { peopleCtl.setText(text); searchDebounce.restart() }
        onAccepted: picker.pickSelected()
        Keys.onDownPressed: picker.move(1)
        Keys.onUpPressed: picker.move(-1)
    }

    ListView {
        id: list
        Layout.fillWidth: true
        Layout.preferredHeight: picker.rows.length === 0 ? Kirigami.Units.gridUnit * 4
            : Math.min(Kirigami.Units.gridUnit * 17, contentHeight + 4)
        clip: true
        spacing: 2
        model: picker.rows
        Accessible.role: Accessible.List
        Accessible.name: "People"
        Controls.ScrollBar.vertical: FoxScrollBar { app: picker.app }
        delegate: Column {
            id: hit
            required property var modelData
            required property int index
            width: list.width
            readonly property bool ok: modelData.pickable === true
            readonly property string tag: modelData.member === true ? "already a member" : ""
            Controls.Label {
                visible: String(hit.modelData.head || "").length > 0
                text: String(hit.modelData.head || "")
                textFormat: Text.PlainText
                color: Th.inviteHintFg
                font.bold: true
                font.pointSize: picker.smallSize
                topPadding: hit.index === 0 ? 2 : 6
                bottomPadding: 2
                leftPadding: Kirigami.Units.smallSpacing
            }
            Item {
                width: parent.width
                height: Math.max(Kirigami.Units.gridUnit * 2.4, rowText.implicitHeight + 8)
                Accessible.role: Accessible.ListItem
                Accessible.name: hit.modelData.line + ", " + hit.modelData.mxid + (hit.tag ? ", " + hit.tag : "") + (hit.modelData.note ? ". " + hit.modelData.note : "")
                Accessible.selected: hit.index === picker.sel
                Accessible.onPressAction: picker.pickRow(hit.index)
                Rectangle {
                    anchors.fill: parent
                    radius: picker.app.radiusSm
                    color: hit.ok && hit.index === picker.sel ? Th.inviteResultBgHover : "transparent"   // one highlight: hovering selects (onEntered)
                    Behavior on color { ColorAnimation { duration: picker.app.dur } }
                }
                MouseArea {
                    id: hitMa
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: hit.ok ? Qt.PointingHandCursor : Qt.ArrowCursor
                    onEntered: if (hit.ok) picker.choose(hit.index)
                    onClicked: picker.pickRow(hit.index)
                }
                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: Kirigami.Units.smallSpacing
                    anchors.rightMargin: Kirigami.Units.smallSpacing
                    spacing: Kirigami.Units.smallSpacing
                    opacity: hit.modelData.member === true ? 0.55 : 1.0
                    // a row with a wrapped note: the picture beside its first line
                    Avatar { Layout.alignment: String(hit.modelData.note || "").length > 0 ? Qt.AlignTop : Qt.AlignVCenter; Layout.topMargin: String(hit.modelData.note || "").length > 0 ? 6 : 0; app: picker.app; size: 28; name: hit.modelData.name || hit.modelData.mxid; seed: hit.modelData.mxid || ""; imageSource: hit.modelData.avatar || "" }
                    ColumnLayout {
                        id: rowText
                        Layout.fillWidth: true
                        spacing: 0
                        // the name elides, the server never does: a long display name can't push it out of sight
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 0
                            // name first and larger, the address smaller and dimmer (the web's rows)
                            Controls.Label { textFormat: Text.PlainText; Layout.fillWidth: true; Layout.maximumWidth: implicitWidth; text: hit.modelData.name; color: Th.invitePopoverFg; font.bold: true; font.pointSize: picker.baseSize; elide: Text.ElideRight }
                            Controls.Label { visible: hit.modelData.server.length > 0; textFormat: Text.PlainText; text: " · " + hit.modelData.server; color: Th.inviteHintFg; font.pointSize: picker.baseSize }
                            Item { Layout.fillWidth: true }
                        }
                        Controls.Label { textFormat: Text.PlainText; Layout.fillWidth: true; text: hit.modelData.mxid || ""; color: Th.inviteHintFg; font.pointSize: picker.smallSize; elide: Text.ElideMiddle }
                        Controls.Label { visible: String(hit.modelData.note || "").length > 0; textFormat: Text.PlainText; Layout.fillWidth: true; Layout.maximumWidth: hit.width - 28 - Kirigami.Units.smallSpacing * 4; text: String(hit.modelData.note || ""); color: Th.inviteHintFg; font.pointSize: picker.smallSize; wrapMode: Text.WordWrap }
                    }
                    Controls.Label { visible: hit.tag.length > 0; textFormat: Text.PlainText; text: hit.tag; color: Th.inviteHintFg; font.pointSize: picker.smallSize; font.italic: true }
                    ThButton { visible: hit.modelData.anyway === true; app: picker.app; flat: true; text: picker.verb + " anyway"; onClicked: picker.pickRow(hit.index) }
                    ThIcon { visible: hit.ok && hit.modelData.anyway !== true; app: picker.app; name: picker.verb === "Message" ? "document-edit-symbolic" : "list-add-user"; width: 16; height: 16; color: Th.inviteResultIcon; hovered: hitMa.containsMouse }
                }
            }
        }
        Controls.Label {
            anchors.centerIn: parent
            width: parent.width - Kirigami.Units.largeSpacing * 2
            visible: picker.rows.length === 0
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            text: peopleCtl.kind === "empty" || peopleCtl.kind === ""
                  ? "Type a name, a username, or someone's full address."
                  : "No one by that name. Someone on another server: type their full address, like @name:their.server."
            color: Th.inviteHintFg
            font.pointSize: picker.smallSize
        }
    }

    RowLayout {
        visible: peopleCtl.me.length > 0
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.smallSpacing
        spacing: Kirigami.Units.smallSpacing
        Controls.Label { text: "Your address:"; color: Th.inviteHintFg; font.pointSize: picker.smallSize }
        Controls.Label { textFormat: Text.PlainText; Layout.fillWidth: true; text: peopleCtl.me; color: Th.inviteHintFg; font.pointSize: picker.smallSize; elide: Text.ElideMiddle }
        ThButton {
            app: picker.app
            flat: true
            icon.name: "edit-copy-symbolic"
            text: "Copy my address"
            onClicked: { picker.app.copyToClipboard(peopleCtl.me); if (picker.app.showPassiveNotification) picker.app.showPassiveNotification("Copied your address", 2000) }
        }
    }
}
