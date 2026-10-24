// ThemeColourPopover.qml — the theme editor's colour popover (THEMES-UI.md §13.9; the web's ColourPopover.tsx): the
// house Popover, 280 wide (padding 12, rows 8 apart), opened left of the swatch / row it edits (top-aligned, 8 apart,
// kept inside the window). The title (label, bold) + id (small monospace); the colour field 256 × 160 (saturation →
// right, value → up at the current hue — content colours drawn with gradients, not tokens) with its Ø14 handle, the hue
// bar and (roles, tokens) the alpha bar over the checkerboard; Hex (+ Alpha) fields; (roles, tokens) the ten "Theme
// colours" (a click stores the role REFERENCE), the stored value / "default: …" line and Edit expression…; an error
// line; the readability lines with Fix; Use default · Reset. A drag in the field / a bar is ONE gesture (one undo step);
// the moves are coalesced to one edit per frame. HSV ↔ RGB and the canonical text: ThemeEditorLib (the web's colour.ts).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: popover.*, settings.* and settings.themeEditor.* tokens

Controls.Popup {
    id: pop
    property var app: null
    property var panel: null
    property var target: ({ kind: "background" })
    readonly property string targetKey: ThemeEditorLib.targetKey(pop.target)
    property Item anchorItem: null

    readonly property var st: pop.panel ? pop.panel.st : ({})
    readonly property var doc: pop.st.doc || null
    readonly property var base: pop.st.base || null
    readonly property var ctl: pop.panel ? pop.panel.ctl : null
    readonly property string current: (pop.doc && pop.st.roles) ? String(ThemeEditorLib.resolvedValue(pop.st, pop.doc, pop.target)) : "#000000"
    readonly property var stored: pop.doc ? ThemeEditorLib.storedValue(pop.doc, pop.target) : null
    readonly property bool withAlpha: ThemeEditorLib.alphaAllowed(pop.target)
    readonly property bool changed: (pop.doc && pop.base) ? ThemeEditorLib.isChanged(pop.doc, pop.base, pop.target) : false
    readonly property bool isExpr: pop.stored !== null && ThemeEditorLib.literalOf(pop.stored) === null
    readonly property var issues: {
        var out = [], list = pop.st.issues || []
        if (pop.target.kind !== "role" && pop.target.kind !== "token") return out
        for (var i = 0; i < list.length; i++) if (list[i].fg === pop.target.id) out.push(list[i])
        return out
    }
    readonly property var themeChips: ["accent.base", "accent.bright", "text.primary", "text.secondary", "surface.window",
                                       "surface.island", "surface.raised", "state.danger", "state.warning", "state.success"]
    readonly property string monoFamily: Th.fontMono   // the code font (type.code, else the platform monospace)

    // the popover's own state (the web's useState): hsv + alpha, the fields, the expression editor, the last error
    property real hue: 0
    property real sat: 0
    property real val: 0
    property real alpha: 1
    property bool hexBad: false
    property bool exprOpen: false
    property string error: ""
    property int dragging: 0                // the drag's gesture id, 0 = none
    property string lastSet: ""

    /// Open for `t` next to `item` (THEMES-UI §13.9: left of it, top-aligned, 8 apart; flipped right when there is no
    /// room; kept 6 inside the window).
    function openFor(t, item) {
        pop.target = t
        pop.anchorItem = item
        pop.exprOpen = false
        pop.error = ""
        pop.lastSet = ""
        pop.follow(true)
        pop.place()
        pop.open()
    }
    function place() {
        if (!pop.anchorItem || !pop.parent) return
        var p = pop.anchorItem.mapToItem(pop.parent, 0, 0)
        var w = pop.implicitWidth, h = pop.implicitHeight
        var x = p.x - 8 - w
        if (x < 6) x = p.x + pop.anchorItem.width + 8
        pop.x = Math.max(6, Math.min(x, pop.parent.width - w - 6))
        pop.y = Math.max(6, Math.min(p.y, pop.parent.height - h - 6))
    }
    onImplicitHeightChanged: if (pop.opened) pop.place()

    /// Follow the working document (open, undo / redo, a chip) — never while a drag is setting it.
    function follow(force) {
        if (pop.dragging !== 0) return
        if (!force && pop.current === pop.lastSet) return
        var c = ThemeEditorLib.literalOf(pop.current)
        if (!c) return
        var n = ThemeEditorLib.rgbToHsv(c.r, c.g, c.b)
        // keep the hue (and saturation) through greys and black, where HSV forgets them
        if (!(n.s === 0 || n.v === 0)) pop.hue = n.h
        if (n.v !== 0) pop.sat = n.s
        pop.val = n.v
        pop.alpha = c.a
        hexField.text = pop.current.substring(0, 7)
        alphaField.text = String(Math.round(c.a * 100))
        pop.hexBad = false
        pop.lastSet = pop.current
    }
    onCurrentChanged: if (pop.opened) pop.follow(false)

    function apply(gesture) {
        var text = ThemeEditorLib.colourText(ThemeEditorLib.hsvToRgb(pop.hue, pop.sat, pop.val), pop.withAlpha ? pop.alpha : 1)
        pop.lastSet = text
        hexField.text = text.substring(0, 7)
        var err = pop.panel.edit(ThemeEditorLib.opFor(pop.target, text), gesture)
        pop.error = err.length > 0 ? err : ""
    }
    // one edit per frame while dragging
    Timer { id: frameTick; interval: 16; onTriggered: pop.apply(pop.dragging) }
    function scheduleApply() { if (!frameTick.running) frameTick.start() }
    function beginDrag() { pop.dragging = pop.panel.newGesture() }
    function endDrag() {
        if (frameTick.running) frameTick.stop()
        pop.apply(pop.dragging)
        pop.dragging = 0
    }
    function commitHex() {
        var t = hexField.text.trim()
        var canon = pop.ctl ? pop.ctl.canonColour(t.charAt(0) === "#" ? t : "#" + t) : ""
        var c = ThemeEditorLib.literalOf(canon)
        if (!c) { pop.hexBad = true; return }
        pop.hexBad = false
        var n = ThemeEditorLib.rgbToHsv(c.r, c.g, c.b)
        pop.hue = n.s === 0 ? pop.hue : n.h
        pop.sat = n.s
        pop.val = n.v
        pop.alpha = pop.withAlpha ? (c.a < 1 ? c.a : pop.alpha) : 1
        pop.apply(0)
    }
    function commitAlpha() {
        var n = Number(alphaField.text)
        if (!isFinite(n) || n < 0 || n > 100 || alphaField.text.trim().length === 0) { alphaField.text = String(Math.round(pop.alpha * 100)); return }
        pop.alpha = Math.round(n) / 100
        pop.apply(0)
    }
    function setError(err) { pop.error = err && err.length > 0 ? err : "" }

    readonly property color hueColour: {
        var rgb = ThemeEditorLib.hsvToRgb(pop.hue, 1, 1)
        return Qt.rgba(rgb[0] / 255, rgb[1] / 255, rgb[2] / 255, 1)   // theme-lint: ok — content: the colour field's hue
    }
    readonly property color solidColour: {
        var rgb = ThemeEditorLib.hsvToRgb(pop.hue, pop.sat, pop.val)
        return Qt.rgba(rgb[0] / 255, rgb[1] / 255, rgb[2] / 255, 1)   // theme-lint: ok — content: the colour being edited
    }

    parent: Controls.Overlay.overlay
    modal: false
    focus: true
    padding: 12
    closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
    background: Rectangle { color: Th.popoverBg; border.width: 1; border.color: Th.popoverBorder }
    onClosed: { pop.exprOpen = false; pop.dragging = 0 }

    contentItem: ColumnLayout {
        width: 256
        spacing: 8

        ColumnLayout {
            Layout.preferredWidth: 256
            spacing: 0
            Controls.Label {
                Layout.fillWidth: true
                text: pop.ctl ? ThemeEditorLib.targetLabel(pop.ctl, pop.target) : ""
                color: Th.settingsFg
                font.bold: true
                elide: Text.ElideRight
            }
            Controls.Label {
                Layout.fillWidth: true
                text: ThemeEditorLib.targetId(pop.target)
                color: Th.settingsHintFg
                font.family: pop.monoFamily
                font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
                elide: Text.ElideRight
            }
        }

        // ---- the colour field: saturation → right, value → up (content colours: white → the hue, then transparent →
        // black over it)
        Item {
            id: svField
            Layout.preferredWidth: 256
            Layout.preferredHeight: 160
            Accessible.role: Accessible.Slider
            Accessible.name: "Saturation and brightness"
            Rectangle {
                anchors.fill: parent
                radius: 4
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0; color: "#ffffff" }   // theme-lint: ok — content: the colour field
                    GradientStop { position: 1; color: pop.hueColour }
                }
            }
            Rectangle {
                anchors.fill: parent
                radius: 4
                gradient: Gradient {
                    GradientStop { position: 0; color: "transparent" }
                    GradientStop { position: 1; color: "#000000" }   // theme-lint: ok — content: the colour field
                }
            }
            ThemePickerHandle { x: pop.sat * svField.width - 7; y: (1 - pop.val) * svField.height - 7; fill: pop.solidColour }
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.CrossCursor
                preventStealing: true
                function pick(mx, my) {
                    pop.sat = Math.min(1, Math.max(0, mx / width))
                    pop.val = 1 - Math.min(1, Math.max(0, my / height))
                    pop.scheduleApply()
                }
                onPressed: (m) => { pop.beginDrag(); pick(m.x, m.y) }
                onPositionChanged: (m) => { if (pressed) pick(m.x, m.y) }
                onReleased: pop.endDrag()
            }
        }
        // ---- the hue bar (the spectrum)
        Item {
            id: hueBar
            Layout.preferredWidth: 256
            Layout.preferredHeight: 12
            Accessible.role: Accessible.Slider
            Accessible.name: "Hue"
            Rectangle {
                anchors.fill: parent
                radius: 6
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    // theme-lint: ok — content: the hue spectrum (the colour picker's own scale, not the theme)
                    GradientStop { position: 0 / 6; color: "#ff0000" }
                    GradientStop { position: 1 / 6; color: "#ffff00" }
                    GradientStop { position: 2 / 6; color: "#00ff00" }
                    GradientStop { position: 3 / 6; color: "#00ffff" }
                    GradientStop { position: 4 / 6; color: "#0000ff" }
                    GradientStop { position: 5 / 6; color: "#ff00ff" }
                    GradientStop { position: 6 / 6; color: "#ff0000" }
                }
            }
            ThemePickerHandle { x: pop.hue / 360 * hueBar.width - 7; y: hueBar.height / 2 - 7; fill: pop.hueColour }
            MouseArea {
                anchors.fill: parent
                preventStealing: true
                function pick(mx) {
                    pop.hue = Math.min(359.999, Math.min(1, Math.max(0, mx / width)) * 360)
                    pop.scheduleApply()
                }
                onPressed: (m) => { pop.beginDrag(); pick(m.x) }
                onPositionChanged: (m) => { if (pressed) pick(m.x) }
                onReleased: pop.endDrag()
            }
        }
        // ---- the alpha bar (roles, tokens): the checkerboard under transparent → the colour
        Item {
            id: alphaBar
            visible: pop.withAlpha
            Layout.preferredWidth: 256
            Layout.preferredHeight: 12
            Accessible.role: Accessible.Slider
            Accessible.name: "Opacity"
            ThemeChecker { anchors.fill: parent; radius: 6 }
            Rectangle {
                anchors.fill: parent
                radius: 6
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0; color: "transparent" }
                    GradientStop { position: 1; color: pop.solidColour }
                }
            }
            ThemePickerHandle { x: pop.alpha * alphaBar.width - 7; y: alphaBar.height / 2 - 7; fill: "transparent" }
            MouseArea {
                anchors.fill: parent
                preventStealing: true
                function pick(mx) {
                    pop.alpha = Math.round(Math.min(1, Math.max(0, mx / width)) * 100) / 100
                    alphaField.text = String(Math.round(pop.alpha * 100))
                    pop.scheduleApply()
                }
                onPressed: (m) => { pop.beginDrag(); pick(m.x) }
                onPositionChanged: (m) => { if (pressed) pick(m.x) }
                onReleased: pop.endDrag()
            }
        }

        // ---- Hex (+ Alpha)
        RowLayout {
            Layout.preferredWidth: 256
            spacing: 8
            Controls.Label { text: "Hex"; color: Th.settingsLabelFg }
            ThTextField {
                id: hexField
                Layout.preferredWidth: 110
                maximumLength: 9
                Accessible.name: "Hex colour"
                onAccepted: pop.commitHex()
                onEditingFinished: pop.commitHex()
                // an unreadable value: the field's border turns to the error colour (the field keeps what you typed)
                Rectangle {
                    anchors.fill: parent
                    radius: 8
                    color: "transparent"
                    visible: pop.hexBad
                    border.width: 2
                    border.color: Th.settingsErrorFg
                }
            }
            Controls.Label { visible: pop.withAlpha; text: "Alpha"; color: Th.settingsLabelFg }
            ThTextField {
                id: alphaField
                visible: pop.withAlpha
                Layout.preferredWidth: 64
                maximumLength: 3
                Accessible.name: "Alpha in per cent"
                onAccepted: pop.commitAlpha()
                onEditingFinished: pop.commitAlpha()
            }
            Item { Layout.fillWidth: true }
        }

        // ---- the theme colours (a REFERENCE: it follows the theme), the stored value, Edit expression…
        Controls.Label { visible: pop.withAlpha; text: "Theme colours"; color: Th.settingsLabelFg }
        Flow {
            visible: pop.withAlpha
            Layout.preferredWidth: 256
            spacing: 6
            Repeater {
                model: pop.themeChips
                delegate: Controls.AbstractButton {
                    id: tchip
                    required property var modelData
                    width: 20; height: 20
                    padding: 0
                    hoverEnabled: true
                    Accessible.role: Accessible.RadioButton
                    Accessible.name: pop.ctl ? pop.ctl.label("role", modelData) : modelData
                    Accessible.checked: pop.stored === modelData
                    Controls.ToolTip.text: Accessible.name
                    Controls.ToolTip.visible: tchip.hovered
                    Controls.ToolTip.delay: 600
                    onClicked: pop.setError(pop.panel.edit(ThemeEditorLib.opFor(pop.target, modelData), 0))
                    background: Item {}
                    contentItem: ThemeEditorSwatch {
                        radius: 10
                        colour: ThemeEditorLib.qmlColour(pop.st.roles ? pop.st.roles[tchip.modelData] : "")
                        ringWidth: pop.stored === tchip.modelData ? 2 : 1
                        ringColour: pop.stored === tchip.modelData ? Th.settingsFg : Th.settingsThemeEditorSwatchBorder
                    }
                }
            }
        }
        Controls.Label {
            visible: pop.withAlpha
            Layout.preferredWidth: 256
            text: pop.stored !== null ? pop.stored
                  : (pop.target.kind === "token" && pop.ctl ? "default: " + pop.ctl.tokenDefault(pop.target.id) : "made from the palette")
            color: Th.settingsHintFg
            font.family: pop.monoFamily
            font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
            wrapMode: Text.WrapAnywhere
        }
        ThButton {
            visible: pop.withAlpha && !pop.exprOpen
            app: pop.app
            flat: true
            text: "Edit expression…"
            onClicked: {
                exprField.text = pop.stored !== null ? pop.stored : ""
                pop.exprOpen = true
                Qt.callLater(function () { exprField.forceActiveFocus() })
            }
        }
        ThTextField {
            id: exprField
            visible: pop.withAlpha && pop.exprOpen
            Layout.preferredWidth: 256
            placeholderText: "mixok(surface.island, text.primary, 0.1)"
            Accessible.name: "Expression"
            onAccepted: {
                var err = pop.panel.edit(ThemeEditorLib.opFor(pop.target, exprField.text), 0)
                pop.setError(err)
                if (err.length === 0) pop.exprOpen = false
            }
        }
        Controls.Label {
            visible: pop.error.length > 0
            Layout.preferredWidth: 256
            text: ThemeEditorLib.errorText(pop.error)
            color: Th.settingsErrorFg
            font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
            wrapMode: Text.Wrap
        }
        Controls.Label {
            visible: pop.isExpr && !pop.exprOpen
            Layout.preferredWidth: 256
            text: "This colour follows an expression; picking a colour replaces it."
            color: Th.settingsHintFg
            font.family: pop.monoFamily
            font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
            wrapMode: Text.Wrap
        }
        // the readability line(s) of this colour + Fix
        Repeater {
            model: pop.issues
            delegate: RowLayout {
                required property var modelData
                Layout.preferredWidth: 256
                spacing: 8
                Kirigami.Icon {
                    Layout.preferredWidth: 16; Layout.preferredHeight: 16
                    source: "dialog-warning"
                    color: Th.settingsThemeEditorWarnIcon
                    isMask: true
                }
                Controls.Label {
                    Layout.fillWidth: true
                    text: modelData.line
                    color: Th.settingsThemeEditorWarnFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
                    wrapMode: Text.Wrap
                }
                ThButton {
                    app: pop.app
                    flat: true
                    text: "Fix"
                    onClicked: {
                        var op = pop.ctl ? pop.ctl.fixOp(modelData.fg, modelData.bg) : ""
                        if (op.length > 0) pop.setError(pop.panel.edit(JSON.parse(op), 0))
                    }
                }
            }
        }
        // ---- Use default · Reset
        RowLayout {
            Layout.preferredWidth: 256
            spacing: 8
            Item { Layout.fillWidth: true }
            ThButton {
                visible: pop.doc ? ThemeEditorLib.canUseDefault(pop.doc, pop.target) : false
                app: pop.app
                flat: true
                text: "Use default"
                onClicked: pop.setError(pop.panel.edit(ThemeEditorLib.opFor(pop.target, null), 0))
            }
            ThButton {
                app: pop.app
                flat: true
                text: "Reset"
                enabled: pop.changed
                onClicked: pop.setError(pop.panel.edit(ThemeEditorLib.resetOp(pop.base, pop.target), 0))
            }
        }
    }
}
