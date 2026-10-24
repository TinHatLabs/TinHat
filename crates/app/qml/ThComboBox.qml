// ThComboBox — the house combo box (0.8.33; rebuilt on QtQuick.Templates in 0.8.34).
//
// Built on the TEMPLATES (the style-less base types), not on QtQuick.Controls: a `Controls.X` is
// whatever QQC2 style the machine happens to run — Basic on Windows and the build rig, KDE's
// breeze on a KDE Plasma desktop — and every style adds its own handlers, tooltips, insets and
// popups underneath our visuals. Under breeze the hovered dropdown row vanished and the click
// never selected ("all drop downs break once hovered"). A template carries the API (model /
// textRole / currentIndex / editable / editText / onActivated / displayText …) and NOTHING
// else; every visual is ours: dark rounded frame like LoginField, a drawn chevron, a DARK
// dropdown list — identical on every machine, whatever style it runs. Colours: the combo.* tokens of Th.
import QtQuick
import QtQuick.Templates as T
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

T.ComboBox {
    id: box
    property real radius: 8

    implicitHeight: 34
    implicitWidth: Math.max(120, implicitContentWidth + leftPadding + rightPadding)
    leftPadding: 10
    rightPadding: 30
    hoverEnabled: true
    font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale   // ENGINE.md §10: the theme's text size

    // 0.8.36: an EDITABLE combo. Qt only pushes the picked row's text into editText when
    // currentIndex CHANGES (QQuickComboBoxPrivate::setCurrentIndex returns early otherwise), so a
    // click on the row that is already current — the first game, for every new event — closed the
    // list and left the field as it was. `activated` fires either way: take the row's text here.
    onActivated: (index) => {
        if (!editable) return
        var t = textAt(index)
        if (t !== undefined && t !== null && editText !== t) editText = t
    }

    // ---- the closed box
    background: Rectangle {
        opacity: parent && parent.enabled === false ? 0.5 : 1.0   // 10.1 (TH-DS-09)
        radius: box.radius
        color: box.activeFocus || box.popup.visible ? Th.comboBgFocus : (box.hovered ? Th.comboBgHover : Th.comboBg)
        border.width: box.activeFocus || box.popup.visible ? 2 : 1
        border.color: box.activeFocus || box.popup.visible ? Th.comboBorderFocus : Th.comboBorder
        Behavior on color { ColorAnimation { duration: Motion.dur } }
        Behavior on border.color { ColorAnimation { duration: Motion.dur } }
    }
    contentItem: T.TextField {
        // The template pattern: a text field that is read-only unless the combo is editable.
        implicitHeight: contentHeight + topPadding + bottomPadding
        text: box.editable ? box.editText : box.displayText
        enabled: box.editable
        autoScroll: box.editable
        readOnly: box.down
        inputMethodHints: box.inputMethodHints
        validator: box.validator
        selectByMouse: box.selectTextByMouse
        font: box.font
        color: Th.comboFg
        selectionColor: Th.inputSelectionBg
        selectedTextColor: Th.inputSelectionFg
        verticalAlignment: Text.AlignVCenter
        leftPadding: 0; rightPadding: 0; topPadding: 0; bottomPadding: 0
    }
    indicator: Canvas {
        id: chev
        x: box.width - width - 11
        y: (box.height - height) / 2
        width: 11; height: 7
        rotation: box.popup.visible ? 180 : 0
        Behavior on rotation { NumberAnimation { duration: Motion.dur } }
        onPaint: {
            var c = getContext("2d"); c.clearRect(0, 0, width, height)
            c.strokeStyle = Th.comboIcon; c.lineWidth = 1.8; c.lineCap = "round"; c.lineJoin = "round"
            c.beginPath(); c.moveTo(1.2, 1.5); c.lineTo(5.5, 5.5); c.lineTo(9.8, 1.5); c.stroke()
        }
        Component.onCompleted: requestPaint()
        Connections { target: Th; function onRevisionChanged() { chev.requestPaint() } }   // a theme switch repaints the chevron
    }

    // ---- the dropdown
    delegate: T.ItemDelegate {
        id: row
        required property var model
        required property var modelData
        required property int index
        width: ListView.view ? ListView.view.width : box.width
        height: 30
        text: box.textRole.length > 0
              ? String(Array.isArray(box.model) ? modelData[box.textRole] : model[box.textRole])
              : String(modelData)
        highlighted: box.highlightedIndex === index
        hoverEnabled: true
        padding: 0
        contentItem: Text {
            text: row.text
            color: Th.comboItemFg
            font.pointSize: box.font.pointSize
            font.weight: box.currentIndex === row.index ? Font.DemiBold : Font.Normal
            elide: Text.ElideRight
            verticalAlignment: Text.AlignVCenter
            leftPadding: 10; rightPadding: 10
        }
        background: Rectangle {
            radius: 6
            color: row.highlighted ? Th.comboItemBgSelected
                 : (row.hovered ? Th.comboItemBgHover : "transparent")
        }
    }
    popup: T.Popup {
        id: dd
        y: box.height + 4
        width: Math.max(box.width, 140)
        implicitHeight: Math.min(list.contentHeight + 12, 330)
        padding: 6
        contentItem: ListView {
            id: list
            clip: true
            implicitHeight: contentHeight
            model: dd.visible ? box.delegateModel : null
            currentIndex: box.highlightedIndex
            highlightMoveDuration: 0
            T.ScrollIndicator.vertical: T.ScrollIndicator {
                id: vind
                implicitWidth: 6
                padding: 2
                contentItem: Rectangle {
                    implicitWidth: 2
                    radius: 1
                    color: Th.comboListThumb
                    visible: vind.size < 1.0
                    opacity: vind.active ? 0.6 : 0.0
                    Behavior on opacity { NumberAnimation { duration: Motion.durLong } }
                }
            }
        }
        background: Rectangle {
            color: Th.comboListBg
            radius: 10
            border.width: 1
            border.color: Th.comboListBorder
        }
        enter: Transition { NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Motion.durShort } }
        exit: Transition { NumberAnimation { property: "opacity"; from: 1; to: 0; duration: Motion.durShort } }
    }
}
