// "My games": would-play / favorite / definitely-not lists, Flexible, catalog search + Steam import.
// Writes ONE state event (dev.tinhat.games.prefs, state_key = me) through ctl.savePrefs().
import QtQuick
import QtQuick.Layouts
import QtQuick.Templates as T
import dev.tinhat.app

Rectangle {
    id: ed
    required property var app
    required property var ctl
    property string catalogBase: ""
    // selfhost: the catalogue search and the Steam import only where the server offers them (hidden, never dimmed)
    readonly property bool catalogOn: !app || !app.serverOn || app.serverOn("games")
    readonly property bool steamOn: !app || !app.serverOn || app.serverOn("steam")
    radius: app.radiusLg
    color: Th.reelEditorBg
    border.color: Th.reelEditorBorder
    implicitHeight: col.implicitHeight + 28

    // Local copy of my prefs; Save writes it back.
    property var prefs: ({ wouldPlay: [], favorites: [], definitelyNot: [], flexible: false })
    property var nameCache: ({})          // id -> {name, art}
    property bool dirty: false
    function reload() {
        try { var p = JSON.parse(ctl.myPrefsJson || "{}"); prefs = { wouldPlay: p.wouldPlay || [], favorites: p.favorites || [], definitelyNot: p.definitelyNot || [], flexible: !!p.flexible, names: p.names || {} }; if (p.names) nameCache = Object.assign({}, nameCache, p.names) } catch (e) {}
        dirty = false
    }
    Component.onCompleted: reload()
    Connections { target: ed.ctl; function onMyPrefsJsonChanged() { if (!ed.dirty) ed.reload() } }
    // 10.1 (TH-REEL-04): names round-trip through the prefs event now (as `{id: "Name"}`; the older
    // `{id: {name}}` shape is still read), so the list reads as names after a restart.
    function nameOf(id) {
        var e = nameCache[id]
        var n = !e ? "" : (typeof e === "string" ? e : (e.name || ""))
        if (n.length > 0) return n
        return id.indexOf("steam:") === 0 ? "Steam app " + id.slice(6) : id.replace(/^[a-z]+:/, "")
    }
    function art(id) { return catalogBase + "/art/" + encodeURIComponent(id) + "/portrait" }
    function setList(key, list) { var p = Object.assign({}, prefs); p[key] = list; prefs = p; dirty = true }
    function addTo(key, g) {
        nameCache = Object.assign({}, nameCache, (function () { var o = {}; o[g.id] = { name: g.name }; return o })())
        var others = ["wouldPlay", "definitelyNot"].filter(function (k) { return k !== key })
        var p = Object.assign({}, prefs)
        for (var i = 0; i < others.length; i++) p[others[i]] = (p[others[i]] || []).filter(function (x) { return x !== g.id })
        if ((p[key] || []).indexOf(g.id) === -1) p[key] = (p[key] || []).concat([g.id])
        if (key === "definitelyNot") p.favorites = (p.favorites || []).filter(function (x) { return x !== g.id })
        prefs = p; dirty = true
    }
    function remove(id) {
        var p = Object.assign({}, prefs)
        for (var k in { wouldPlay: 1, favorites: 1, definitelyNot: 1 }) p[k] = (p[k] || []).filter(function (x) { return x !== id })
        prefs = p; dirty = true
    }
    function toggleFavorite(id) {
        var p = Object.assign({}, prefs)
        p.favorites = (p.favorites || []).indexOf(id) !== -1 ? [] : [id]
        prefs = p; dirty = true
    }
    function save() { ctl.savePrefs(JSON.stringify({ wouldPlay: prefs.wouldPlay, favorites: prefs.favorites, definitelyNot: prefs.definitelyNot, flexible: prefs.flexible, names: nameCache })); dirty = false }
    // searchJson is already the ARRAY (TH-REEL-01); an object with `items` is tolerated too.
    readonly property var results: { try { var r = JSON.parse(ctl.searchJson || "[]"); return Array.isArray(r) ? r : (r.items || []) } catch (e) { return [] } }

    ColumnLayout {
        id: col
        x: 14; y: 14
        width: parent.width - 28
        spacing: 10
        RowLayout {
            Layout.fillWidth: true
            Text { text: "My games"; color: Th.reelFg; font.pixelSize: 16; font.weight: Font.DemiBold; Layout.fillWidth: true }
            Text { visible: ed.dirty; text: "unsaved"; color: Th.reelWarningFg; font.pixelSize: 11 }
            ReelButton { text: "Save"; primary: true; enabled: ed.dirty && !ctl.busy; onClicked: ed.save(); radius: app.radius }
        }
        // search
        Rectangle {
            Layout.fillWidth: true
            height: 36; radius: app.radius; color: Th.reelFieldBg; border.color: searchField.activeFocus ? Th.reelFieldBorderFocus : Th.reelFieldBorder
            T.TextField {
                id: searchField
                anchors.fill: parent; anchors.leftMargin: 10; anchors.rightMargin: 10
                color: Th.reelFieldFg; font.pixelSize: 13; verticalAlignment: TextInput.AlignVCenter
                placeholderText: ed.catalogOn ? "Add a game — type its name" : "Add a game — type its name, then press Enter"
                Text { textFormat: Text.PlainText; visible: !searchField.text.length && !searchField.activeFocus; text: searchField.placeholderText; color: Th.reelFieldPlaceholder; font.pixelSize: 13; anchors.verticalCenter: parent.verticalCenter }
                onTextEdited: searchDebounce.restart()
                // "Press Enter to add it by name" (the copy below) — now true (TH-REEL-01 / TH-GAP-19).
                onAccepted: if (text.trim().length >= 2 && ed.results.length === 0) ed.ctl.addCustomGame(text.trim())
                Timer { id: searchDebounce; interval: 220; onTriggered: if (ed.catalogOn && searchField.text.trim().length >= 2) ed.ctl.searchCatalog(searchField.text.trim()) }
            }
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            visible: searchField.text.trim().length >= 2 && ed.results.length > 0
            Repeater {
                model: ed.results.slice(0, 10)
                delegate: Rectangle {
                    required property var modelData
                    width: 200; height: 54; radius: app.radiusSm; color: Th.reelSearchHitBg; border.color: Th.reelSearchHitBorder
                    RowLayout {
                        anchors.fill: parent; anchors.margins: 6; spacing: 8
                        Image { source: ed.art(modelData.id); Layout.preferredWidth: 28; Layout.preferredHeight: 42; fillMode: Image.PreserveAspectCrop; asynchronous: true }
                        ColumnLayout {
                            Layout.fillWidth: true; spacing: 1
                            Text { textFormat: Text.PlainText; text: modelData.name; color: Th.reelFg; font.pixelSize: 12; elide: Text.ElideRight; Layout.fillWidth: true }
                            Text { textFormat: Text.PlainText; text: (modelData.year ? modelData.year + " · " : "") + (modelData.flags === null ? "checking…" : (modelData.flags && (modelData.flags.multiplayer || modelData.flags.coop || modelData.flags.onlinePvp || modelData.flags.onlineCoop)) ? "multiplayer" : "single-player"); color: Th.reelMetaFg; font.pixelSize: 10 }
                        }
                        ReelButton { text: "+"; implicitWidth: 30; implicitHeight: 26; onClicked: ed.addTo("wouldPlay", modelData); radius: app.radiusSm; hint: "Add to would-play" }
                        ReelButton { text: "✕"; implicitWidth: 30; implicitHeight: 26; quiet: true; onClicked: ed.addTo("definitelyNot", modelData); radius: app.radiusSm; hint: "Definitely not" }
                    }
                }
            }
        }
        Text {
            visible: searchField.text.trim().length >= 2 && ed.results.length === 0
            text: ed.catalogOn ? "Nothing yet — Steam takes a moment. Not on Steam? Press Enter to add it by name." : "Press Enter to add it by name."
            color: Th.reelMetaFg; font.pixelSize: 11; wrapMode: Text.WordWrap; Layout.fillWidth: true
        }
        // lists
        GridLayout {
            Layout.fillWidth: true
            columns: 3
            columnSpacing: 12
            rowSpacing: 6
            Repeater {
                model: [ { key: "wouldPlay", title: "Would play", hint: "Games you own and would play." },
                         { key: "favorites", title: "Favorite", hint: "One game that gets the bigger share of your vote." },
                         { key: "definitelyNot", title: "Definitely not", hint: "A hard veto while you’re here." } ]
                delegate: ColumnLayout {
                    id: colDelegate
                    required property var modelData
                    // 10.1 (TH-REEL-02): the row delegate used `parent.parent.modelData.key`, which
                    // resolved to the GridLayout (no modelData) — so the ★ never rendered for anyone.
                    readonly property string listKey: modelData.key
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignTop
                    spacing: 4
                    Text { textFormat: Text.PlainText; text: modelData.title; color: Th.reelSectionFg; font.pixelSize: 11; font.letterSpacing: 1; font.capitalization: Font.AllUppercase }
                    Text { textFormat: Text.PlainText; text: modelData.hint; color: Th.reelMetaFg; font.pixelSize: 10; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                    Repeater {
                        model: ed.prefs[modelData.key] || []
                        delegate: Rectangle {
                            required property var modelData
                            readonly property string listKey: colDelegate.listKey
                            Layout.fillWidth: true
                            height: 34; radius: app.radiusSm; color: Th.reelListRowBg; border.color: (ed.prefs.favorites || []).indexOf(modelData) !== -1 ? Th.reelListRowFavoriteBorder : Th.reelListRowBorder
                            RowLayout {
                                anchors.fill: parent; anchors.margins: 4; spacing: 6
                                Image { source: ed.art(modelData); Layout.preferredWidth: 18; Layout.preferredHeight: 26; fillMode: Image.PreserveAspectCrop; asynchronous: true }
                                Text { textFormat: Text.PlainText; text: ed.nameOf(modelData); color: Th.reelFg; font.pixelSize: 12; elide: Text.ElideRight; Layout.fillWidth: true }
                                ReelButton { visible: listKey === "wouldPlay"; text: (ed.prefs.favorites || []).indexOf(modelData) !== -1 ? "★" : "☆"; quiet: true; implicitWidth: 26; implicitHeight: 24; onClicked: ed.toggleFavorite(modelData); radius: app.radiusSm; hint: "Favorite" }
                                ReelButton { text: "–"; quiet: true; implicitWidth: 26; implicitHeight: 24; onClicked: ed.remove(modelData); radius: app.radiusSm; hint: "Remove" }
                            }
                        }
                    }
                    Text { visible: !(ed.prefs[modelData.key] || []).length; text: "—"; color: Th.reelMetaFg; font.pixelSize: 12 }
                }
            }
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 10
            T.CheckBox {
                id: flex
                checked: ed.prefs.flexible === true
                onToggled: { var p = Object.assign({}, ed.prefs); p.flexible = checked; ed.prefs = p; ed.dirty = true }
                implicitHeight: 24
                indicator: Rectangle { width: 18; height: 18; radius: 4; anchors.verticalCenter: parent.verticalCenter; color: flex.checked ? Th.reelCheckboxBgChecked : Th.reelCheckboxBg; border.color: flex.checked ? Th.reelCheckboxBorderChecked : Th.reelCheckboxBorder
                                       Text { anchors.centerIn: parent; text: "✓"; visible: flex.checked; color: Th.reelCheckboxFg; font.pixelSize: 12 } }
                contentItem: Text { leftPadding: 24; text: "I’ll play whatever wins. Your games still count toward what everyone owns."; color: Th.reelFg; font.pixelSize: 12; verticalAlignment: Text.AlignVCenter; wrapMode: Text.WordWrap }
                Layout.fillWidth: true
            }
        }
        // Steam import
        RowLayout {
            visible: ed.steamOn
            Layout.fillWidth: true
            spacing: 8
            Rectangle {
                Layout.fillWidth: true
                height: 34; radius: app.radius; color: Th.reelFieldBg; border.color: steamField.activeFocus ? Th.reelFieldBorderFocus : Th.reelFieldBorder
                T.TextField {
                    id: steamField
                    anchors.fill: parent; anchors.leftMargin: 10; anchors.rightMargin: 10
                    color: Th.reelFieldFg; font.pixelSize: 12; verticalAlignment: TextInput.AlignVCenter
                    Text { visible: !steamField.text.length && !steamField.activeFocus; text: "Steam profile link (optional) — imports your multiplayer games"; color: Th.reelFieldPlaceholder; font.pixelSize: 12; anchors.verticalCenter: parent.verticalCenter }
                }
            }
            ReelButton { text: "Import"; enabled: steamField.text.trim().length > 2 && !ctl.busy; onClicked: ed.ctl.importSteamLibrary(steamField.text.trim(), true); radius: app.radius }
        }
        RowLayout {
            id: impRow
            Layout.fillWidth: true
            spacing: 8
            readonly property var imp: { try { return JSON.parse(ed.ctl.importJson || "{}") } catch (e) { return {} } }
            visible: imp.error !== undefined || (imp.items !== undefined)
            Text {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                text: impRow.imp.error ? impRow.imp.error : ((impRow.imp.items || []).length + " games found" + (impRow.imp.pending ? " (" + impRow.imp.pending + " still being checked)" : "") + " — tick the ones you’d play:")
                color: impRow.imp.error ? Th.reelErrorFg : Th.reelMetaFg; font.pixelSize: 11; wrapMode: Text.WordWrap; Layout.fillWidth: true
            }
            // 10.1 (TH-REEL-15): nothing is pre-ticked (the import is a suggestion, not a decision),
            // but the alternative used to be sixty clicks — one button takes the lot.
            ReelButton {
                visible: !impRow.imp.error && (impRow.imp.items || []).length > 0
                text: "Tick all"
                implicitHeight: 24
                radius: app.radiusSm
                onClicked: { var items = (impRow.imp.items || []).slice(0, 60); for (var i = 0; i < items.length; i++) ed.addTo("wouldPlay", items[i]) }
            }
        }
        Flow {
            Layout.fillWidth: true
            spacing: 6
            Repeater {
                model: { try { return (JSON.parse(ed.ctl.importJson || "{}").items || []).slice(0, 60) } catch (e) { return [] } }
                delegate: ReelButton {
                    required property var modelData
                    text: modelData.name
                    checked: (ed.prefs.wouldPlay || []).indexOf(modelData.id) !== -1
                    implicitHeight: 26
                    onClicked: checked ? ed.remove(modelData.id) : ed.addTo("wouldPlay", modelData)
                    radius: app.radiusSm
                }
            }
        }
        Text { text: "The group sees your lists, favorite, vetoes and the Flexible flag. It never sees your Steam ID, playtime or full library; the import is forgotten the moment it is shown."; color: Th.reelMetaFg; font.pixelSize: 10; wrapMode: Text.WordWrap; Layout.fillWidth: true }
    }
}
