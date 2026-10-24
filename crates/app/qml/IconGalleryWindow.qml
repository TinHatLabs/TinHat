// IconGalleryWindow.qml — every icon name the app uses, rendered through ThIcon at the
// sizes the UI draws, so an icon pack can be reviewed IN the real app (theme colours, the
// hover / active triggers, the fallback to Breeze for names the pack lacks). Opened from the
// Help menu ("Icon gallery") or at start with TINHAT_GUI_ICONGALLERY=1. The name list mirrors
// docs/ICONS.md — keep both in sync when a new icon name enters the app.
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the iconGallery.* tokens

Window {
    id: gal
    required property var app
    signal reloadRequested()

    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app ? app : fallbackTheme
    readonly property var pack: (app && app.mIconPack) ? app.mIconPack : null

    title: "Icon gallery — Tin Hat"
    width: 1000
    height: 720
    minimumWidth: 640
    minimumHeight: 420
    color: Th.iconGalleryBg

    property int iconSize: 32
    property bool playAll: false
    property bool showActive: false
    property string filter: ""

    // docs/ICONS.md — A. calls, B. chat/rooms/people, C. players, D. account/settings, + 0.8.26 calendar.
    readonly property var names: [
        "audio-input-microphone-symbolic", "microphone-sensitivity-muted-symbolic", "audio-volume-high-symbolic",
        "audio-volume-low-symbolic", "audio-volume-muted-symbolic", "audio-speakers-symbolic", "camera-video-symbolic",
        "monitor-symbolic", "window-symbolic", "window-duplicate-symbolic", "video-display-brightness-symbolic",
        "video-television-symbolic", "call-start-symbolic", "call-stop-symbolic", "emblem-music-symbolic",
        "folder-music-symbolic", "link-symbolic", "media-record-symbolic", "layer-visible-off-symbolic",
        "layer-visible-on-symbolic", "view-fullscreen-symbolic", "view-restore-symbolic", "window-new",
        "applications-games-symbolic", "view-statistics-symbolic", "checkmark-symbolic", "dialog-ok-symbolic",
        "media-playback-stop-symbolic", "edit-undo-symbolic", "appointment-missed-symbolic", "configure-symbolic",
        "settings-configure-symbolic", "view-more-symbolic",
        "mail-message-symbolic", "mail-message-new-symbolic", "mail-reply-sender-symbolic", "list-add-symbolic",
        "list-add-user", "contact-new-symbolic", "insert-image-symbolic", "edit-copy-symbolic", "edit-delete-symbolic",
        "delete-symbolic", "edit-delete-remove", "edit-clear-symbolic", "edit-clear-all-symbolic",
        "document-edit-symbolic", "document-save-symbolic", "document-open-symbolic", "document-send-symbolic",
        "folder-new-symbolic", "folder-symbolic", "go-home-symbolic", "go-next-symbolic", "go-previous-symbolic",
        "rating", "starred-symbolic", "non-starred-symbolic", "dialog-close-symbolic", "dialog-cancel-symbolic",
        "dialog-ok-apply-symbolic", "dialog-messages-symbolic", "view-list-details-symbolic", "documentinfo-symbolic",
        "view-left-close-symbolic", "sidebar-collapse-left-symbolic", "sidebar-expand-left-symbolic",
        "application-menu-symbolic", "view-refresh-symbolic", "window-close-symbolic", "window-minimize-symbolic",
        "window-pin-symbolic", "user-symbolic", "user-identity", "system-users-symbolic", "im-kick-user-symbolic",
        "im-ban-user-symbolic", "channel-secure-symbolic", "channel-insecure-symbolic", "lock-symbolic",
        "view-visible-symbolic", "view-hidden-symbolic", "network-server-symbolic", "network-connect",
        "network-disconnect", "network-disconnect-symbolic",
        "media-playback-start", "media-playback-start-symbolic", "media-playback-pause", "media-playback-stop",
        "media-skip-forward", "media-skip-backward", "media-playlist-append", "list-add", "list-remove-symbolic",
        "view-preview-symbolic", "video-x-generic", "audio-x-generic", "application-pdf", "application-x-archive",
        "application-octet-stream",
        "security-high-symbolic", "security-low-symbolic", "backup-symbolic", "dialog-warning", "computer-symbolic",
        "smartphone-symbolic", "help-about-symbolic", "help-hint-symbolic", "tools-report-bug-symbolic",
        "system-log-out-symbolic", "system-software-update", "system-software-install", "preferences-desktop-color",
        "settings-configure", "contrast-symbolic", "view-private-symbolic",
        // 0.8.26 — calendar
        "view-calendar-symbolic", "view-calendar-month-symbolic", "view-calendar-list-symbolic",
        "view-calendar-upcoming-events-symbolic", "appointment-new-symbolic", "notifications",
        "notifications-disabled-symbolic",
        // 0.8.30 — drawn in the pack's style for the names the app used that the pack lacked
        "image-x-generic", "text-x-generic", "face-smile-symbolic", "headphones-symbolic",
        "bookmark-new-symbolic", "view-grid-symbolic"
    ]
    readonly property var shown: {
        var f = filter.trim().toLowerCase()
        if (f.length === 0) return names
        return names.filter(function (n) { return n.indexOf(f) >= 0 })
    }
    function isAnimated(n) {
        return !!(pack && pack.icons && pack.icons[n] && pack.icons[n].anim)
    }
    readonly property int animatedCount: {
        var c = 0
        for (var i = 0; i < names.length; ++i) if (isAnimated(names[i])) c++
        return c
    }
    function openOrRaise() {
        if (gal.visible) { gal.raise(); gal.requestActivate() }
        else { gal.show(); gal.raise(); gal.requestActivate() }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Rectangle {
            Layout.fillWidth: true
            implicitHeight: hdr.implicitHeight + Kirigami.Units.largeSpacing * 2
            color: Th.iconGalleryHeaderBg
            ColumnLayout {
                id: hdr
                anchors.fill: parent
                anchors.margins: Kirigami.Units.largeSpacing
                spacing: Kirigami.Units.smallSpacing
                RowLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.largeSpacing
                    Kirigami.Heading { level: 2; text: "Icon gallery"; color: Th.iconGalleryFg }
                    Controls.Label {
                        Layout.fillWidth: true
                        elide: Text.ElideMiddle
                        color: Th.iconGalleryMetaFg
                        text: gal.pack && gal.pack.path
                              ? ("Pack: " + gal.pack.path + "  ·  " + gal.animatedCount + " of " + gal.names.length + " animated"
                                 + (gal.pack.theme ? "  ·  static theme '" + gal.pack.theme + "'" : "  ·  no static theme (Breeze)"))
                              : "No icon pack found — showing Breeze. Drop a pack in <app data>/iconpack and press Reload."
                    }
                    Controls.Button {
                        text: "Reload pack"
                        icon.name: "view-refresh-symbolic"
                        onClicked: gal.reloadRequested()
                    }
                }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.largeSpacing
                    Controls.TextField {
                        Layout.preferredWidth: 260
                        placeholderText: "Filter names…"
                        onTextChanged: gal.filter = text
                    }
                    Controls.Label { text: "Size"; color: Th.iconGalleryMetaFg }
                    Controls.ComboBox {
                        model: ["16", "22", "32", "48", "64"]
                        currentIndex: 2
                        onActivated: gal.iconSize = Number(currentText)
                    }
                    Controls.CheckBox { text: "Play all"; checked: gal.playAll; onToggled: gal.playAll = checked }
                    Controls.CheckBox { text: "Active state"; checked: gal.showActive; onToggled: gal.showActive = checked }
                    Item { Layout.fillWidth: true }
                    Controls.Label { text: gal.shown.length + " icons"; color: Th.iconGalleryMetaFg }
                }
            }
        }

        GridView {
            id: grid
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            cellWidth: 160
            cellHeight: 126
            model: gal.shown
            delegate: Item {
                width: grid.cellWidth
                height: grid.cellHeight
                required property string modelData
                Rectangle {
                    anchors.fill: parent
                    anchors.margins: 4
                    radius: 10
                    color: ma.containsMouse ? Th.iconGalleryCellBgHover : "transparent"
                    border.width: 1
                    border.color: gal.isAnimated(modelData) ? Th.iconGalleryCellAnimatedBorder : Th.iconGalleryCellBorder
                }
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 8
                    spacing: 6
                    Item {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 64
                        ThIcon {
                            anchors.centerIn: parent
                            width: gal.iconSize
                            height: gal.iconSize
                            app: gal.app
                            name: modelData
                            color: Th.iconGalleryIcon
                            hovered: ma.containsMouse
                            active: gal.showActive
                            playing: gal.playAll
                        }
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        text: modelData
                        color: Th.iconGalleryCellFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        elide: Text.ElideMiddle
                    }
                    Controls.Label {
                        Layout.alignment: Qt.AlignHCenter
                        visible: gal.isAnimated(modelData)
                        text: gal.isAnimated(modelData) ? ("animated · " + (gal.pack.icons[modelData].trigger || "hover")) : ""
                        color: Th.iconGalleryCellAnimatedFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.9
                    }
                }
                MouseArea {
                    id: ma
                    anchors.fill: parent
                    hoverEnabled: true
                    acceptedButtons: Qt.NoButton
                }
            }
        }
    }
}
