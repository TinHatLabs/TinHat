// ThemeShareCard.qml — a theme shared in a room, inside its message bubble (11.0 step 5, contract THEMES-UI.md §14.3;
// the web's ShareCard.tsx + share.module.css): painted in the SHARED theme's own roles (`row.colours`, computed by this
// receiver — theme_editor.rs share_row): "Tin Hat theme", the name, "by ‹author› · Dark theme", the ten swatches, Try
// (30 s, then back) and Keep ("Keep" / "Use" / "In use"). A card this version can't use (refused / newer) says so in the
// app's colours. The bubble hides the message's body text while a card shows. `row` = the timeline row's `themeShare`
// ({kind, name, author, variant, swatchColours, colours, themeJson, error}); `app` = Main.qml (tryTheme, keepTheme,
// endThemeTrial, themeTrial, editorCtl, themeCtl).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: bubble.themeShare.*

Item {
    id: card
    property var app: null
    property var row: null
    property string eventId: ""
    property bool mine: false
    property real maxWidth: 320

    readonly property bool ok: card.row !== null && card.row.kind === "ok"
    readonly property var k: card.ok && card.row.colours ? card.row.colours : ({})
    readonly property string name: card.row && card.row.name ? String(card.row.name) : ""
    readonly property bool trying: !!(card.app && card.app.themeTrial && card.app.themeTrial.key === card.eventId && card.eventId.length > 0)
    readonly property bool editorOpen: !!(card.app && card.app.editorCtl && card.app.editorCtl.previewOwner === "editor")
    // "keep" | "use" | "inUse" — re-read when My themes or the theme on screen change
    readonly property string keepState: {
        var _rev = card.app && card.app.themeCtl ? card.app.themeCtl.stateRev : 0
        if (!card.ok || !card.app || !card.app.editorCtl) return "keep"
        return card.app.editorCtl.keepState(card.row.themeJson, card.app.themeCtl ? card.app.themeCtl.effectiveTheme : "")
    }
    readonly property string refusedText: {
        if (!card.row || card.ok) return ""
        if (card.row.kind === "newer") return "This theme needs a newer version of Tin Hat."
        var code = String(card.row.error || "refused").split(":")[0]
        return card.name.length > 0 ? card.name + " can't be used by this version of Tin Hat (" + code + ")."
                                    : "This theme can't be used (" + code + ")."
    }

    implicitWidth: Math.min(320, card.maxWidth)
    implicitHeight: col.implicitHeight + 24
    Accessible.role: Accessible.Grouping
    Accessible.name: card.ok ? card.name + ", Tin Hat theme" : "Tin Hat theme"

    Rectangle {
        anchors.fill: parent
        radius: 10
        color: card.ok ? (card.k.bg || "transparent") : "transparent"
        border.width: 1
        border.color: card.mine ? Th.bubbleThemeShareBorderOwn : Th.bubbleThemeShareBorder
    }

    ColumnLayout {
        id: col
        x: 12; y: 12
        width: card.width - 24
        spacing: 8

        // 1. the caption: the glyph (the card's bright accent; refused: the warning) + "Tin Hat theme"
        RowLayout {
            spacing: 6
            Kirigami.Icon {
                Layout.preferredWidth: 16; Layout.preferredHeight: 16
                source: card.ok ? "preferences-desktop-color" : "dialog-warning"
                color: card.ok ? card.k.bright : Th.bubbleThemeShareRefusedIcon
                isMask: true
            }
            Controls.Label {
                text: "Tin Hat theme"
                color: card.ok ? card.k.fg2 : (card.mine ? Th.bubbleThemeShareRefusedFgOwn : Th.bubbleThemeShareRefusedFg)
                font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
            }
        }
        // refused / newer: the reason
        Controls.Label {
            visible: !card.ok
            Layout.fillWidth: true
            text: card.refusedText
            color: card.mine ? Th.bubbleThemeShareRefusedFgOwn : Th.bubbleThemeShareRefusedFg
            wrapMode: Text.Wrap
        }
        // 2. the name, 3. "by ‹author› · Dark theme"
        ColumnLayout {
            visible: card.ok
            Layout.fillWidth: true
            spacing: 2
            Controls.Label {
                Layout.fillWidth: true
                text: card.name
                color: card.k.fg || "transparent"
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }
            Controls.Label {
                Layout.fillWidth: true
                text: (card.row && card.row.author ? "by " + card.row.author + " · " : "")
                      + (card.row && card.row.variant === "light" ? "Light theme" : "Dark theme")
                color: card.k.fg2 || "transparent"
                font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
                elide: Text.ElideRight
            }
        }
        // 4. the swatch strip: 10 squares 18 × 18, radius 4, gap 4, a 1 px ring in the card's subtle border
        Flow {
            visible: card.ok
            Layout.fillWidth: true
            spacing: 4
            Accessible.ignored: true
            Repeater {
                model: card.ok ? (card.row.swatchColours || []) : []
                delegate: Rectangle {
                    required property var modelData
                    width: 18; height: 18
                    radius: 4
                    color: modelData
                    border.width: 1
                    border.color: card.k.border || "transparent"
                }
            }
        }
        // 5. Try · Keep
        RowLayout {
            visible: card.ok
            spacing: 8
            ThemeShareButton {
                k: card.k
                text: card.trying ? "Stop" : "Try for 30 s"
                Accessible.name: card.trying ? "Stop trying " + card.name : "Try " + card.name
                tip: card.editorOpen ? "Close the theme editor to try another theme" : ""
                enabled: !card.editorOpen
                onClicked: {
                    if (card.trying) card.app.endThemeTrial(true)
                    else card.app.tryTheme(card.eventId, card.row.themeJson, card.name)
                }
            }
            ThemeShareButton {
                k: card.k
                primary: true
                text: card.keepState === "keep" ? "Keep" : (card.keepState === "use" ? "Use" : "In use")
                Accessible.name: "Keep " + card.name
                enabled: card.keepState !== "inUse"
                onClicked: card.app.keepTheme(card.row.themeJson)
            }
        }
    }
}
