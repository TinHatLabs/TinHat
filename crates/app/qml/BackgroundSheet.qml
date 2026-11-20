// BackgroundSheet.qml — room menu → "Chat background…" (the web's ambience/BackgroundSheet.tsx, same words from
// testvectors/ambience/words.json): the built-in scenes as live swatches, None, and the effect. One choice for everyone
// in the room (room state, docs/AMBIENCE.md); people who can't change it see it read-only.
import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import QtQuick.Controls as Controls
import QtQuick.Dialogs
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Controls.Dialog {
    id: dlg
    property var app
    property var roomCtl: null
    readonly property var ambCtl: app ? app.ambience : null
    function w(k) { return ambCtl ? ambCtl.word(k) : "" }

    title: LinkGate.plain(w("title"))
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    readonly property real boxWidth: Math.min(560, (parent ? parent.width : 560) - Kirigami.Units.gridUnit * 2)
    implicitWidth: dlg.boxWidth

    readonly property bool can: !!roomCtl && roomCtl.canSetBackground
    // bgsheet-1220 (12.2.0, the web's sheetPerm.ts): false while the room's power levels are still being read (a room just
    // opened): the sheet is then NEUTRAL (nothing pickable, no "only moderators" note, nothing greyed), never the
    // read-only form; it turns editable or read-only by itself when they arrive
    readonly property bool known: !!roomCtl && roomCtl.canSetBackgroundKnown !== false
    // 12.1.17: people who can't change the background see the other choices greyed (their clicks were already ignored);
    // the room's current choice stays clear. The web: BackgroundSheet.module.css .locked (same 0.5).
    function tileOpacity(selected) { return dlg.can || !dlg.known || selected ? 1.0 : 0.5 }
    readonly property bool roomEncrypted: { try { return !!JSON.parse(String(roomCtl.roomSummaryJson)).encrypted } catch (e) { return true } }
    readonly property string mode: ambCtl ? ambCtl.themeMode(String(Th.timelineBg)) : "dark"
    readonly property var presets: ambCtl ? JSON.parse(ambCtl.presetsJson()) : []
    readonly property var effects: ambCtl ? JSON.parse(ambCtl.effectsJson()) : []
    property string chosen: "none"      // "none", "game" (sun-gamebg), a preset id, or "image" (your own image over a preset fallback)
    readonly property var gw: app ? JSON.parse(app.gameArtWords) : ({})
    property string fallback: "rain"     // kind image: the scene shown to anyone who can't show the image
    property string picked: ""           // a newly picked image file (posted on Apply)
    property bool formatNote: false      // 12.1.17: the last pick was a picture this device can't read (customFormat)
    // a scene picked while a picture is still being read wins over it
    onChosenChanged: if (chosen !== "image") { pickProbe.source = ""; formatNote = false }
    property string effect: ""          // "" = the preset's own

    // the sheet's selection for a room background (an UNSET room already shows game art by itself, so it is "Match the
    // game": sun-gamebg)
    function choiceOf(b) { return b.kind === "game" || b.kind === "unset" ? "game" : (b.kind === "image" ? "image" : (b.kind === "preset" && b.preset ? b.preset : "none")) }
    // 12.1.17 (review round): someone who can't change it always sees the room's CURRENT choice clear, also when another
    // member changes it while the sheet is open
    function showCurrent() {
        dlg.chosen = dlg.choiceOf(dlg.current)
        if (dlg.current.preset) dlg.fallback = dlg.current.preset
        dlg.effect = (dlg.current.kind === "preset" || dlg.current.kind === "image") ? dlg.current.effect : ""
    }
    onCurrentChanged: if (!dlg.can && dlg.visible) dlg.showCurrent()
    onCanChanged: if (!dlg.can && dlg.visible) dlg.showCurrent()   // losing the permission drops an unapplied pick
    onAboutToShow: {
        var raw = roomCtl ? String(roomCtl.backgroundJson) : ""
        var b = { kind: "unset", preset: null, effect: "none" }
        if (raw.length > 0) { var o = JSON.parse(raw); b = JSON.parse(ambCtl.parse(JSON.stringify(o.content), o.sender)) }
        dlg.picked = ""
        dlg.formatNote = false
        pickProbe.source = ""   // a pick still being read in an earlier session changes nothing here (review round)
        dlg.fallback = b.preset ? b.preset : "rain"
        dlg.chosen = dlg.choiceOf(b)
        gamePreview.setInk(String(Th.timelineBg), "[]", "dark")   // the preview shows the art itself (no text over it)
        gamePreview.setHidden(false)
        gamePreview.setPrefs(true, true)
        gamePreview.setBackground(JSON.stringify({ v: 1, kind: "game" }))
        gamePreview.open(roomCtl ? String(roomCtl.roomId) : "")
        dlg.effect = (b.kind === "preset" || b.kind === "image") ? b.effect : ""
    }
    function ownEffect() {
        var id = chosen === "image" ? fallback : chosen
        for (var i = 0; i < presets.length; i++) if (presets[i].id === id) return presets[i].effect
        return "none"
    }
    readonly property string me: app && app.mUserId ? String(app.mUserId) : ""
    readonly property var current: {
        var raw = roomCtl ? String(roomCtl.backgroundJson) : ""
        if (!ambCtl || raw.length === 0) return ({ kind: "unset", preset: null, effect: "none", image: null, by: "" })
        var o = JSON.parse(raw)
        return JSON.parse(ambCtl.parse(JSON.stringify(o.content), o.sender))
    }
    // what Apply does (shared rule: never republish someone else's image under my name; nothing changed = no write)
    // sun-gamebg: "Match the game" needs no write where the room already shows game art (game, or nothing set)
    readonly property var gamePlan: JSON.parse(gamePreview.matchGamePlanJson(current.kind, gamePreview.artUrl.length > 0))
    readonly property string plan: chosen === "game" ? gamePlan.plan : ambCtl ? ambCtl.applyPlan(JSON.stringify(current), chosen === "none" ? "none" : chosen === "image" ? "image" : "preset",
                                                        chosen === "image" ? fallback : (chosen === "none" ? "" : chosen),
                                                        effect.length > 0 ? effect : "", picked.length > 0, me) : "write"
    function apply() {
        if (!can || !ambCtl) return
        if (plan === "skip") { dlg.close(); return }
        if (plan === "blocked") return
        var eff = effect === ownEffect() ? "" : effect
        if (chosen === "image") {
            if (picked.length > 0) roomCtl.postBackgroundImage(picked, fallback, eff)   // posts the image, then sets it
            else {
                var raw = String(roomCtl.backgroundJson)
                var cur = raw.length > 0 ? JSON.parse(ambCtl.parse(JSON.stringify(JSON.parse(raw).content), JSON.parse(raw).sender)) : null
                if (cur && cur.image) roomCtl.setBackground(ambCtl.content("image", fallback, eff, cur.image))
            }
        } else if (chosen === "game") roomCtl.setBackground(JSON.stringify(gamePlan.content))   // sun-gamebg (plan "write")
        else roomCtl.setBackground(chosen === "none" ? ambCtl.content("none", "", "", "") : ambCtl.content("preset", chosen, eff, ""))
        dlg.close()
    }

    // sun-gamebg: the "Match the game" swatch's preview (stops following the room when the sheet closes)
    GameArtController { id: gamePreview }
    onClosed: { gamePreview.setHidden(true); gamePreview.open(""); pickProbe.source = "" }   // asleep until the sheet opens again; a pick still being read is dropped

    // 12.1.18 review #2: a tile's caption keeps to the tile's width (centred, up to two lines, elided) and the tile grows
    // with it (its height = 72 + the caption's; a one-line caption always ended at the tile's bottom), so at a big text size a long
    // caption ("Match the game") stays inside the tile and its click area. At 100 % the tile is 112 x 92 as before.
    component TileCaption: Controls.Label {
        x: 2; y: 72; width: parent.width - 4
        textFormat: Text.PlainText
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap; maximumLineCount: 2; elide: Text.ElideRight
        font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
        color: Th.settingsFg
    }

    component Swatch: Item {
        id: sw
        property string pid: ""
        property string label: ""
        readonly property var look: pid.length > 0 && dlg.ambCtl ? JSON.parse(dlg.ambCtl.lookJson(pid, dlg.mode)) : null
        readonly property bool selected: dlg.chosen === (pid.length > 0 ? pid : "none")
        implicitWidth: 112; implicitHeight: 92
        opacity: dlg.tileOpacity(selected)
        Accessible.role: Accessible.RadioButton
        Accessible.name: label
        Accessible.checked: selected
        height: Math.max(92, 72 + swCap.implicitHeight)   // 12.1.18 review #2: grows with its caption (92 at 100 %)
        Rectangle {
            id: scene
            x: 4; y: 4; width: parent.width - 8; height: 64; radius: 8
            clip: true
            color: Th.timelineBg
            border.width: 1; border.color: Th.islandBorder
            gradient: sw.look ? grad : null
            Gradient {
                id: grad
                GradientStop { position: sw.look ? sw.look.stops[0][0] : 0; color: sw.look ? sw.look.stops[0][1] : "transparent" }
                GradientStop { position: sw.look ? sw.look.stops[1][0] : 0.5; color: sw.look ? sw.look.stops[1][1] : "transparent" }
                GradientStop { position: sw.look ? sw.look.stops[2][0] : 1; color: sw.look ? sw.look.stops[2][1] : "transparent" }
            }
        }
        Rectangle {
            x: 1; y: 1; width: parent.width - 2; height: 70; radius: 10
            color: "transparent"
            border.width: 2
            border.color: sw.selected || swMa.activeFocus ? Th.accentBase : "transparent"
        }
        TileCaption { id: swCap; text: sw.label }
        MouseArea {
            id: swMa
            anchors.fill: parent
            enabled: dlg.can
            cursorShape: dlg.can ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: { dlg.chosen = sw.pid.length > 0 ? sw.pid : "none"; if (sw.pid.length > 0) dlg.fallback = sw.pid }
        }
    }

    ColumnLayout {
        width: dlg.boxWidth - dlg.leftPadding - dlg.rightPadding
        spacing: 12
        // the pick's one-time read (12.1.17 picker). It lives INSIDE the layout (hidden = no space): as a second content
        // child of the Dialog it collapsed the sheet's content height to 0, the tiles drew outside the sheet and every
        // press on them closed it (a Popup sizes itself from its ONE content child).
        Image {
            id: pickProbe
            visible: false; asynchronous: true; cache: false
            sourceSize.width: 64; sourceSize.height: 64
            onStatusChanged: {
                if (String(source).length === 0) return
                if (status === Image.Ready) { dlg.formatNote = false; dlg.picked = String(source); dlg.chosen = "image"; source = "" }
                else if (status === Image.Error) { dlg.formatNote = true; source = "" }
            }
        }
        Controls.Label {
            Layout.fillWidth: true
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            visible: dlg.can || dlg.known
            text: dlg.can ? dlg.w("forEveryone") : dlg.w("readOnly")
            color: Th.settingsHintFg
        }
        Controls.Label {
            Layout.fillWidth: true
            visible: !!dlg.app && !dlg.app.mChatBackgrounds
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            text: dlg.w("offHere")
            color: Th.settingsHintFg
        }
        Flow {
            Layout.fillWidth: true
            spacing: 10
            Swatch { pid: ""; label: dlg.w("none") }
            // sun-gamebg: "Match the game" — its preview is the art of the game played here now (plain when nobody plays)
            Item {
                implicitWidth: 112; implicitHeight: 92
                opacity: dlg.tileOpacity(dlg.chosen === "game")
                Rectangle {
                    x: 4; y: 4; width: parent.width - 8; height: 64; radius: 8; clip: true
                    color: Th.timelineBg; border.width: 1; border.color: Th.islandBorder
                    Image {
                        anchors.fill: parent; anchors.margins: 1
                        fillMode: Image.PreserveAspectCrop; asynchronous: true; sourceSize.width: 224
                        source: gamePreview.artUrl
                    }
                }
                Rectangle { x: 1; y: 1; width: parent.width - 2; height: 70; radius: 10; color: "transparent"; border.width: 2; border.color: dlg.chosen === "game" ? Th.accentBase : "transparent" }
                TileCaption { id: gameCap; text: dlg.gw.matchGame || "" }
                MouseArea {
                    anchors.fill: parent
                    enabled: dlg.can
                    cursorShape: dlg.can ? Qt.PointingHandCursor : Qt.ArrowCursor
                    onClicked: dlg.chosen = "game"
                }
                Accessible.role: Accessible.Button
                Accessible.name: dlg.gw.matchGame || ""
                height: Math.max(92, 72 + gameCap.implicitHeight)   // 12.1.18 review #2: grows with its caption
            }
            Repeater {
                model: dlg.presets
                delegate: Swatch { required property var modelData; pid: modelData.id; label: modelData.name }
            }
            // your own image: the picked file (or the room's current one) over its fallback scene
            Item {
                visible: dlg.chosen === "image"
                implicitWidth: 112; implicitHeight: 92
                Rectangle {
                    x: 4; y: 4; width: parent.width - 8; height: 64; radius: 8; clip: true
                    color: Th.timelineBg; border.width: 1; border.color: Th.islandBorder
                    Image {
                        anchors.fill: parent; anchors.margins: 1
                        fillMode: Image.PreserveAspectCrop; asynchronous: true; sourceSize.width: 224
                        source: dlg.picked.length > 0 ? dlg.picked : (dlg.roomCtl ? String(dlg.roomCtl.backgroundImage) : "")
                    }
                }
                Rectangle { x: 1; y: 1; width: parent.width - 2; height: 70; radius: 10; color: "transparent"; border.width: 2; border.color: Th.accentBase }
                TileCaption { id: imageCap; text: dlg.w("customLabel") }
                height: Math.max(92, 72 + imageCap.implicitHeight)   // 12.1.18 review #2: grows with its caption
            }
        }
        Controls.Button {
            visible: dlg.can
            text: LinkGate.plain(dlg.w("customImage"))
            icon.name: "insert-image-symbolic"
            onClicked: imagePicker.open()
        }
        Controls.Label {
            Layout.fillWidth: true
            visible: dlg.formatNote
            textFormat: Text.PlainText; wrapMode: Text.Wrap
            text: dlg.w("customFormat")
            color: Th.settingsErrorFg
            Accessible.role: Accessible.AlertMessage
        }
        Controls.Label {
            Layout.fillWidth: true
            visible: dlg.plan === "blocked"
            textFormat: Text.PlainText; wrapMode: Text.Wrap
            text: dlg.w("otherImage")
            color: Th.settingsHintFg
        }
        Controls.Label {
            Layout.fillWidth: true
            visible: dlg.chosen === "image"
            textFormat: Text.PlainText; wrapMode: Text.Wrap
            text: dlg.w("customPosted")
            color: Th.settingsHintFg
        }
        Controls.Label {
            Layout.fillWidth: true
            visible: dlg.chosen === "image" && !dlg.roomEncrypted
            textFormat: Text.PlainText; wrapMode: Text.Wrap
            text: dlg.w("customPlainRoom")
            color: Th.settingsErrorFg
        }
        Controls.Label {
            Layout.fillWidth: true
            visible: dlg.chosen === "game"
            textFormat: Text.PlainText; wrapMode: Text.Wrap
            text: dlg.gw.matchGameHelp || ""
            color: Th.settingsHintFg
        }
        RowLayout {
            visible: dlg.chosen !== "none" && dlg.chosen !== "game"
            spacing: 12
            Controls.Label { textFormat: Text.PlainText; text: dlg.w("effect"); color: Th.settingsFg }
            ThComboBox {
                id: effCombo
                enabled: dlg.can
                readonly property var rows: {
                    var own = dlg.ownEffect(), name = own
                    for (var i = 0; i < dlg.effects.length; i++) if (dlg.effects[i].id === own) name = dlg.effects[i].name
                    var r = [{ id: "", text: dlg.w("effectOwn") + " (" + name + ")" }]
                    for (var j = 0; j < dlg.effects.length; j++) r.push({ id: dlg.effects[j].id, text: dlg.effects[j].name })
                    return r
                }
                model: rows
                textRole: "text"
                currentIndex: { for (var i = 1; i < rows.length; i++) if (rows[i].id === dlg.effect) return i; return 0 }
                onActivated: (i) => dlg.effect = rows[i].id
                Accessible.name: dlg.w("effect")
            }
        }
    }

    FileDialog {
        id: imagePicker
        title: LinkGate.plain(dlg.w("customImage"))
        // 12.1.17: the shared formats (ambience IMAGE_PICK_EXTS = the web's accept list), HEIC / HEIF too; the pick is
        // read once first: a picture this device can't read says so (customFormat) instead of failing at Apply
        nameFilters: ["Images (*.png *.jpg *.jpeg *.gif *.webp *.avif *.heic *.heif)"]
        onAccepted: { pickProbe.source = ""; pickProbe.source = selectedFile }
    }

    footer: Controls.DialogButtonBox {
        Controls.Button {
            visible: dlg.can
            enabled: dlg.plan !== "blocked" && pickProbe.status !== Image.Loading   // Apply waits while a pick is read
            text: LinkGate.plain(dlg.w("apply"))
            Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.ActionRole
            onClicked: dlg.apply()
        }
        Controls.Button {
            text: dlg.can ? "Cancel" : "Close"
            Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.RejectRole
        }
    }
}
