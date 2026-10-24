// WidgetFrame.qml — widgets (modular UI step 2): one widget = one small island (contract WIDGETS-UI.md §2; the web's
// features/widgets/Frame.tsx). A 1 px `widget.border` drawn inside the island (radius 12, `widget.bg`, no shadow), a
// 30 px header (icon, title, the static live dot, the right-aligned "extra" slot, ⋯) over the body — a folded widget
// is 1 + 30 + 1 = 32 px tall. S and collapsed look the same: the header with the widget's one-line summary in the
// extra slot. The whole island is at most the widget's max height (M 220 / L 420); the body scrolls past it.
//
// Click the header (not ⋯) = the host decides: S → the default size, auto-folded → open it with priority, else
// collapse / expand. Press and move ≥ 4 px = reorder (the widget lifts, opacity 0.9, follows the pointer vertically;
// the host draws the drop line; release drops, Esc cancels). Keyboard: the header is a button (Enter / Space). An id
// this app does not know is kept and shown as "Needs a newer Tin Hat".
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: frame
    property var app
    property var hostRef                      // WidgetHost (window state, motion switch, controllers)
    property string widgetId: ""
    property bool known: true
    property string title: ""
    property string iconName: ""
    property var sizes: []
    property bool hasSettings: false
    property var maxH: ({ M: 220, L: 420 })
    property string size: "M"
    property var settings: ({})
    property bool userCollapsed: false
    property bool autoFolded: false
    property bool canMoveUp: false
    property bool canMoveDown: false
    property bool preview: false              // the gallery's live preview: no menu, no clicks
    property bool dragging: false

    signal sizeRequested(string size)
    signal headerClicked()
    signal removeRequested()
    signal moveRequested(int delta)
    signal settingsRequested()
    signal addRequested()
    signal dragStarted()
    signal dragMoved(real centerY)
    signal dragEnded(bool commit)

    readonly property bool collapsed: frame.userCollapsed || frame.autoFolded
    readonly property bool headerOnly: frame.known && !frame.preview && (frame.collapsed || frame.size === "S")
    readonly property var body: bodyLoader.item
    readonly property bool live: !!(frame.body && frame.body.live)
    readonly property int headerHeight: 30
    readonly property int foldedHeight: 32                                  // 1 + 30 + 1
    readonly property real maxTotal: frame.preview ? 200 - 12 : (frame.size === "L" ? (frame.maxH.L || 420) : (frame.maxH.M || 220))
    // the body: the widget's content + 5 bottom padding (6 from the island's edge with the border)
    readonly property real bodyNow: frame.known ? (frame.body ? Math.ceil(frame.body.implicitHeight) : 0) + 5 : unknownText.implicitHeight + 18
    // A folded body is hidden, and Layouts leave hidden children out of their implicit size — so a folded widget
    // reported a smaller body than it has open, and a stack sitting exactly at its height limit (the dock host gives
    // the widgets island its natural height) folded and unfolded that widget forever. The body counts at the height it
    // last had OPEN: recorded imperatively while shown (never a binding on the fold state, which the host's fold rule
    // itself decides — that would be a binding loop). Growth is recorded even while folded: hiding only ever shrinks a
    // body, so a larger one is real — a widget whose data arrives while the host has it auto-folded (widgets W3: the Dice
    // history after the room loads) otherwise kept its empty height and clipped the rest.
    property real bodyOpenNatural: 0
    onBodyNowChanged: if (!frame.headerOnly || frame.bodyNow > frame.bodyOpenNatural) frame.bodyOpenNatural = frame.bodyNow
    Component.onCompleted: if (!frame.headerOnly) frame.bodyOpenNatural = frame.bodyNow
    readonly property real bodyNatural: frame.bodyOpenNatural > 0 ? frame.bodyOpenNatural : frame.bodyNow
    readonly property real bodyHeight: Math.max(0, Math.min(frame.maxTotal - frame.foldedHeight, frame.bodyNatural))
    // the height when not auto-folded (the host folds bottom-up with it)
    readonly property real openHeight: frame.known && !frame.preview && (frame.size === "S" || frame.userCollapsed)
        ? frame.foldedHeight : frame.foldedHeight + frame.bodyHeight

    width: 280
    height: frame.foldedHeight + (frame.headerOnly ? 0 : frame.bodyHeight)
    opacity: frame.dragging ? 0.9 : 1
    z: frame.dragging ? 10 : 0
    Accessible.role: Accessible.Grouping
    Accessible.name: frame.known ? frame.title : "Widget"

    // rowSlide (a widget added from the gallery): opacity 0→1, y +4→0, long 200 ms OutCubic
    property real slideY: 0
    property real dragY: 0
    transform: Translate { y: frame.slideY + frame.dragY }
    function slideIn() {
        var d = frame.hostRef ? frame.hostRef.durLong : 0
        if (d <= 0) return
        frame.slideY = 4; island.opacity = 0
        slideAnim.duration = d
        slideAnim.restart()
    }
    ParallelAnimation {
        id: slideAnim
        property int duration: 200
        NumberAnimation { target: frame; property: "slideY"; to: 0; duration: slideAnim.duration; easing.type: Easing.OutCubic }
        NumberAnimation { target: island; property: "opacity"; to: 1; duration: slideAnim.duration; easing.type: Easing.OutCubic }
    }

    Rectangle {
        id: island
        anchors.fill: parent
        radius: 12
        color: Th.widgetBg
        border.width: 1
        border.color: Th.widgetBorder
        clip: true

        // the island owns its pixels (nothing underneath reacts)
        MouseArea { anchors.fill: parent; hoverEnabled: true; acceptedButtons: Qt.NoButton }

        // ── header (inside the 1 px frame) ──
        Item {
            id: header
            x: 1; y: 1
            width: parent.width - 2
            height: frame.headerHeight
            // hover wash: top corners follow the island (radius 11 inside the frame); a folded widget: all four
            Rectangle {
                anchors.fill: parent
                radius: 11
                visible: headMa.containsMouse && !frame.preview && frame.known
                color: Th.widgetHeaderBgHover
                Rectangle {   // square the bottom corners while the body shows
                    anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
                    height: parent.radius; color: parent.color
                    visible: !frame.headerOnly
                }
            }
            Rectangle {   // keyboard focus ring (1 px inside)
                anchors.fill: parent
                radius: 11
                color: "transparent"
                border.width: 1
                border.color: Th.widgetRingFocus
                visible: headMa.activeFocus
            }
            MouseArea {
                id: headMa
                anchors.fill: parent
                enabled: !frame.preview
                hoverEnabled: true
                activeFocusOnTab: !frame.preview
                cursorShape: Qt.PointingHandCursor
                Accessible.role: Accessible.Button
                Accessible.name: (frame.known ? frame.title : "Widget") + (frame.headerOnly ? " (collapsed)" : "")
                property real pressY: 0
                property bool moved: false
                // window coordinates: the frame follows the pointer, so its own coordinates would chase themselves
                onPressed: (m) => { pressY = headMa.mapToItem(null, m.x, m.y).y; moved = false }
                onPositionChanged: (m) => {
                    if (!pressed) return
                    var dy = headMa.mapToItem(null, m.x, m.y).y - pressY
                    if (!moved && Math.abs(dy) >= 4) { moved = true; frame.dragStarted() }
                    if (moved) {
                        // follow the pointer vertically, clamped to the stack
                        var stackH = frame.parent ? frame.parent.height : frame.height
                        var minY = -frame.y, maxY = stackH - frame.y - frame.height
                        frame.dragY = Math.max(minY, Math.min(maxY, dy))
                        frame.dragMoved(frame.y + frame.dragY + frame.height / 2)
                    }
                }
                onReleased: {
                    if (moved) { frame.dragY = 0; frame.dragEnded(true) }
                    else frame.headerClicked()
                    moved = false
                }
                onCanceled: { if (moved) { frame.dragY = 0; frame.dragEnded(false) } moved = false }
                Keys.onPressed: (e) => {
                    if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter || e.key === Qt.Key_Space) { frame.headerClicked(); e.accepted = true }
                    else if (e.key === Qt.Key_Escape && moved) { moved = false; frame.dragY = 0; frame.dragEnded(false); e.accepted = true }
                }
            }
            // [9 | icon 16 | 6 | title | 5 | dot 6 | 6 | extra (fills, right-aligned) | 6 | ⋯ 24 | 2]
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 9; anchors.rightMargin: 2
                spacing: 0
                ThIcon {
                    Layout.preferredWidth: 16; Layout.preferredHeight: 16
                    Layout.alignment: Qt.AlignVCenter
                    app: frame.app
                    name: frame.known ? frame.iconName : "dialog-question-symbolic"
                    color: Th.widgetHeaderIcon
                }
                WidgetText {
                    Layout.leftMargin: 6
                    Layout.alignment: Qt.AlignVCenter
                    role: "title"
                    elide: Text.ElideNone
                    text: frame.known ? frame.title : "Widget"
                    color: Th.widgetHeaderFg
                }
                Rectangle {   // the live dot: static, never pulses
                    Layout.leftMargin: 5
                    Layout.alignment: Qt.AlignVCenter
                    visible: frame.live && frame.known
                    implicitWidth: 6; implicitHeight: 6; radius: 3
                    color: Th.widgetLiveDot
                }
                // the extra slot (right-aligned, elided): folded / S = the widget's summary; open = its header extra
                Item {
                    id: extraSlot
                    Layout.fillWidth: true
                    Layout.leftMargin: 6
                    Layout.fillHeight: true
                    clip: true
                    Loader {
                        id: extraLoader
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        width: item ? Math.min(item.implicitWidth, extraSlot.width) : 0
                        height: item ? item.implicitHeight : 0
                        readonly property var comp: !frame.known || !frame.body ? null
                            : (frame.headerOnly ? (frame.body.summaryComponent || null) : (frame.body.extraComponent || null))
                        active: !!comp
                        sourceComponent: comp
                    }
                }
                Controls.AbstractButton {
                    id: menuBtn
                    Layout.leftMargin: 6
                    Layout.alignment: Qt.AlignVCenter
                    visible: !frame.preview
                    implicitWidth: 24; implicitHeight: 24
                    hoverEnabled: true
                    focusPolicy: Qt.TabFocus
                    Accessible.name: (frame.known ? frame.title : "Widget") + " options"
                    Controls.ToolTip.text: "Widget options"
                    Controls.ToolTip.visible: hovered && !menu.opened
                    Controls.ToolTip.delay: 500
                    background: Rectangle {
                        radius: 6
                        color: (menuBtn.hovered || menuBtn.pressed || menu.opened) ? Th.widgetMenuBgHover : "transparent"
                        border.width: menuBtn.visualFocus ? 1 : 0
                        border.color: Th.widgetRingFocus
                    }
                    contentItem: Item {
                        ThIcon {
                            anchors.centerIn: parent
                            width: 16; height: 16
                            app: frame.app
                            name: "overflow-menu-symbolic"
                            color: Th.widgetMenuIcon
                        }
                    }
                    onClicked: menu.opened ? menu.close() : menu.popup(menuBtn, 0, menuBtn.height)
                }
            }
        }

        // ── body: padding 0 5 5 inside the frame (the widgets lay their rows out from x = 6) ──
        Flickable {
            id: bodyFlick
            x: 0; y: 1 + frame.headerHeight
            width: parent.width
            height: frame.bodyHeight
            visible: !frame.headerOnly && frame.known
            contentHeight: frame.bodyNatural
            interactive: frame.bodyNatural > frame.bodyHeight + 0.5 && !frame.preview
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            Controls.ScrollBar.vertical: FoxScrollBar { app: frame.app; visible: bodyFlick.interactive }
            Loader {
                id: bodyLoader
                width: bodyFlick.width
                active: frame.known
                sourceComponent: frame.widgetId === "upNext" ? upNextC
                               : frame.widgetId === "callStats" ? callStatsC
                               : frame.widgetId === "whosAround" ? whosAroundC
                               : frame.widgetId === "gameServers" ? gameServersC
                               : frame.widgetId === "pinned" ? pinnedC
                               : frame.widgetId === "mentions" ? mentionsC
                               : frame.widgetId === "note" ? noteC
                               : frame.widgetId === "clocks" ? clocksC
                               : frame.widgetId === "dice" ? diceC
                               : frame.widgetId === "polls" ? pollsC
                               : frame.widgetId === "justMe" ? justMeC
                               : frame.widgetId === "memories" ? memoriesC
                               : frame.widgetId === "soundPad" ? soundPadC
                               : frame.widgetId === "gameNight" ? gameNightC : null
            }
            // `expand`: the body fades in (dur 120, OutCubic) when the widget opens; collapse snaps
            NumberAnimation { id: expandAnim; target: bodyFlick; property: "opacity"; from: 0; to: 1; easing.type: Easing.OutCubic }
        }
        WidgetText {
            id: unknownText
            visible: !frame.known
            x: 6; y: 1 + frame.headerHeight + 8
            width: parent.width - 12
            role: "meta"
            text: "Needs a newer Tin Hat"
            color: Th.widgetUnknownFg
            wrapMode: Text.Wrap
        }
    }
    onHeaderOnlyChanged: {
        var d = frame.hostRef ? frame.hostRef.dur : 0
        if (!frame.headerOnly && d > 0) { expandAnim.duration = d; expandAnim.restart() }
        else { expandAnim.stop(); bodyFlick.opacity = 1 }
    }

    Component { id: upNextC; UpNextWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    Component { id: callStatsC; CallStatsWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; preview: frame.preview } }
    Component { id: whosAroundC; WhosAroundWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    Component { id: gameServersC; GameServersWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    // widgets W1 (2026-09-28)
    Component { id: pinnedC; PinnedWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    Component { id: mentionsC; MentionsWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    Component { id: noteC; NoteWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    Component { id: clocksC; ClocksWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    // widgets W3 (2026-09-28)
    Component { id: diceC; DiceWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    Component { id: pollsC; PollsWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    // widgets W2 (2026-09-28)
    Component { id: justMeC; JustMeWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    Component { id: memoriesC; MemoriesWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    Component { id: soundPadC; SoundPadWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }
    Component { id: gameNightC; GameNightWidget { app: frame.app; hostRef: frame.hostRef; size: frame.size; settings: frame.settings; preview: frame.preview } }

    // ── ⋯ menu (WIDGETS-UI.md §2, the web's Frame menu): Size · — · Collapse | Expand · Settings… · Move up · Move down
    //    · — · Add widget… · Remove (an unknown widget: Move up · Move down · — · Remove) ──
    Controls.Menu {
        id: menu
        modal: false
        readonly property bool manySizes: frame.known && frame.sizes.length > 1
        MenuSectionLabel {
            text: "Size"
            visible: menu.manySizes
            height: visible ? implicitHeight : 0
        }
        Repeater {
            model: menu.manySizes ? [{ k: "S", t: "Small" }, { k: "M", t: "Medium" }, { k: "L", t: "Large" }].filter(function (s) { return frame.sizes.indexOf(s.k) >= 0 }) : []
            delegate: CheckMenuItem {
                required property var modelData
                text: modelData.t
                isChecked: () => frame.size === modelData.k
                onTriggered: frame.sizeRequested(modelData.k)
            }
        }
        Controls.MenuSeparator { visible: menu.manySizes; height: visible ? implicitHeight : 0 }
        Controls.MenuItem {
            text: frame.headerOnly ? "Expand" : "Collapse"
            icon.name: frame.headerOnly ? "go-down-symbolic" : "go-up-symbolic"
            visible: frame.known && frame.size !== "S"
            height: visible ? implicitHeight : 0
            onTriggered: frame.headerClicked()
        }
        Controls.MenuItem {
            text: "Settings…"
            icon.name: "configure-symbolic"
            visible: frame.known && frame.hasSettings
            height: visible ? implicitHeight : 0
            onTriggered: frame.settingsRequested()
        }
        Controls.MenuItem { text: "Move up"; icon.name: "go-up-symbolic"; enabled: frame.canMoveUp; onTriggered: frame.moveRequested(-1) }
        Controls.MenuItem { text: "Move down"; icon.name: "go-down-symbolic"; enabled: frame.canMoveDown; onTriggered: frame.moveRequested(1) }
        Controls.MenuSeparator {}
        Controls.MenuItem {
            text: "Add widget…"
            icon.name: "list-add-symbolic"
            visible: frame.known
            height: visible ? implicitHeight : 0
            onTriggered: frame.addRequested()
        }
        Controls.MenuItem {
            text: "Remove"
            icon.name: "list-remove-symbolic"
            icon.color: Th.menuItemDangerFg
            palette.windowText: Th.menuItemDangerFg
            palette.text: Th.menuItemDangerFg
            Kirigami.Theme.inherit: false
            Kirigami.Theme.textColor: Th.menuItemDangerFg
            onTriggered: frame.removeRequested()
        }
    }
}
