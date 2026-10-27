// Island.qml — one island's frame in the dock host (ISLANDS-ARCH §2.3, §3.4). The host writes its geometry (x, y, width,
// height, z, visible) from the solver in one imperative pass; the frame draws the island look from the theme's shape: the
// fill (island.bg.<ref>) UNDER the content, the 1 px frame (island.border) ABOVE it — content that paints to its edge
// never covers the frame — and the corner radius; the content sits inset by ceil(0.293 × radius), so square-clipped
// content never pokes out of a rounded corner (`contentInset: 0` = the content rounds itself, e.g. the header's
// gradient). `flush: true` draws nothing and insets nothing (the widget stack: every widget is its own card).
// objectName "island:<ref>" for the gates. No layer, no effect items, ever (GPU.md §2.9).
//
// The content (the one child) reports its natural size to the host through `naturalHeight` / `naturalWidth` (its implicit
// size + the inset; the host quantizes them, §2.1) and optional extra measurements through `extra` (the widgets' minH);
// `empty` = the island has nothing to show (the DM strip with no DMs, no widgets). An island in the nav drawer
// (`inDrawer`, set by the host) follows the drawer panel's fade and slide and reports its hover to the drawer's intent.
import QtQuick
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the island tokens and the theme's shape

Item {
    id: frame
    required property string ref
    property bool flush: false            // draw nothing, no inset
    // the host's shape (the theme's, or Flat: 0 / 0) — Th's shape when the frame has no dock host as its parent
    property real radius: (frame.parent && frame.parent.islandRadius !== undefined) ? frame.parent.islandRadius : Th.shapeRadius
    property int borderWidth: (frame.parent && frame.parent.islandBorder !== undefined) ? frame.parent.islandBorder : Th.shapeBorder
    property color fill: "transparent"
    property color borderColor: Th.islandBorder
    property int contentInset: -1          // -1 = ceil(0.293 × radius)
    property var extra: ({})              // extra measurements the content reports (the widgets: minH)
    property bool empty: false
    property bool reports: true           // false = unmeasured (the registry's pref / minimum applies)
    // 10.8: the natural height is also the island's minimum (content.minH, LAYOUT.md §7.4) — the composer: a reply bar and
    // several lines are never squeezed to the registry's 48 px while a call or the timeline take the room
    property bool minIsNatural: false
    property bool inDrawer: false         // set by the host: the island sits in the nav drawer
    default property alias content: holder.data
    objectName: "island:" + ref

    readonly property Item contentItem: holder.children.length > 0 ? holder.children[0] : null
    readonly property int inset: frame.flush ? 0 : (frame.contentInset >= 0 ? frame.contentInset : Math.ceil(0.293 * frame.radius))
    readonly property real naturalHeight: frame.contentItem ? frame.contentItem.implicitHeight + 2 * frame.inset : 0
    readonly property real naturalWidth: frame.contentItem ? frame.contentItem.implicitWidth + 2 * frame.inset : 0

    // the nav drawer: ride its panel's fade + slide (the host passes them on), keep it open while hovered
    opacity: frame.inDrawer ? frame.parent.drawerOpacity : 1
    transform: Translate { x: frame.inDrawer ? frame.parent.drawerShift : 0 }
    HoverHandler {
        enabled: frame.inDrawer
        onHoveredChanged: frame.parent.drawerHover(frame.ref, hovered)
    }

    Rectangle {   // the fill, under the content
        anchors.fill: parent
        visible: !frame.flush
        radius: frame.radius
        color: frame.fill
    }
    Item {
        id: holder
        // never a negative size: an island the layout has not placed yet (a room island before the first room) is 0 x 0,
        // and margins would hand its content -2 x inset — layouts and positioners thrash on negative sizes
        x: frame.inset
        y: frame.inset
        width: Math.max(0, frame.width - 2 * frame.inset)
        height: Math.max(0, frame.height - 2 * frame.inset)
    }
    // 11.0 edit mode (EDITMODE-UI §2): a placeholder where the island's real content is absent (no call, no room, the
    // panel closed, no unread DMs, no widgets) — drawn by the island itself, above its content and under its frame, so
    // a floating island covers it the way it covers content. Nothing exists outside edit mode (an inactive Loader).
    readonly property bool _placeholder: !!frame.parent && frame.parent.editing === true && !!frame.parent.absentRefs
                                         && frame.parent.absentRefs.indexOf(frame.ref) >= 0
    Loader {
        anchors.fill: parent
        active: frame._placeholder
        sourceComponent: Component {
            Rectangle {
                id: ph
                readonly property var t: (frame.parent && frame.parent.placeholderTextOf) ? frame.parent.placeholderTextOf(frame.ref) : ({ icon: "", name: frame.ref, absent: "" })
                readonly property bool isShort: frame.height < 72
                radius: frame.radius
                color: Th.editmodePlaceholderBg
                Column {
                    anchors.centerIn: parent
                    width: parent.width - 16
                    spacing: 6
                    visible: !ph.isShort
                    Kirigami.Icon {
                        anchors.horizontalCenter: parent.horizontalCenter
                        width: 24; height: 24
                        source: ph.t.icon
                        color: Th.editmodePlaceholderFg
                        isMask: true
                    }
                    Text {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        width: parent.width
                        horizontalAlignment: Text.AlignHCenter
                        elide: Text.ElideRight
                        font.pixelSize: 13
                        font.weight: Font.DemiBold
                        color: Th.editmodePlaceholderFg
                        text: ph.t.name
                    }
                    Text {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        width: parent.width
                        visible: ph.t.absent.length > 0
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        maximumLineCount: 2
                        elide: Text.ElideRight
                        font.pixelSize: 12
                        color: Th.editmodePlaceholderFg
                        text: ph.t.absent
                    }
                }
                Row {   // short islands (h < 72): the name and the line on one row, no icon
                    anchors.centerIn: parent
                    visible: ph.isShort
                    spacing: 8
                    Text { textFormat: Text.PlainText; anchors.verticalCenter: parent.verticalCenter; font.pixelSize: 13; font.weight: Font.DemiBold; color: Th.editmodePlaceholderFg; text: ph.t.name }
                    Text { textFormat: Text.PlainText; anchors.verticalCenter: parent.verticalCenter; font.pixelSize: 12; color: Th.editmodePlaceholderFg; text: ph.t.absent; visible: ph.t.absent.length > 0 }
                }
            }
        }
    }
    Rectangle {   // the frame, above the content
        anchors.fill: parent
        visible: !frame.flush && frame.borderWidth > 0
        radius: frame.radius
        color: "transparent"
        border.width: frame.borderWidth
        border.color: frame.borderColor
    }
}
