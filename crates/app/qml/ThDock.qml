// ThDock.qml — the reusable tuck-away corner shell for Tin Hat, ported from
// the earlier web client's corner dock: an always-visible corner handle that
// reveals a floating panel on hover (with hover-intent + auto-hide delays) or a
// click-to-pin. Two of these make the signature nav: top-LEFT spaces→rooms
// (the nav dock) and top-RIGHT utility (the utility dock).
//
// This component is pure chrome: it owns the handle, the slide/fade panel, and the
// open/close/pin lifecycle. The caller fills the panel via default children and can
// call `close()` after acting on a row. It fills its parent but is input-transparent
// except over the handle and the open panel, so the app underneath stays usable.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the dock.* tokens

Item {
    id: dock
    required property var app          // ApplicationWindow root (theme tokens)
    property string side: "left"       // "left" | "right" — which corner to dock to
    property string handleIcon: "go-home-symbolic"
    property string badgeText: ""      // optional handle badge (unread count)
    property bool badgeHighlight: false
    property string ariaLabel: ""
    property real panelWidth: Kirigami.Units.gridUnit * 18
    // When true the panel extends to (near) the window bottom so a fill-height list
    // can use the space — a ListView reports implicitHeight 0 inside a Layout, so a
    // content-sized panel would collapse to its fixed header. Utility-style docks with
    // only fixed rows leave this false and size to content.
    property bool fillPanelHeight: false
    // A determinate panel height (px). When >0 the panel is exactly this tall (capped to
    // the window) regardless of content — used for the corner nav so it "hovers over the
    // corner" at a fixed size instead of running floor-to-ceiling.
    property real panelHeight: 0
    // 11.0 (islands C6/C7): where the dock host's solver put the drawer — its handle and its panel (the islands sit in
    // the panel); an empty rect = the corner defaults (smallSpacing off the corner, panelWidth × the panel height rule).
    property rect handleRect: Qt.rect(0, 0, 0, 0)
    property rect panelRect: Qt.rect(0, 0, 0, 0)
    readonly property bool _placed: dock.handleRect.width > 0 && dock.panelRect.width > 0

    // ---- open/pin lifecycle ----
    property bool opened: false
    property bool pinned: false
    property bool handleHover: false
    property bool panelHovered: false
    // 11.0 (islands C5): the drawer's island sits ABOVE the panel frame (a sibling in the dock host, not a child),
    // so the panel's own HoverHandler loses the pointer to it — the island reports its hover here.
    property bool islandHovered: false
    property bool nearTop: false       // pointer near the docked top corner (OLED-burn-in handle reveal)
    function close() { dock.pinned = false; dock.opened = false }
    // 10.8: the handle item — the dock host hands the focus back to it when the drawer closes around the focus
    // (ISLANDS-ARCH §2.7: the invoker first), and it is a keyboard stop of its own (Tab, Enter / Space = a click)
    readonly property Item handleItem: handle
    function _toggleFromHandle() { dock.pinned = !dock.pinned; dock.opened = dock.pinned || dock.opened }

    readonly property bool isLeft: side === "left"
    readonly property var _a: app

    anchors.fill: parent
    z: 1000

    function _evalHover() {
        if (handleHover || panelHovered || islandHovered || pinned) {
            closeT.stop()
            if (!opened) openT.restart()
        } else {
            openT.stop()
            closeT.restart()
        }
    }
    onHandleHoverChanged: _evalHover()
    onPanelHoveredChanged: _evalHover()
    onIslandHoveredChanged: _evalHover()
    onPinnedChanged: _evalHover()

    Timer { id: openT; interval: 110; onTriggered: dock.opened = true }
    Timer {
        id: closeT; interval: 280
        onTriggered: if (!dock.pinned && !dock.handleHover && !dock.panelHovered) dock.opened = false
    }

    // Top-edge reveal zone (OLED burn-in): hovering near the docked top corner fades the
    // handle in, mirroring the fork's useNearTop(64), so the handle can rest fully hidden.
    // Input-transparent (HoverHandler only) so clicks pass through to the app beneath.
    Item {
        anchors.top: parent.top
        anchors.left: dock.isLeft ? parent.left : undefined
        anchors.right: dock.isLeft ? undefined : parent.right
        width: Math.min(parent.width, Kirigami.Units.gridUnit * 12)
        height: 64
        HoverHandler { onHoveredChanged: dock.nearTop = hovered }
    }

    // ---- the corner handle (revealed on hover/open/pin/near-top; hidden at rest for OLED) ----
    Rectangle {
        id: handle
        x: dock._placed ? dock.handleRect.x : (dock.isLeft ? Kirigami.Units.smallSpacing : dock.width - width - Kirigami.Units.smallSpacing)
        y: dock._placed ? dock.handleRect.y : Kirigami.Units.smallSpacing
        width: dock._placed ? dock.handleRect.width : 40
        height: dock._placed ? dock.handleRect.height : 40
        radius: dock._a.radius

        color: (dock.opened || handleMa.containsMouse || handle.activeFocus)
            ? Th.dockHandleBgActive
            : Th.dockHandleBg
        border.width: 1
        border.color: Th.dockHandleBorder
        // OLED burn-in: rest fully hidden; reveal on hover / open / pin / pointer-near-top.
        // (ThDock is corner mode only — the persistent sidebar uses ThNav directly.)
        opacity: (dock.opened || dock.handleHover || dock.pinned || dock.nearTop || handle.activeFocus) ? 1 : 0   // (focused: shown)
        Behavior on opacity { NumberAnimation { duration: dock._a.dur } }
        Behavior on color { ColorAnimation { duration: dock._a.dur } }

        Kirigami.Icon {
            anchors.centerIn: parent
            width: 20; height: 20
            source: dock.handleIcon
            color: Th.dockHandleIcon
        }

        // Pin indicator — a tiny dot in the corner when the dock is pinned open.
        Rectangle {
            visible: dock.pinned
            width: 7; height: 7; radius: 3.5
            color: Th.dockPinBg
            border.width: 1; border.color: Th.dockPinBorder
            anchors.right: parent.right; anchors.bottom: parent.bottom
            anchors.rightMargin: 3; anchors.bottomMargin: 3
        }

        // Unread badge (nav dock only; hidden when blank — badgeText is "", "1".."99" or "99+").
        UnreadBadge {
            app: dock._a
            // badgeText is pre-capped by Rust ("99+"): parseInt would eat the '+' and
            // display "99"; anything with a '+' means >99, which UnreadBadge re-caps.
            count: dock.badgeText.indexOf("+") >= 0 ? 100 : (parseInt(dock.badgeText, 10) || 0)
            highlight: dock.badgeHighlight
            border.width: 1.5; border.color: Th.dockBadgeBorder
            anchors.right: parent.right; anchors.top: parent.top
            anchors.rightMargin: -4; anchors.topMargin: -4
        }

        HoverHandler { id: handleHh; onHoveredChanged: dock.handleHover = hovered }
        MouseArea {
            id: handleMa
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            // Click toggles pin (so it stays open while you work); also opens instantly.
            onClicked: dock._toggleFromHandle()
        }
        activeFocusOnTab: true
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) { dock._toggleFromHandle(); event.accepted = true }
        }
        Accessible.role: Accessible.Button
        Accessible.name: dock.ariaLabel
        Accessible.onPressAction: dock._toggleFromHandle()
        Controls.ToolTip.text: dock.ariaLabel
        Controls.ToolTip.visible: handleMa.containsMouse && dock.ariaLabel.length > 0 && !dock.opened
        Controls.ToolTip.delay: 500
    }

    // ---- the reveal panel ----
    Rectangle {
        id: panel
        width: dock._placed ? dock.panelRect.width : dock.panelWidth
        x: dock._placed ? dock.panelRect.x : (dock.isLeft ? Kirigami.Units.smallSpacing : dock.width - width - Kirigami.Units.smallSpacing)
        y: dock._placed ? dock.panelRect.y : handle.y + handle.height + Kirigami.Units.smallSpacing
        // Cap height so the panel never runs off the bottom of the window. Fill-height
        // docks (room lists) take the whole available column; content docks size to fit.
        readonly property real availH: dock.height - handle.height - Kirigami.Units.gridUnit * 3
        height: dock._placed ? dock.panelRect.height
            : dock.fillPanelHeight ? availH
            : (dock.panelHeight > 0 ? Math.min(dock.panelHeight, availH)
                                    : Math.min(implicitHeight, availH))
        implicitHeight: panelColumn.implicitHeight + Kirigami.Units.largeSpacing * 2

        radius: dock._a.radiusLg
        // 10.8: the nav drawer panel's own tokens (drawer.panel.* — by default the dock.panel.* values, so OLED/Goose and
        // every theme without its own drawer colours paint as before); the shadow keeps dock.panel.shadow (no drawer token)
        color: Th.drawerPanelBg
        border.width: 1
        border.color: Th.drawerPanelBorder

        visible: dock.opened || opacity > 0.01
        opacity: dock.opened ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: dock._a.dur; easing.type: Easing.OutCubic } }
        // Slide in from the docked edge + fade.
        transform: Translate {
            id: panelSlide
            x: dock.opened ? 0 : (dock.isLeft ? -16 : 16)
            Behavior on x { NumberAnimation { duration: dock._a.dur; easing.type: Easing.OutCubic } }
        }

        // Soft drop shadow so the floating panel reads above the page beneath it.
        Kirigami.ShadowedRectangle {
            anchors.fill: parent
            radius: parent.radius
            color: "transparent"
            shadow.size: 18
            shadow.color: Th.dockPanelShadow
            shadow.yOffset: 4
            z: -1
        }

        HoverHandler { id: panelHh; onHoveredChanged: dock.panelHovered = hovered }

        // Caller-provided content lives in this column.
        ColumnLayout {
            id: panelColumn
            anchors.fill: parent
            anchors.margins: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.smallSpacing
        }
    }

    // Expose the content holder so subclasses can reparent into it.
    default property alias content: panelColumn.data
    readonly property alias panelItem: panel
    // 11.0 (islands C5): the panel is on screen (open, or still fading out) and its slide offset — the drawer's
    // island follows both (it is placed by the dock host, not parented to the panel)
    readonly property bool panelShown: panel.visible
    readonly property real panelShift: panelSlide.x
}
