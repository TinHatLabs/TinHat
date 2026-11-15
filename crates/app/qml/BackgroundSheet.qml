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
    readonly property bool roomEncrypted: { try { return !!JSON.parse(String(roomCtl.roomSummaryJson)).encrypted } catch (e) { return true } }
    readonly property string mode: ambCtl ? ambCtl.themeMode(String(Th.timelineBg)) : "dark"
    readonly property var presets: ambCtl ? JSON.parse(ambCtl.presetsJson()) : []
    readonly property var effects: ambCtl ? JSON.parse(ambCtl.effectsJson()) : []
    property string chosen: "none"      // "none", "game" (sun-gamebg), a preset id, or "image" (your own image over a preset fallback)
    readonly property var gw: app ? JSON.parse(app.gameArtWords) : ({})
    property string fallback: "rain"     // kind image: the scene shown to anyone who can't show the image
    property string picked: ""           // a newly picked image file (posted on Apply)
    property string effect: ""          // "" = the preset's own

    onAboutToShow: {
        var raw = roomCtl ? String(roomCtl.backgroundJson) : ""
        var b = { kind: "unset", preset: null, effect: "none" }
        if (raw.length > 0) { var o = JSON.parse(raw); b = JSON.parse(ambCtl.parse(JSON.stringify(o.content), o.sender)) }
        dlg.picked = ""
        dlg.fallback = b.preset ? b.preset : "rain"
        // an UNSET room already shows game art by itself, so the sheet opens on "Match the game" (sun-gamebg)
        dlg.chosen = b.kind === "game" || b.kind === "unset" ? "game" : (b.kind === "image" ? "image" : (b.kind === "preset" && b.preset ? b.preset : "none"))
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
    onClosed: { gamePreview.setHidden(true); gamePreview.open("") }   // asleep until the sheet opens again

    component Swatch: Item {
        id: sw
        property string pid: ""
        property string label: ""
        readonly property var look: pid.length > 0 && dlg.ambCtl ? JSON.parse(dlg.ambCtl.lookJson(pid, dlg.mode)) : null
        readonly property bool selected: dlg.chosen === (pid.length > 0 ? pid : "none")
        implicitWidth: 112; implicitHeight: 92
        Accessible.role: Accessible.RadioButton
        Accessible.name: label
        Accessible.checked: selected
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
        Controls.Label {
            textFormat: Text.PlainText
            anchors.horizontalCenter: parent.horizontalCenter
            y: 72
            text: sw.label
            font.pointSize: Kirigami.Theme.smallFont.pointSize
            color: Th.settingsFg
        }
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
        Controls.Label {
            Layout.fillWidth: true
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
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
                Controls.Label { textFormat: Text.PlainText; anchors.horizontalCenter: parent.horizontalCenter; y: 72; text: dlg.gw.matchGame || ""; font.pointSize: Kirigami.Theme.smallFont.pointSize; color: Th.settingsFg }
                MouseArea {
                    anchors.fill: parent
                    enabled: dlg.can
                    cursorShape: dlg.can ? Qt.PointingHandCursor : Qt.ArrowCursor
                    onClicked: dlg.chosen = "game"
                }
                Accessible.role: Accessible.Button
                Accessible.name: dlg.gw.matchGame || ""
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
                Controls.Label { textFormat: Text.PlainText; anchors.horizontalCenter: parent.horizontalCenter; y: 72; text: dlg.w("customLabel"); font.pointSize: Kirigami.Theme.smallFont.pointSize; color: Th.settingsFg }
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
        nameFilters: ["Images (*.png *.jpg *.jpeg *.gif *.webp)"]
        onAccepted: { dlg.picked = String(selectedFile); dlg.chosen = "image" }
    }

    footer: Controls.DialogButtonBox {
        Controls.Button {
            visible: dlg.can
            enabled: dlg.plan !== "blocked"
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
