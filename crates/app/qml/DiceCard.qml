// DiceCard.qml — widgets W3 (2026-09-28): a roll as a card (WIDGETS-UI.md §16.3; the web's widgets/dice/DiceCard.tsx).
// The total big on the left, then who · when (the widget) or "rolled <expr>" (the chat), the dice faces — kept dice
// filled, dropped dice outlined and struck through, a subtracted group led by "−" — the modifier, and the Advantage /
// Disadvantage label; a natural 20 / 1 on the one kept d20 colours the total and that die and adds a "NAT 20" / "NAT 1"
// chip. An opaque card (widget.dice.card.*), so it reads the same in my bubble, in others' and in the widget.
//
// `r` = the card JSON (DiceController / RoomController: {expr, label, total, natural, faces: [{v, sides, kept, sign}],
// modifier, …}). The landing (motion `diceTumble`, WIDGETS-UI.md §16.5, GPU.md §1 (a): one-shot, the duration token): a
// new `playKey` (a roll I just made here) lands once while `animate` — the faces rotate ∓12° → 0 (alternating), scale
// 0.7 → 1 and fade in, the total with them, `verylong` 400 ms OutCubic; otherwise (and for every other card) the final
// card at once. No timer, nothing loops.
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Rectangle {
    id: card
    property var app
    property var r: null
    property string variant: "widget"          // "widget" | "chat"
    property string headWho: ""
    property string headWhen: ""
    property bool headPrivate: false
    property bool headFailed: false
    property int playKey: -1
    property bool animate: false
    property real maxW: 320

    readonly property bool chat: card.variant === "chat"
    readonly property var faces: card.r && card.r.faces ? card.r.faces : []
    readonly property int limit: card.chat ? card.faces.length : 8
    readonly property int more: Math.max(0, card.faces.length - card.limit)
    readonly property string natural: card.r ? String(card.r.natural || "") : ""
    readonly property string natText: card.natural === "nat20" ? "NAT 20" : card.natural === "nat1" ? "NAT 1" : ""
    readonly property int natIdx: {
        if (!card.natural) return -1
        for (var i = 0; i < card.faces.length; ++i) if (card.faces[i].sides === 20 && card.faces[i].kept) return i
        return -1
    }
    readonly property color natFg: card.natural === "nat20" ? Th.widgetDiceNat20Fg : Th.widgetDiceNat1Fg
    readonly property color natBg: card.natural === "nat20" ? Th.widgetDiceNat20Bg : Th.widgetDiceNat1Bg
    readonly property real body: Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10
    readonly property int faceSide: card.chat ? 18 : 20
    // the lines' width (from the card, never from the ColumnLayout itself: a child bound to its layout's width re-lays
    // it out recursively)
    readonly property real mainW: Math.max(0, card.width - 20 - Math.max(44, totalText.implicitWidth) - 10)

    // ── the landing (a roll I just made here) ──
    property real land: 1
    property int played: -1
    onPlayKeyChanged: card._maybePlay()
    Component.onCompleted: card._maybePlay()
    function _maybePlay() {
        // another card (someone else's roll, an older one) never lands
        if (card.playKey < 0) { landAnim.stop(); card.land = 1; return }
        if (card.played === card.playKey) return
        card.played = card.playKey
        landAnim.stop()
        card.land = 1
        if (card.animate && Motion.durVeryLong > 0) landAnim.restart()
    }
    NumberAnimation { id: landAnim; target: card; property: "land"; from: 0; to: 1; duration: Motion.durVeryLong; easing.type: Easing.OutCubic }

    /// the card's text for screen readers (the web's diceLabel)
    function label() {
        if (!card.r) return ""
        var kept = [], dropped = []
        for (var i = 0; i < card.faces.length; ++i) {
            var f = card.faces[i]
            if (f.kept) kept.push((f.sign < 0 ? "minus " : "") + f.v); else dropped.push(String(f.v))
        }
        var mod = String(card.r.modifier || "")
        var nat = card.natural === "nat20" ? ", natural 20" : card.natural === "nat1" ? ", natural 1" : ""
        return card.r.expr + (card.r.label ? " (" + card.r.label + ")" : "") + ": " + card.r.total + nat + " — dice " + kept.join(", ")
            + (mod.length > 0 ? ", " + (mod.charAt(0) === "-" ? "minus" : "plus") + " " + mod.replace(/^[+-]/, "") : "")
            + (dropped.length > 0 ? "; dropped " + dropped.join(", ") : "")
    }

    radius: 10
    color: Th.widgetDiceCardBg
    border.width: 1
    border.color: Th.widgetDiceCardBorder
    // chat: the natural width (the total column + the longer of the line and the faces on one line), 200 at least, the
    // bubble's inner width at most — from hidden measures only, so the Flow can wrap in it without a binding loop
    implicitWidth: card.chat ? Math.min(card.maxW, Math.max(200, 20 + Math.max(44, totalText.implicitWidth) + 10 + 4
        + Math.max(lineMeasure.implicitWidth + (card.natText.length > 0 ? 6 + natMeasure.implicitWidth + 10 : 0), facesMeasure.implicitWidth))) : 268
    implicitHeight: row.implicitHeight + 16
    Accessible.role: Accessible.StaticText
    Accessible.name: "Dice roll " + card.label()

    RowLayout {
        id: row
        x: 10; y: 8
        width: card.width - 20
        spacing: 10
        Text {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            id: totalText
            Layout.minimumWidth: 44
            Layout.alignment: Qt.AlignVCenter
            horizontalAlignment: Text.AlignHCenter
            text: card.r ? String(card.r.total) : ""
            opacity: card.land
            scale: 0.7 + 0.3 * card.land
            font.family: Kirigami.Theme.defaultFont.family
            font.pointSize: card.body * (card.chat ? 1.8 : 2.0)
            font.weight: Font.Bold
            font.features: ({ "tnum": 1 })
            color: card.natural === "nat20" ? Th.widgetDiceNat20Fg : card.natural === "nat1" ? Th.widgetDiceNat1Fg : Th.widgetDiceTotalFg
            Accessible.ignored: true
        }
        ColumnLayout {
            id: mainCol
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignVCenter
            spacing: 4
            // widget: who · when
            RowLayout {
                visible: !card.chat
                Layout.fillWidth: true
                spacing: 4
                ThIcon {
                    visible: card.headPrivate
                    Layout.preferredWidth: 12; Layout.preferredHeight: 12
                    Layout.alignment: Qt.AlignVCenter
                    app: card.app
                    name: "lock-symbolic"
                    color: Th.widgetMutedFg
                }
                WidgetText {
                    // no fillWidth (two fill items share the space and squeeze the name): its own width, elided to what's left
                    Layout.maximumWidth: Math.max(0, card.mainW - whenText.implicitWidth - 4 - (card.headPrivate ? 16 : 0))
                    role: card.headPrivate ? "text" : "strong"
                    text: card.headWho
                    color: card.headPrivate ? Th.widgetMutedFg : Th.widgetFg
                }
                WidgetText {
                    id: whenText
                    role: "num"
                    text: "· " + card.headWhen
                    color: card.headFailed ? Th.widgetDiceErrorFg : Th.widgetMetaFg
                }
                Item { Layout.fillWidth: true }
            }
            // chat: "rolled <expr> · Label" + the nat chip
            RowLayout {
                visible: card.chat
                Layout.fillWidth: true
                spacing: 6
                WidgetText {
                    Layout.maximumWidth: Math.max(0, card.mainW - (card.natText.length > 0 ? natMeasure.implicitWidth + 10 + 6 : 0))
                    role: "meta"
                    text: card.r ? "rolled " + card.r.expr + (card.r.label ? " · " + card.r.label : "") : ""
                    color: Th.widgetDiceMetaFg
                }
                NatChip { visible: card.chat && card.natText.length > 0 }
                Item { Layout.fillWidth: true }
            }
            // the faces (widget: one line, 8 then "+n"; chat: every die, wrapping)
            Flow {
                id: facesFlow
                Layout.fillWidth: true
                spacing: 4
                clip: !card.chat
                Accessible.ignored: true
                Repeater {
                    model: Math.min(card.faces.length, card.limit)
                    delegate: Row {
                        id: fb
                        required property int index
                        readonly property var f: card.faces[index]
                        readonly property bool lead: fb.f.sign < 0 && (fb.index === 0 || card.faces[fb.index - 1].sign > 0)
                        readonly property bool nat: fb.index === card.natIdx
                        spacing: 2
                        height: card.faceSide
                        WidgetText {
                            visible: fb.lead
                            anchors.verticalCenter: parent.verticalCenter
                            role: "meta"; semibold: true
                            text: "−"
                            color: Th.widgetDiceMetaFg
                        }
                        Rectangle {
                            width: card.faceSide; height: card.faceSide
                            radius: card.chat ? 4 : 5
                            antialiasing: true
                            rotation: (1 - card.land) * ((fb.index % 2) ? 12 : -12)
                            scale: 0.7 + 0.3 * card.land
                            opacity: card.land
                            color: fb.nat ? card.natBg : (fb.f.kept ? Th.widgetDiceFaceBg : "transparent")
                            border.width: 1
                            border.color: fb.nat ? card.natFg : (fb.f.kept ? Th.widgetDiceFaceBorder : Th.widgetDiceFaceDroppedBorder)
                            WidgetText {
                                anchors.centerIn: parent
                                role: "chip"
                                font.features: ({ "tnum": 1 })
                                font.strikeout: !fb.f.kept
                                text: String(fb.f.v)
                                color: fb.nat ? card.natFg : (fb.f.kept ? Th.widgetDiceFaceFg : Th.widgetDiceFaceDroppedFg)
                            }
                        }
                    }
                }
                WidgetText {
                    visible: card.more > 0
                    height: card.faceSide
                    role: "chip"
                    text: "+" + card.more
                    color: Th.widgetDiceMetaFg
                }
                WidgetText {
                    visible: !!card.r && String(card.r.modifier || "").length > 0
                    height: card.faceSide
                    role: "meta"; semibold: true
                    font.features: ({ "tnum": 1 })
                    text: card.r ? String(card.r.modifier || "") : ""
                    color: Th.widgetDiceMetaFg
                }
            }
            // widget: the expression · label + the nat chip
            RowLayout {
                visible: !card.chat
                Layout.fillWidth: true
                spacing: 6
                WidgetText {
                    Layout.maximumWidth: Math.max(0, card.mainW - (card.natText.length > 0 ? natMeasure.implicitWidth + 10 + 6 : 0))
                    role: "meta"
                    text: card.r ? card.r.expr + (card.r.label ? " · " + card.r.label : "") : ""
                    color: Th.widgetDiceMetaFg
                }
                NatChip { visible: !card.chat && card.natText.length > 0 }
                Item { Layout.fillWidth: true }
            }
        }
    }
    // the chat card's measures: its faces on one line (a face + the gap each, a "−" before a subtracted group, the
    // modifier) and its first line
    Row {
        id: facesMeasure
        visible: false
        spacing: 4
        Repeater {
            model: card.chat ? card.faces.length : 0
            delegate: Item {
                required property int index
                width: card.faceSide + ((card.faces[index].sign < 0 && (index === 0 || card.faces[index - 1].sign > 0)) ? 10 : 0)
                height: 1
            }
        }
        WidgetText { role: "meta"; semibold: true; text: card.r ? String(card.r.modifier || "") : "" }
    }
    WidgetText { id: lineMeasure; visible: false; role: "meta"; text: card.r ? "rolled " + card.r.expr + (card.r.label ? " · " + card.r.label : "") : "" }
    WidgetText { id: natMeasure; visible: false; role: "label"; text: card.natText }

    component NatChip: Rectangle {
        implicitWidth: natLabel.implicitWidth + 10
        implicitHeight: 14
        radius: 4
        color: card.natBg
        WidgetText {
            id: natLabel
            anchors.centerIn: parent
            role: "label"
            text: card.natText
            color: card.natFg
        }
    }
}
