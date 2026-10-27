// ActionButton.qml — one button of a row, drawn from its resolved item (ACTIONS.md §9.4; web src/actions/ui/
// ActionButton.tsx — the same rules, the same numbers).
//
// Styles (the row kind's `style`): `header` = the room header's round HeaderIconButton; `compact` = the room column
// header's 28 px tool button (16 px glyph); everything else = the house tool button (ThToolButton's own background,
// focus ring, padding and tooltip): scale S = the style's size with a 22 px glyph (today's tool rows), M = 50 px with a
// 28 px glyph. Looks: icon | iconLabel | label. A dimmed item is disabled at half opacity; a placeholder (a newer Tin
// Hat's action) is always dimmed. toggleLook: "pose" holds the glyph's active pose (today's tool rows), "checked" also
// fills the button (your own button islands, where a toggle must read as on / off). A danger action's glyph and label
// are red; a primary action is the call to action (GlowButton). Colours only from Th; nothing animates at rest.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the action.* and button.* tokens

Item {
    id: ab
    required property var app
    property var item: ({})                 // an ItemOut (ActionsController.row(…).items[i])
    property string rowStyle: "tool"
    property string rowScale: "S"          // the row's scale: S | M (not `scale`: that is Item's)
    property string toggleLook: "pose"     // "pose" | "checked"
    property bool editing: false
    signal run(Item anchor)
    signal hold(Item anchor, bool down)          // a hold action (push to talk): the press and the release

    readonly property bool known: item.known === true
    readonly property bool dimmed: item.state !== "shown" || !known
    readonly property bool faint: item.state === "hidden"
    readonly property string look: item.look || "icon"
    readonly property string glyph: item.icon ? item.icon : (item.targetIcon ? item.targetIcon : "")
    readonly property string tip: item.tip ? item.tip : (item.label || "")
    readonly property string text: look === "icon" ? "" : (item.shortLabel ? item.shortLabel : (item.label || ""))
    readonly property bool active: item.active === true
    // the glyph: an emoji (a status preset's) before the icon, else the target's avatar (a room, a person)
    readonly property string emoji: look !== "label" && item.emoji ? item.emoji : ""
    readonly property bool avatar: emoji.length === 0 && glyph.length === 0 && look !== "label" && !!item.subject
                                   && (item.subjectKind === "room" || item.subjectKind === "person" || item.subjectKind === "space")
    readonly property bool compact: rowStyle === "compact"
    readonly property int iconSize: compact ? 16 : (rowScale === "M" ? 28 : Kirigami.Units.iconSizes.smallMedium)
    readonly property color ink: !known ? Th.actionPlaceholderFg : (item.danger === true ? Th.actionButtonFgDanger : Th.buttonToolFg)

    implicitWidth: loader.item ? loader.item.implicitWidth : 0
    implicitHeight: loader.item ? loader.item.implicitHeight : 0
    opacity: faint ? 0.25 : 1

    Loader {
        id: loader
        anchors.fill: parent
        // the header's round button only for the icon look (an icon + label / label look is the tool button, like the web)
        // the call bar's round CallButton for an icon (the web's CallBar does the same; an emoji, an avatar, a label,
        // a hold action or a newer Tin Hat's button is the tool button)
        sourceComponent: ab.item.primary === true && ab.rowStyle !== "header" ? primaryComp
                         : (ab.rowStyle === "header" && ab.glyph.length > 0 && ab.look === "icon" ? headerComp
                            : (ab.rowStyle === "call" && ab.glyph.length > 0 && ab.look === "icon" && ab.emoji.length === 0
                               && !ab.avatar && ab.item.hold !== true && ab.known ? callComp : toolComp))
    }

    Component {
        id: headerComp
        HeaderIconButton {
            app: ab.app
            iconName: ab.glyph
            tip: ab.tip
            accent: ab.active
            size: ab.rowScale === "M" ? 44 : Kirigami.Units.gridUnit * 2
            enabled: !ab.dimmed || ab.editing
            onClicked: if (!ab.editing && !ab.dimmed) ab.run(this)
        }
    }
    Component {
        id: callComp
        CallButton {
            app: ab.app
            iconName: ab.glyph
            tip: ab.tip
            active: ab.active
            danger: ab.item.danger === true
            live: !ab.dimmed || ab.editing
            onClicked: if (!ab.editing && !ab.dimmed) ab.run(this)
        }
    }
    Component {
        id: primaryComp
        GlowButton {
            app: ab.app
            text: ab.look === "icon" ? "" : (ab.item.label || "")
            icon.name: ab.glyph
            enabled: !ab.dimmed || ab.editing
            Controls.ToolTip.text: ab.tip
            Controls.ToolTip.visible: hovered && ab.tip.length > 0
            Controls.ToolTip.delay: 400
            onClicked: if (!ab.editing && !ab.dimmed) ab.run(this)
        }
    }
    Component {
        id: toolComp
        // ThToolButton's look (the style's background, focus ring, padding, tooltip) with an optional avatar for a
        // button that draws its target (a room, a person).
        Controls.ToolButton {
            id: tb
            display: Controls.AbstractButton.IconOnly
            Accessible.role: Accessible.Button
            Accessible.name: ab.tip
            Controls.ToolTip.text: ab.tip
            Controls.ToolTip.visible: hovered && ab.tip.length > 0
            Controls.ToolTip.delay: 400
            enabled: !ab.dimmed || ab.editing
            opacity: ab.dimmed && !ab.faint ? 0.5 : 1
            checkable: false
            checked: ab.toggleLook === "checked" && ab.active
            // S keeps the style's own size (ThToolButton's); M is at least 50 × 50, compact exactly 28 × 28
            implicitWidth: ab.compact ? 28 : Math.max(ab.rowScale === "M" ? 50 : 0, implicitBackgroundWidth + leftInset + rightInset,
                                                     implicitContentWidth + leftPadding + rightPadding)
            implicitHeight: ab.compact ? 28 : Math.max(ab.rowScale === "M" ? 50 : 0, implicitBackgroundHeight + topInset + bottomInset,
                                                      implicitContentHeight + topPadding + bottomPadding)
            Binding on padding { when: ab.compact || ab.rowScale === "M"; value: ab.compact ? 0 : 8 }
            onClicked: if (!ab.editing && !ab.dimmed && ab.item.hold !== true) ab.run(this)
            onPressedChanged: if (ab.item.hold === true && !ab.editing && !ab.dimmed) ab.hold(this, pressed)
            contentItem: RowLayout {
                spacing: ab.rowScale === "M" ? 6 : Kirigami.Units.smallSpacing
                Avatar {
                    visible: ab.avatar
                    Layout.preferredWidth: ab.iconSize
                    Layout.preferredHeight: ab.iconSize
                    Layout.alignment: Qt.AlignVCenter
                    app: ab.app
                    size: ab.iconSize
                    name: ab.item.targetName ? ab.item.targetName : (ab.item.label || "")
                    seed: ab.item.subject ? ab.item.subject : (ab.item.label || "")
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    visible: ab.emoji.length > 0
                    Layout.preferredWidth: ab.iconSize
                    Layout.preferredHeight: ab.iconSize
                    Layout.alignment: Qt.AlignCenter
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    text: ab.emoji
                    font.pixelSize: Math.round(ab.iconSize * 0.8)
                }
                ThIcon {
                    visible: ab.look !== "label" && !ab.avatar && ab.emoji.length === 0 && ab.glyph.length > 0
                    Layout.preferredWidth: ab.iconSize
                    Layout.preferredHeight: ab.iconSize
                    Layout.alignment: Qt.AlignCenter
                    app: ab.app
                    name: ab.glyph
                    color: ab.ink
                    isMask: true
                    hovered: tb.hovered
                    active: ab.active || tb.checked
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    visible: ab.text.length > 0
                    Layout.alignment: Qt.AlignVCenter
                    text: ab.text
                    color: ab.ink
                    font.bold: true
                }
            }
        }
    }
}
