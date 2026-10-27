// LinkPreviewCard.qml — the card under a message that carries an ordinary link (9.3).
//
// Renders one preview descriptor from room.rs / linkpreview.rs:
//   { url, title, description, siteName, host, image (file:// or ""), imageW, imageH }
// Discord's shape: a left accent bar, the site in small caps, the title as the link, a
// two-line description, and the og:image below when there is one. Click anywhere on the
// title/image opens the link in the browser; nothing loads from the third party here (the
// homeserver spidered the page and re-hosted the image — see linkpreview.rs).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the linkPreview.* tokens

Item {
    id: card
    property var app: null
    property var preview: null
    property real maxW: 320
    property bool onAccent: false     // drawn on my own (accent) bubble

    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app ? app : fallbackTheme

    readonly property string url: (preview && preview.url) ? String(preview.url) : ""
    readonly property string title: (preview && preview.title) ? String(preview.title) : ""
    readonly property string description: (preview && preview.description) ? String(preview.description) : ""
    readonly property string site: (preview && preview.siteName && String(preview.siteName).length > 0)
        ? String(preview.siteName) : ((preview && preview.host) ? String(preview.host) : "")
    readonly property string image: (preview && preview.image) ? String(preview.image) : ""
    readonly property real imgW: (preview && preview.imageW) ? Number(preview.imageW) : 0
    readonly property real imgH: (preview && preview.imageH) ? Number(preview.imageH) : 0
    // A wide og:image goes below the text (article / store page); a small square one sits at
    // the right like an icon (a repo, a profile).
    readonly property bool thumbStyle: image.length > 0 && imgW > 0 && imgH > 0 && imgW <= 240 && Math.abs(imgW - imgH) < imgW * 0.5

    readonly property real cardW: Math.min(maxW, 420)
    implicitWidth: cardW
    implicitHeight: box.implicitHeight
    visible: url.length > 0 && (title.length > 0 || description.length > 0 || image.length > 0)

    Rectangle {
        id: box
        width: parent.width
        implicitHeight: content.implicitHeight + 16
        radius: 8
        color: card.onAccent ? Th.linkPreviewBgOwn : Th.linkPreviewBg
        border.width: 1
        border.color: Th.linkPreviewBorder
        Rectangle {
            anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom
            width: 3
            radius: 1.5
            color: card.onAccent ? Th.linkPreviewStripeOwn : Th.linkPreviewStripe
            opacity: 0.9
        }
        RowLayout {
            id: content
            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
            anchors.leftMargin: 12; anchors.rightMargin: 8; anchors.topMargin: 8
            spacing: 10
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    visible: card.site.length > 0
                    text: card.site.toUpperCase()
                    // the tokens' alpha is the item's opacity (docs/MIGRATING-THEME.md §3.8)
                    color: card.onAccent ? Qt.alpha(Th.linkPreviewSiteFgOwn, 1) : Qt.alpha(Th.linkPreviewSiteFg, 1)
                    opacity: (card.onAccent ? Th.linkPreviewSiteFgOwn : Th.linkPreviewSiteFg).a
                    font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.85
                    font.letterSpacing: 0.8
                    elide: Text.ElideRight
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    visible: card.title.length > 0
                    text: card.title
                    color: card.onAccent ? Th.linkPreviewTitleFgOwn : Th.linkPreviewTitleFg
                    font.bold: true
                    wrapMode: Text.Wrap
                    maximumLineCount: 2
                    elide: Text.ElideRight
                    font.underline: titleHover.hovered
                    HoverHandler { id: titleHover; cursorShape: Qt.PointingHandCursor }
                    TapHandler { onTapped: LinkGate.open(card.url) }
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    visible: card.description.length > 0
                    text: card.description
                    color: card.onAccent ? Qt.alpha(Th.linkPreviewDescFgOwn, 1) : Qt.alpha(Th.linkPreviewDescFg, 1)
                    opacity: (card.onAccent ? Th.linkPreviewDescFgOwn : Th.linkPreviewDescFg).a
                    wrapMode: Text.Wrap
                    maximumLineCount: 3
                    elide: Text.ElideRight
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                // Wide image below the text.
                Image {
                    visible: card.image.length > 0 && !card.thumbStyle
                    Layout.topMargin: 6
                    readonly property real capW: card.cardW - 24
                    readonly property real capH: 220
                    readonly property real natW: card.imgW > 0 ? card.imgW : implicitWidth
                    readonly property real natH: card.imgH > 0 ? card.imgH : implicitHeight
                    readonly property real fit: (natW > 0 && natH > 0) ? Math.min(1.0, capW / natW, capH / natH) : 0
                    Layout.preferredWidth: natW > 0 ? Math.round(natW * fit) : capW
                    Layout.preferredHeight: natH > 0 ? Math.round(natH * fit) : 0
                    source: visible ? card.image : ""
                    fillMode: Image.PreserveAspectFit
                    asynchronous: true
                    cache: true
                    sourceSize.width: 840
                    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: LinkGate.open(card.url) }
                }
            }
            // Small square image at the right.
            Image {
                visible: card.thumbStyle
                Layout.alignment: Qt.AlignTop
                Layout.preferredWidth: 64
                Layout.preferredHeight: 64
                source: visible ? card.image : ""
                fillMode: Image.PreserveAspectCrop
                asynchronous: true
                cache: true
                sourceSize.width: 128
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: LinkGate.open(card.url) }
            }
        }
    }
}
