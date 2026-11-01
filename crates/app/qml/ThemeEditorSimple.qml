// ThemeEditorSimple.qml — the theme editor's Simple mode (THEMES-UI.md §13.7; the web's Simple.tsx): Name + Author,
// Colours (THEME-SIMPLE 2026-10-31: the palette groups of tinhat_theme::palette, the accent quick picks, "set by hand",
// the adjusted note, the readability warnings with Fix),
// Text (three fonts from the allowed lists, the text size), Density, Islands (radius, gap, margin, border, shadow,
// transparency). Rows: a 112 px label column (settings.label.fg), then the control, 8 apart; sections 16 apart. Every
// value is read from the working document (panel.st: look, shape, opacity, accentNow, accentChips, issues, fixes);
// every change is one edit through panel.edit() — a drag / one focus of a field is one undo step (a gesture).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the settings.* / settings.themeEditor.* tokens

Item {
    id: simple
    property var app: null
    property var panel: null                // ThemeEditorPanel (st, edit, newGesture …)

    readonly property var st: simple.panel ? simple.panel.st : ({})
    readonly property var doc: simple.st.doc || ({})
    readonly property var look: simple.st.look || ({ density: "normal", fonts: {}, shadow: 0, size: 100 })
    readonly property var shape: simple.st.shape || ({ radius: 12, gap: 6, margin: 6, border: 1 })
    readonly property var fonts: simple.panel && simple.panel.ctl ? JSON.parse(simple.panel.ctl.fontsJson()) : ({ ui: [], code: [] })
    readonly property var uiFonts: ["Default"].concat(simple.fonts.ui)
    readonly property var codeFonts: ["Default"].concat(simple.fonts.code)
    readonly property string monoFamily: Th.fontMono   // the code font (type.code, else the platform monospace)
    // the accent chips as text: an edit that leaves them as they were keeps the chips (and a chip's keyboard focus)
    readonly property string chipsKey: JSON.stringify(simple.st.accentChips || [])
    // THEME-SIMPLE: the palette view (stateJson.palette), People folded away until asked for
    readonly property var groups: simple.st.palette ? simple.st.palette.groups : []
    readonly property var handSet: simple.st.palette ? simple.st.palette.handSet : []
    readonly property bool hasPeople: simple.groups.some(g => g.id === "people" && g.swatches.length > 0)
    property bool more: false
    readonly property var groupLabels: ({ backgrounds: "Backgrounds", text: "Text", accent: "Accent", status: "Status", people: "People" })
    function groupLabel(id) { return simple.groupLabels[id] || id }
    /// How many OTHER colours follow a swatch (its own roles left out: Advanced's "used by" count; the web's usedCount).
    function usedCount(sw) { return sw.usedBy.length - sw.sources.filter(x => x.indexOf(".") >= 0).length }
    /// A swatch's tooltip (the web's swatchTip): its label (+ merged sources), the colour, what it reaches.
    function swatchTip(sw, lead, members) {
        var more = sw.sources.length > 1 ? " +" + (sw.sources.length - 1) + " more" : ""
        var n = simple.usedCount(sw)
        var used = n === 1 ? "used by 1 colour" : "used by " + n + " colours"
        var also = sw.alsoIn.length ? " · also changes " + sw.alsoIn.map(simple.groupLabel).join(", ") : ""
        var group = lead ? " · moves the " + members + " colours of its group" : ""
        return sw.label + more + " · " + String(sw.colour).substring(0, 7) + " · " + used + also + group
    }

    function edit(op, g) { return simple.panel.edit(op, g || 0) }
    /// the review harness: the background / accent popover as a click opens it
    function openPopover(kind) {
        // the review harness: a Simple swatch ("lead:<group>" / "swatch:<source>") as a click opens it
        var gs = simple.groups
        for (var i = 0; i < gs.length; i++) {
            for (var j = 0; j < gs[i].swatches.length; j++) {
                var sw = gs[i].swatches[j], lead = j === 0 && gs[i].swatches.length > 1
                if (kind === "lead:" + gs[i].id && j === 0 || kind === "swatch:" + sw.id)
                    return popover.openFor({ kind: "swatch", id: sw.id, group: gs[i].id, whole: lead, label: sw.label }, simple,
                                           { lead: lead, members: gs[i].swatches.length, usedBy: simple.usedCount(sw), alsoIn: sw.alsoIn.map(simple.groupLabel) })
            }
        }
    }
    function closePopover() { popover.close() }
    function fontIndex(list, v) { return Math.max(0, v ? list.indexOf(v) : 0) }
    function chipOn(c) {
        var now = simple.st.accentNow
        if (now !== null && now !== undefined) return now === c.value
        return String(c.colour).toLowerCase() === String(simple.st.roles ? simple.st.roles["accent.base"] : "")
    }

    // a label + control row (min height 32). (Inline components cannot see this file's ids: what they need comes in
    // through properties.)
    component FieldRow: RowLayout {
        property alias label: rowLabel.text
        Layout.fillWidth: true
        Layout.minimumHeight: 32
        spacing: 8
        Controls.Label {
            id: rowLabel
            Layout.preferredWidth: 112
            Layout.alignment: Qt.AlignVCenter
            color: Th.settingsLabelFg
            elide: Text.ElideRight
        }
    }
    // a slider row: the slider (200 wide) + its value text; one gesture per drag
    component SliderRow: RowLayout {
        id: sr
        property var panel: null
        property alias label: srLabel.text
        property alias from: sl.from
        property alias to: sl.to
        property alias stepSize: sl.stepSize
        property real value: 0
        property string valueText: ""
        signal moved(int v, int gesture)
        property int _g: 0
        Layout.fillWidth: true
        Layout.minimumHeight: 32
        spacing: 8
        Controls.Label {
            id: srLabel
            Layout.preferredWidth: 112
            color: Th.settingsLabelFg
            elide: Text.ElideRight
        }
        ThSlider {
            id: sl
            Layout.preferredWidth: 200
            Layout.maximumWidth: 200
            Layout.fillWidth: true
            value: sr.value
            snapMode: Controls.Slider.SnapAlways
            Accessible.name: srLabel.text
            onMoved: {
                if (sr._g === 0) sr._g = sr.panel.newGesture()
                sr.moved(Math.round(sl.value), sr._g)
            }
            onPressedChanged: if (!sl.pressed) sr._g = 0
        }
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            Layout.minimumWidth: 48
            text: sr.valueText
            color: Th.settingsHintFg
            font.features: ({ "tnum": 1 })
        }
        Item { Layout.fillWidth: true }
    }
    // a text field that edits while you type (one gesture per focus): while it has the focus it shows what you
    // typed; an unusable value (an empty name) is simply not applied
    component EditField: ThTextField {
        id: ef
        property var panel: null
        property string docValue: ""
        property var apply: null            // function (text, gesture)
        property int _g: 0
        Layout.fillWidth: true
        maximumLength: 80
        text: ef.docValue
        onActiveFocusChanged: {
            if (ef.activeFocus) { ef._g = ef.panel.newGesture(); ef.text = ef.docValue }
            else ef.text = Qt.binding(() => ef.docValue)
        }
        onTextEdited: if (ef.apply) ef.apply(ef.text, ef._g)
    }

    Controls.ScrollView {
        id: scroll
        anchors.fill: parent
        contentWidth: availableWidth
        clip: true
        Controls.ScrollBar.vertical: FoxScrollBar { app: simple.app }

        ColumnLayout {
            width: scroll.availableWidth - 22
            x: 11
            spacing: 16

            // ---- 1. Name, Author ------------------------------------------------------------------------------------
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 8
                FieldRow {
                    label: "Name"
                    EditField {
                        panel: simple.panel
                        docValue: String(simple.doc.name || "")
                        Accessible.name: "Theme name"
                        apply: (v, g) => simple.edit({ op: "name", value: v }, g)
                    }
                }
                FieldRow {
                    label: "Author"
                    EditField {
                        panel: simple.panel
                        docValue: typeof simple.doc.author === "string" ? simple.doc.author : ""
                        placeholderText: "Optional"
                        Accessible.name: "Author"
                        apply: (v, g) => simple.edit({ op: "author", value: v }, g)
                    }
                }
            }

            // ---- 2. Colours -----------------------------------------------------------------------------------------
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 8
                SectionLabel { app: simple.app; text: "Colours" }
                HintLabel {
                    app: simple.app
                    Layout.fillWidth: true
                    text: "Change a colour and everything made from it follows. The large swatch moves its whole group."
                }
                // THEME-SIMPLE (2026-10-31): the palette groups (tinhat_theme::palette; the web's Simple.tsx): a label,
                // the lead swatch 36 (a link badge: it moves its group), the others 24
                Repeater {
                    model: simple.groups
                    delegate: RowLayout {
                        id: grp
                        required property var modelData
                        visible: grp.modelData.swatches.length > 0 && (grp.modelData.id !== "people" || simple.more)
                        Layout.fillWidth: true
                        Layout.minimumHeight: 40
                        spacing: 8
                        Accessible.role: Accessible.Grouping
                        Accessible.name: grp.modelData.label
                        Controls.Label {
                            Layout.preferredWidth: 112
                            Layout.alignment: Qt.AlignVCenter
                            text: grp.modelData.label
                            textFormat: Text.PlainText
                            color: Th.settingsLabelFg
                            elide: Text.ElideRight
                        }
                        Flow {
                            Layout.fillWidth: true
                            spacing: 6
                            Repeater {
                                model: grp.modelData.swatches
                                delegate: Controls.AbstractButton {
                                    id: swb
                                    required property var modelData
                                    required property int index
                                    readonly property bool lead: swb.index === 0 && grp.modelData.swatches.length > 1
                                    readonly property string tip: simple.swatchTip(swb.modelData, swb.lead, grp.modelData.swatches.length)
                                    width: swb.lead ? 40 : 24; height: swb.lead ? 36 : 24
                                    y: swb.lead ? 0 : 6
                                    padding: 0
                                    hoverEnabled: true
                                    focusPolicy: Qt.StrongFocus
                                    Accessible.name: swb.tip
                                    Controls.ToolTip.text: swb.tip
                                    Controls.ToolTip.visible: swb.hovered
                                    Controls.ToolTip.delay: 600
                                    onClicked: popover.openFor({ kind: "swatch", id: swb.modelData.id, group: grp.modelData.id, whole: swb.lead, label: swb.modelData.label }, swb,
                                                               { lead: swb.lead, members: grp.modelData.swatches.length, usedBy: simple.usedCount(swb.modelData),
                                                                 alsoIn: swb.modelData.alsoIn.map(simple.groupLabel) })
                                    background: Item {}
                                    contentItem: Item {
                                        ThemeEditorSwatch {
                                            width: swb.lead ? 36 : 24; height: swb.lead ? 36 : 24
                                            radius: swb.lead ? 8 : 6
                                            colour: ThemeEditorLib.qmlColour(swb.modelData.colour)
                                            ringWidth: popover.opened && popover.target0.kind === "swatch" && popover.target0.id === swb.modelData.id ? 2 : 1
                                            ringColour: ringWidth === 2 ? Th.settingsFg : Th.settingsThemeEditorSwatchBorder
                                        }
                                        Rectangle {
                                            visible: swb.lead
                                            x: 36 - 12; y: 36 - 12
                                            width: 16; height: 16; radius: 8
                                            color: Th.settingsThemeEditorBg
                                            border.width: 1; border.color: Th.settingsThemeEditorSwatchBorder
                                            Kirigami.Icon {
                                                anchors.centerIn: parent
                                                width: 12; height: 12
                                                source: "link-symbolic"
                                                color: Th.settingsFg
                                                isMask: true
                                            }
                                        }
                                        Rectangle {
                                            width: swb.lead ? 36 : 24; height: swb.lead ? 36 : 24
                                            anchors.margins: -3
                                            radius: swb.lead ? 11 : 9; color: "transparent"
                                            visible: swb.visualFocus
                                            border.width: 2; border.color: Th.settingsThemeCardBorderFocus
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                FieldRow {
                    label: "Quick picks"
                    Flow {
                        Layout.fillWidth: true
                        spacing: 6
                        Accessible.role: Accessible.RadioButton
                        Accessible.name: "Accent quick picks"
                        Repeater {
                            model: JSON.parse(simple.chipsKey)
                            delegate: Controls.AbstractButton {
                                id: chip
                                required property var modelData
                                readonly property bool on: simple.chipOn(modelData)
                                width: 24; height: 24
                                padding: 0
                                focusPolicy: Qt.StrongFocus
                                Accessible.role: Accessible.RadioButton
                                Accessible.name: modelData.label
                                Accessible.checked: chip.on
                                Controls.ToolTip.text: modelData.label
                                Controls.ToolTip.visible: chip.hovered
                                Controls.ToolTip.delay: 600
                                hoverEnabled: true
                                onClicked: simple.edit({ op: "accent", value: modelData.value })
                                contentItem: Item {
                                    ThemeEditorSwatch {
                                        anchors.fill: parent
                                        radius: 12
                                        colour: ThemeEditorLib.qmlColour(simple.panel.ctl.canonColour(chip.modelData.colour))
                                        ringWidth: chip.on ? 2 : 1
                                        ringColour: chip.on ? Th.settingsFg : Th.settingsThemeEditorSwatchBorder
                                    }
                                    Kirigami.Icon {
                                        visible: chip.on
                                        anchors.centerIn: parent
                                        width: 12; height: 12
                                        source: "checkmark-symbolic"
                                        color: Th.settingsFg
                                        isMask: true
                                    }
                                    Rectangle {
                                        anchors.fill: parent; anchors.margins: -3
                                        radius: 15; color: "transparent"
                                        visible: chip.visualFocus
                                        border.width: 2; border.color: Th.settingsThemeCardBorderFocus
                                    }
                                }
                                background: Item {}
                            }
                        }
                    }
                }
                ThButton {
                    app: simple.app
                    visible: simple.hasPeople
                    flat: true
                    text: simple.more ? "Fewer colours" : "More colours (people)"
                    onClicked: simple.more = !simple.more
                }
                RowLayout {
                    visible: simple.handSet.length > 0
                    Layout.fillWidth: true
                    spacing: 6
                    Kirigami.Icon {
                        Layout.preferredWidth: 16; Layout.preferredHeight: 16
                        source: "help-about-symbolic"
                        color: Th.settingsThemeNoteIcon
                        isMask: true
                    }
                    Controls.Label {
                        textFormat: Text.PlainText
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        text: simple.handSet.length === 1 ? "1 colour is set by hand and doesn't follow these."
                                                          : simple.handSet.length + " colours are set by hand and don't follow these."
                        color: Th.settingsThemeNoteFg
                    }
                    ThButton {
                        app: simple.app
                        flat: true
                        text: "Show in Advanced"
                        onClicked: { simple.panel.query = ""; simple.panel.changedOnly = false; simple.panel.onlyIds = simple.handSet; simple.panel.mode = "advanced" }
                    }
                }
                // the palette theme's own readability adjustments (§3.2's note)
                RowLayout {
                    visible: (simple.st.adjusted || 0) > 0
                    spacing: 6
                    Kirigami.Icon {
                        Layout.preferredWidth: 16; Layout.preferredHeight: 16
                        source: "help-about-symbolic"
                        color: Th.settingsThemeNoteIcon
                        isMask: true
                    }
                    Controls.Label { textFormat: Text.PlainText; text: ThemeEditorLib.adjustedText(simple.st.adjusted || 0); color: Th.settingsThemeNoteFg }
                }
                // §13.5: what the edits broke — "‹n› colours are hard to read", at most 4 lines with Fix
                ThemeEditorWarnings {
                    Layout.fillWidth: true
                    visible: (simple.st.fixes || []).length > 0
                    app: simple.app
                    fixes: simple.st.fixes || []
                    onFix: (op) => simple.edit(op, 0)
                }
            }

            // ---- 3. Text --------------------------------------------------------------------------------------------
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 8
                SectionLabel { app: simple.app; text: "Text" }
                Repeater {
                    model: [["ui", "Interface font"], ["chat", "Chat font"], ["code", "Code font"]]
                    delegate: FieldRow {
                        id: fontRow
                        required property var modelData
                        readonly property var list: modelData[0] === "code" ? simple.codeFonts : simple.uiFonts
                        label: modelData[1]
                        ThComboBox {
                            Layout.preferredWidth: 220
                            model: fontRow.list
                            currentIndex: simple.fontIndex(fontRow.list, simple.look.fonts ? simple.look.fonts[fontRow.modelData[0]] : null)
                            Accessible.name: fontRow.modelData[1]
                            onActivated: (i) => simple.edit({ op: "font", slot: fontRow.modelData[0], value: i === 0 ? null : fontRow.list[i] }, 0)
                        }
                        Item { Layout.fillWidth: true }
                    }
                }
                SliderRow {
                    panel: simple.panel
                    label: "Text size"
                    from: 80; to: 150; stepSize: 5
                    value: simple.look.size
                    valueText: simple.look.size + " %"
                    onMoved: (v, g) => simple.edit({ op: "size", value: v === 100 ? null : v }, g)
                }
            }

            // ---- 4. Density -----------------------------------------------------------------------------------------
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 8
                SectionLabel { app: simple.app; text: "Density" }
                RowLayout {
                    spacing: 12
                    Accessible.role: Accessible.RadioButton
                    Repeater {
                        model: [["compact", "Compact"], ["normal", "Normal"], ["touch", "Touch"]]
                        delegate: Controls.RadioButton {
                            required property var modelData
                            text: LinkGate.plain(modelData[1])
                            checked: simple.look.density === modelData[0]
                            onClicked: {
                                simple.edit({ op: "density", value: modelData[0] === "normal" ? null : modelData[0] }, 0)
                                checked = Qt.binding(() => simple.look.density === modelData[0])
                            }
                        }
                    }
                }
            }

            // ---- 5. Islands -----------------------------------------------------------------------------------------
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 8
                SectionLabel { app: simple.app; text: "Islands" }
                SliderRow {
                    panel: simple.panel
                    label: "Corner radius"; from: 0; to: 24; stepSize: 1
                    value: simple.shape.radius; valueText: simple.shape.radius + " px"
                    onMoved: (v, g) => simple.edit({ op: "shape", key: "radius", value: v }, g)
                }
                SliderRow {
                    panel: simple.panel
                    label: "Gap"; from: 0; to: 24; stepSize: 1
                    value: simple.shape.gap; valueText: simple.shape.gap + " px"
                    onMoved: (v, g) => simple.edit({ op: "shape", key: "gap", value: v }, g)
                }
                SliderRow {
                    panel: simple.panel
                    label: "Margin"; from: 0; to: 24; stepSize: 1
                    value: simple.shape.margin; valueText: simple.shape.margin + " px"
                    onMoved: (v, g) => simple.edit({ op: "shape", key: "margin", value: v }, g)
                }
                SliderRow {
                    panel: simple.panel
                    label: "Border"; from: 0; to: 2; stepSize: 1
                    value: simple.shape.border; valueText: simple.shape.border + " px"
                    onMoved: (v, g) => simple.edit({ op: "shape", key: "border", value: v }, g)
                }
                SliderRow {
                    panel: simple.panel
                    label: "Shadow"; from: 0; to: 24; stepSize: 1
                    value: simple.look.shadow; valueText: simple.look.shadow === 0 ? "Off" : simple.look.shadow + " px"
                    onMoved: (v, g) => simple.edit({ op: "shape", key: "shadow", value: v === 0 ? null : v }, g)
                }
                SliderRow {
                    panel: simple.panel
                    readonly property bool mixed: simple.st.opacity === null || simple.st.opacity === undefined
                    label: "Transparency"; from: 0; to: 60; stepSize: 5
                    value: mixed ? 0 : 100 - simple.st.opacity
                    valueText: mixed ? "Mixed" : (100 - simple.st.opacity) + " %"
                    onMoved: (v, g) => simple.edit({ op: "opacity", value: 100 - v }, g)
                }
                HintLabel {
                    app: simple.app
                    Layout.fillWidth: true
                    text: "Transparency lets the window background show through the islands."
                }
            }
            Item { Layout.preferredHeight: 4 }
        }
    }

    ThemeColourPopover {
        id: popover
        app: simple.app
        panel: simple.panel
    }
}
