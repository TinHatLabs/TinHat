// WidgetGallery.qml — "Add a widget" (contract WIDGETS-UI.md §7; the web's features/widgets/Gallery.tsx): the house
// Dialog, width 600, at most 80 % of the window tall (scrolls). One card per registry widget (padding 12, radius 12,
// gap 12): a 292-wide preview (padding 6, radius 8, ≤ 200 tall, clipped) showing the REAL widget at M, not
// interactive — or its sample state with a "PREVIEW" badge when there is no real data yet — and a column (gap 6) with
// the title, the description, the sizes and, at the bottom, [+ Add] or "✓ Added" + a flat "Remove". Add appends at
// the bottom at the widget's default size (it plays rowSlide); the dialog stays open.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.Dialog {
    id: dlg
    property var app
    property var hostRef
    title: "Add a widget"
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(600, (parent ? parent.width : 600) - Kirigami.Units.gridUnit * 2)
    height: Math.min(implicitHeight, (parent ? parent.height : 800) * 0.8)
    standardButtons: Controls.Dialog.Close
    readonly property real body: Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10

    contentItem: Flickable {
        id: gFlick
        implicitHeight: cards.implicitHeight
        contentHeight: cards.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        Controls.ScrollBar.vertical: FoxScrollBar { app: dlg.app; visible: gFlick.contentHeight > gFlick.height }
        Column {
            id: cards
            width: gFlick.width
            spacing: 10
            Repeater {
                model: dlg.opened && dlg.hostRef ? (dlg.hostRef.registry.widgets || []).filter(function (w) { return !dlg.app || !dlg.app.widgetOffered || dlg.app.widgetOffered(w.id) }) : []
                delegate: Rectangle {
                    id: card
                    required property var modelData
                    readonly property bool added: !!dlg.hostRef && dlg.hostRef.has(modelData.id)
                    width: cards.width
                    height: Math.max(previewBox.height, info.implicitHeight) + 24
                    radius: 12
                    color: Th.widgetGalleryCardBg
                    border.width: 1
                    border.color: cardHover.hovered ? Th.widgetGalleryCardBorderHover : Th.widgetGalleryCardBorder
                    HoverHandler { id: cardHover }
                    Rectangle {
                        id: previewBox
                        x: 12; y: 12
                        width: 292
                        height: Math.min(200, preview.height + 12)
                        radius: 8
                        color: Th.widgetGalleryPreviewBg
                        clip: true
                        WidgetFrame {
                            id: preview
                            x: 6; y: 6
                            width: 280
                            app: dlg.app
                            hostRef: dlg.hostRef
                            preview: true
                            widgetId: card.modelData.id
                            title: card.modelData.title
                            iconName: card.modelData.icon
                            sizes: card.modelData.sizes
                            maxH: card.modelData.maxH || ({ M: 220, L: 420 })
                            size: "M"
                            settings: ({})
                        }
                        MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; hoverEnabled: false }   // not interactive
                    }
                    ColumnLayout {
                        id: info
                        anchors.left: previewBox.right; anchors.leftMargin: 12
                        anchors.right: parent.right; anchors.rightMargin: 12
                        anchors.top: parent.top; anchors.topMargin: 12
                        anchors.bottom: parent.bottom; anchors.bottomMargin: 12
                        spacing: 6
                        // 11.1.1: the PREVIEW badge (no real data: the preview shows the widget's sample state) sits after the
                        // title here, not over the preview's own header, where it covered the widget's name (both apps)
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 8
                            Text {
                                Layout.fillWidth: !badge.visible
                                Layout.maximumWidth: info.width - (badge.visible ? badge.width + 8 : 0)
                                text: card.modelData.title
                                color: Th.widgetFg
                                font.family: Kirigami.Theme.defaultFont.family
                                font.pointSize: dlg.body * 1.1
                                font.weight: Font.DemiBold
                                textFormat: Text.PlainText
                                elide: Text.ElideRight
                            }
                            Rectangle {
                                id: badge
                                visible: !!(preview.body && preview.body.sample)
                                Layout.alignment: Qt.AlignVCenter
                                implicitHeight: 18; radius: 9
                                implicitWidth: badgeText.implicitWidth + 16
                                color: Th.widgetGalleryBadgeBg
                                WidgetText {
                                    id: badgeText
                                    anchors.centerIn: parent
                                    role: "label"
                                    text: "Preview"
                                    color: Th.widgetGalleryBadgeFg
                                }
                            }
                            Item { Layout.fillWidth: badge.visible }
                        }
                        WidgetText {
                            Layout.fillWidth: true
                            role: "meta"
                            text: card.modelData.description || ""
                            color: Th.widgetMetaFg
                            wrapMode: Text.Wrap
                            maximumLineCount: 4
                        }
                        WidgetText {
                            Layout.fillWidth: true
                            role: "meta"
                            text: (card.modelData.sizes || []).map(function (s) { return s === "S" ? "Small" : (s === "M" ? "Medium" : "Large") }).join(" · ")
                            color: Th.widgetMutedFg
                        }
                        Item { Layout.fillHeight: true }
                        RowLayout {
                            spacing: 8
                            RowLayout {
                                visible: card.added
                                spacing: 4
                                ThIcon {
                                    Layout.preferredWidth: 14; Layout.preferredHeight: 14
                                    app: dlg.app
                                    name: "object-select-symbolic"
                                    color: Th.widgetGalleryAddedFg
                                }
                                WidgetText { role: "meta"; semibold: true; text: "Added"; color: Th.widgetGalleryAddedFg }
                            }
                            ThButton {
                                visible: card.added
                                app: dlg.app
                                flat: true
                                text: "Remove"
                                onClicked: dlg.hostRef.removeId(card.modelData.id)
                            }
                            ThButton {
                                visible: !card.added
                                app: dlg.app
                                highlighted: true
                                text: "Add"
                                icon.name: "list-add-symbolic"
                                onClicked: dlg.hostRef.add(card.modelData.id)
                            }
                        }
                    }
                }
            }
        }
    }
}
