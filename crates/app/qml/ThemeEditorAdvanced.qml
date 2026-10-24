// ThemeEditorAdvanced.qml — the theme editor's Advanced mode (THEMES-UI.md §13.8; the web's Advanced.tsx): every
// palette slot, role and element colour, grouped (the palette, the eight role groups, every token area under its
// section — editor-labels.json, grouped in Rust: tinhat_theme::labels), searchable (the shared searchGroups rule), a
// Changed filter, and the Pick tool's "Picked" rows first. One virtualized list of fixed-height items: section
// headings 28, group headers 32 (chevron, label · count, "‹k› changed"), rows 40 (swatch, label over id, the Pick
// badge, the warning glyph of a shown readability issue, the changed dot). A row opens the colour popover.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the settings.* / settings.themeEditor.* tokens

Item {
    id: adv
    property var app: null
    property var panel: null

    readonly property var st: adv.panel ? adv.panel.st : ({})
    readonly property var doc: adv.st.doc || null
    readonly property var base: adv.st.base || null
    readonly property var ctl: adv.panel ? adv.panel.ctl : null
    readonly property bool hasPal: adv.doc ? ThemeEditorLib.hasPalette(adv.doc) : false
    readonly property string monoFamily: Th.fontMono   // the code font (type.code, else the platform monospace)
    readonly property var badges: ({ text: "Text", fill: "Fill", border: "Border", icon: "Icon", shadow: "Shadow", other: "Other" })

    // every group (+ the token descriptions) — the list is fixed per kind of theme, so parse it once per kind
    property var _groupsCache: ({})
    function groupsOf(pal) {
        var k = pal ? "p" : "r"
        if (!adv._groupsCache[k] && adv.ctl) adv._groupsCache[k] = JSON.parse(adv.ctl.groupsJson(pal))
        return adv._groupsCache[k] || ({ groups: [], descriptions: {} })
    }
    readonly property var all: adv.groupsOf(adv.hasPal)
    readonly property int total: {
        var n = 0, g = adv.all.groups
        for (var i = 0; i < g.length; i++) n += g[i].rows.length
        return n
    }
    readonly property var warned: {
        var m = {}, list = adv.st.issues || []
        for (var i = 0; i < list.length; i++) if (m[list[i].fg] === undefined) m[list[i].fg] = list[i].line
        return m
    }
    function targetOf(r) { return { kind: r.kind, id: r.id } }
    function isChangedRow(r) { return adv.doc && adv.base ? ThemeEditorLib.isChanged(adv.doc, adv.base, adv.targetOf(r)) : false }

    // the flattened list: {t: "section", label} | {t: "group", g, open} | {t: "row", row, badge, picked}. It is the
    // list's MODEL, so it depends only on what changes its shape — the search, Changed, the open groups, the Pick — and
    // not on the document (an edit repaints the rows' swatches in place instead of rebuilding the list); with Changed on
    // it follows the document too.
    readonly property bool ready: adv.st.open === true
    readonly property var items: {
        if (!adv.ready || !adv.ctl || !adv.panel) return []
        var q = adv.panel.query
        var shown = q.trim().length > 0 ? JSON.parse(adv.ctl.searchJson(adv.hasPal, q)) : adv.all.groups
        if (adv.panel.changedOnly) {
            var d = adv.doc, b = adv.base, f = []
            for (var i = 0; i < shown.length; i++) {
                var rows = shown[i].rows.filter(function (r) { return ThemeEditorLib.isChanged(d, b, adv.targetOf(r)) })
                if (rows.length > 0) f.push({ id: shown[i].id, section: shown[i].section, label: shown[i].label, rows: rows })
            }
            shown = f
        }
        var allGroups = []
        var picked = adv.panel.picked
        if (picked && picked.length > 0) {
            var prow = [], pb = {}
            for (var p = 0; p < picked.length; p++) {
                var isRole = adv.isRoleId(picked[p].id)
                prow.push({ kind: isRole ? "role" : "token", id: picked[p].id, label: adv.ctl.label(isRole ? "role" : "token", picked[p].id) })
                pb[picked[p].id] = adv.badges[picked[p].kind] || "Other"
            }
            allGroups.push({ g: { id: "picked", section: "Picked", label: "Picked", rows: prow }, badges: pb })
        }
        for (var j = 0; j < shown.length; j++) allGroups.push({ g: shown[j], badges: null })
        var forceOpen = q.trim().length > 0 || adv.panel.changedOnly
        var list = [], section = ""
        for (var k = 0; k < allGroups.length; k++) {
            var g = allGroups[k].g
            if (g.section !== section) { section = g.section; list.push({ t: "section", label: section }) }
            var open = forceOpen || g.id === "picked" || adv.panel.openGroups[g.id] === true
            // the Pick tool's group is always open: its section heading is its header
            if (g.id !== "picked") list.push({ t: "group", g: g, open: open })
            if (!open) continue
            for (var r2 = 0; r2 < g.rows.length; r2++)
                list.push({ t: "row", row: g.rows[r2], badge: allGroups[k].badges ? (allGroups[k].badges[g.rows[r2].id] || "") : "", picked: g.id === "picked" })
        }
        return list
    }
    /// how many rows of a group differ from the base (the header's "‹k› changed")
    function countChanged(g) {
        if (!adv.doc || !adv.base) return 0
        var n = 0
        for (var r = 0; r < g.rows.length; r++) if (ThemeEditorLib.isChanged(adv.doc, adv.base, adv.targetOf(g.rows[r]))) n++
        return n
    }
    // a role id (the roles group of editor-labels.json lists them; everything else a Pick finds is a token)
    readonly property var _roleIds: {
        var m = {}, g = adv.all.groups
        for (var i = 0; i < g.length; i++)
            for (var r = 0; r < g[i].rows.length; r++) if (g[i].rows[r].kind === "role") m[g[i].rows[r].id] = true
        return m
    }
    function isRoleId(id) { return adv._roleIds[id] === true }
    /// the review harness: a row's popover as a click opens it (the row must be in the list)
    function openRowPopover(kind, id) {
        var kids = list.contentItem.children
        for (var i = 0; i < kids.length; i++) {
            var d = kids[i]
            if (d.modelData && d.modelData.t === "row" && d.modelData.row.kind === kind && d.modelData.row.id === id) {
                popover.openFor({ kind: kind, id: id }, d)
                return true
            }
        }
        return false
    }
    function closePopover() { popover.close() }
    function toggle(id, open) {
        var m = {}
        for (var k in adv.panel.openGroups) m[k] = adv.panel.openGroups[k]
        m[id] = !open
        adv.panel.openGroups = m
    }
    // a new pick: back to the top, where "Picked" is
    Connections {
        target: adv.panel
        function onPickedChanged() { if (adv.panel.picked) list.positionViewAtBeginning() }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // the search (full width) + Changed (padding 0 12 8 12)
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 11
            Layout.rightMargin: 11
            Layout.bottomMargin: 8
            spacing: 8
            Kirigami.SearchField {
                id: search
                Layout.fillWidth: true
                placeholderText: "Search " + adv.total.toLocaleString(Qt.locale("en_US"), "f", 0) + " colours"
                Accessible.name: "Search colours"
                text: adv.panel ? adv.panel.query : ""
                onTextChanged: if (adv.panel && adv.panel.query !== text) adv.panel.query = text
            }
            Controls.ToolButton {
                text: "Changed"
                display: Controls.AbstractButton.TextOnly
                checkable: true
                checked: adv.panel ? adv.panel.changedOnly : false
                Controls.ToolTip.text: "Only the colours you changed"
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.delay: 600
                onClicked: { adv.panel.changedOnly = !adv.panel.changedOnly; checked = Qt.binding(() => adv.panel ? adv.panel.changedOnly : false) }
            }
        }

        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: adv.items
            cacheBuffer: 400
            boundsBehavior: Flickable.StopAtBounds
            Controls.ScrollBar.vertical: FoxScrollBar { app: adv.app }
            Accessible.role: Accessible.Tree
            Accessible.name: "Colours"

            delegate: Item {
                id: it
                required property var modelData
                required property int index
                width: list.width
                height: modelData.t === "section" ? 28 : (modelData.t === "group" ? 32 : 40)

                // ---- a section heading (28, bold, bottom-aligned, padding 0 12 4 12)
                Controls.Label {
                    visible: it.modelData.t === "section"
                    x: 11
                    anchors.bottom: parent.bottom
                    anchors.bottomMargin: 4
                    text: it.modelData.label || ""
                    color: Th.settingsThemeEditorGroupFg
                    font.bold: true
                }

                // ---- a group header (32): chevron, label · count, "‹k› changed"
                Controls.AbstractButton {
                    id: groupBtn
                    visible: it.modelData.t === "group"
                    anchors.fill: parent
                    hoverEnabled: true
                    focusPolicy: Qt.StrongFocus
                    Accessible.role: Accessible.Button
                    Accessible.name: it.modelData.t === "group" ? it.modelData.g.label : ""
                    onClicked: adv.toggle(it.modelData.g.id, it.modelData.open)
                    Keys.onReturnPressed: adv.toggle(it.modelData.g.id, it.modelData.open)
                    background: Rectangle {
                        color: groupBtn.hovered ? Th.settingsThemeEditorRowBgHover : "transparent"
                        border.width: groupBtn.visualFocus ? 2 : 0
                        border.color: Th.settingsThemeCardBorderFocus
                    }
                    contentItem: RowLayout {
                        spacing: 6
                        Item { Layout.preferredWidth: 5 }
                        Kirigami.Icon {
                            Layout.preferredWidth: 16; Layout.preferredHeight: 16
                            source: it.modelData.open ? "arrow-down" : "arrow-right"
                            color: Th.settingsThemeEditorGroupFg
                            isMask: true
                        }
                        // the label, then " · ‹count›" right after it (one run of text, like the web's)
                        RowLayout {
                            spacing: 0
                            Controls.Label {
                                text: it.modelData.t === "group" ? it.modelData.g.label : ""
                                color: Th.settingsThemeEditorGroupFg
                            }
                            Controls.Label {
                                text: it.modelData.t === "group" ? " · " + it.modelData.g.rows.length : ""
                                color: Th.settingsHintFg
                            }
                        }
                        Item { Layout.fillWidth: true }
                        Controls.Label {
                            readonly property int n: it.modelData.t === "group" ? adv.countChanged(it.modelData.g) : 0
                            visible: n > 0
                            text: n + " changed"
                            color: Th.settingsThemeEditorEditedDot
                            font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
                        }
                        Item { Layout.preferredWidth: 1 }
                    }
                }

                // ---- a colour row (40): swatch · label over id · badge · warning · changed dot
                Controls.AbstractButton {
                    id: rowBtn
                    visible: it.modelData.t === "row"
                    anchors.fill: parent
                    hoverEnabled: true
                    focusPolicy: Qt.StrongFocus
                    readonly property var row: it.modelData.t === "row" ? it.modelData.row : null
                    readonly property var target: rowBtn.row ? adv.targetOf(rowBtn.row) : null
                    readonly property string warn: rowBtn.row && adv.warned[rowBtn.row.id] !== undefined ? adv.warned[rowBtn.row.id] : ""
                    readonly property bool selected: rowBtn.row !== null && (it.modelData.picked === true
                        || (popover.opened && popover.targetKey === ThemeEditorLib.targetKey(rowBtn.target)))
                    readonly property string tip: rowBtn.row && rowBtn.row.kind === "token" ? (adv.all.descriptions[rowBtn.row.id] || "") : ""
                    Accessible.role: Accessible.Button
                    Accessible.name: rowBtn.row ? rowBtn.row.label : ""
                    Controls.ToolTip.text: rowBtn.tip
                    Controls.ToolTip.visible: rowBtn.hovered && rowBtn.tip.length > 0
                    Controls.ToolTip.delay: 600
                    onClicked: popover.openFor(rowBtn.target, rowBtn)
                    Keys.onReturnPressed: popover.openFor(rowBtn.target, rowBtn)
                    background: Rectangle {
                        color: rowBtn.selected ? Th.settingsThemeEditorRowBgSelected
                             : (rowBtn.hovered ? Th.settingsThemeEditorRowBgHover : "transparent")
                        border.width: rowBtn.visualFocus ? 2 : 0
                        border.color: Th.settingsThemeCardBorderFocus
                    }
                    contentItem: RowLayout {
                        spacing: 8
                        Item { Layout.preferredWidth: 11 }
                        ThemeEditorSwatch {
                            Layout.preferredWidth: 24; Layout.preferredHeight: 24
                            radius: 5
                            colour: rowBtn.row && adv.st.roles ? ThemeEditorLib.qmlColour(ThemeEditorLib.resolvedValue(adv.st, adv.doc, rowBtn.target)) : "transparent"
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            Layout.minimumWidth: 0
                            spacing: 0
                            Controls.Label {
                                Layout.fillWidth: true
                                text: rowBtn.row ? rowBtn.row.label : ""
                                color: Th.settingsFg
                                elide: Text.ElideRight
                            }
                            Controls.Label {
                                Layout.fillWidth: true
                                text: rowBtn.row ? rowBtn.row.id : ""
                                color: Th.settingsHintFg
                                font.family: adv.monoFamily
                                font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
                                elide: Text.ElideRight
                            }
                        }
                        // the Pick tool's kind badge
                        Rectangle {
                            visible: (it.modelData.badge || "").length > 0
                            Layout.preferredHeight: badgeText.implicitHeight
                            Layout.preferredWidth: badgeText.implicitWidth + 12
                            radius: 8
                            color: "transparent"
                            border.width: 1
                            border.color: Th.settingsThemeEditorSwatchBorder
                            Controls.Label {
                                id: badgeText
                                anchors.centerIn: parent
                                text: it.modelData.badge || ""
                                color: Th.settingsHintFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
                            }
                        }
                        Kirigami.Icon {
                            visible: rowBtn.warn.length > 0
                            Layout.preferredWidth: 16; Layout.preferredHeight: 16
                            source: "dialog-warning"
                            color: Th.settingsThemeEditorWarnIcon
                            isMask: true
                            HoverHandler { id: warnHover }
                            Controls.ToolTip.text: rowBtn.warn
                            Controls.ToolTip.visible: warnHover.hovered
                            Controls.ToolTip.delay: 300
                        }
                        Rectangle {
                            visible: rowBtn.row !== null && adv.isChangedRow(rowBtn.row)
                            Layout.preferredWidth: 6; Layout.preferredHeight: 6
                            radius: 3
                            color: Th.settingsThemeEditorEditedDot
                            Accessible.name: "Changed"
                        }
                        Item { Layout.preferredWidth: 1 }
                    }
                }
            }

            // nothing to list
            Controls.Label {
                anchors.horizontalCenter: parent.horizontalCenter
                y: 24
                width: parent.width - 24
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                visible: adv.items.length === 0
                text: adv.panel && adv.panel.query.trim().length > 0 ? "No colours match “" + adv.panel.query.trim() + "”."
                      : (adv.panel && adv.panel.changedOnly ? "Nothing changed yet." : "Loading the colours…")
                color: Th.settingsHintFg
            }
        }
    }

    ThemeColourPopover {
        id: popover
        app: adv.app
        panel: adv.panel
    }
}
