// Tour — the first-run onboarding overlay. A themed, paged welcome that walks new
// users through Tin Hat's highlights, then never shows again (persisted via
// login.setTourSeen(true), mirroring the theme/nav-mode persistence). Re-openable any
// time from the ☰ menu ("Help & tour").
//
// Pure-QML; its colours are the tour.* / sheet.* tokens of the active theme (Th), its sizes and
// motion the passed-in `app`'s (= root ApplicationWindow). Drive it with open()/close(); on Done or Skip it calls
// onFinished() (the parent persists the seen flag). ESC closes (treated as Skip).
//
// IMPORTANT (CXX-Qt / qmlcachegen landmines):
//   • Never declare `property string icon` / `active` here (would shadow a FINAL group
//     and break the whole tree at runtime). We only use plain props + Kirigami.Icon.
//   • Every app property referenced (app.radius*, app.dur, …) is defined on the root in
//     Main.qml; colours come from Th (themes, docs/MIGRATING-THEME.md).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the tour.* / sheet.* tokens

Kirigami.OverlaySheet {
    id: tour
    parent: Controls.Overlay.overlay

    // The root ApplicationWindow — supplies the sizes and motion (radius…, dur)
    // and the isFurry / blobfox() helpers. Set by the parent: `app: root`.
    required property var app
    // Called when the user finishes (Done) or dismisses (Skip / ESC). The parent
    // persists the "seen" flag here so the tour never auto-shows again.
    signal finished()

    property int step: 0
    readonly property int count: cards.length

    // ~7 warm, concise cards. iconName = a Kirigami symbolic icon; blobfox = a furry
    // emote name shown instead of the icon when the Furry theme is active.
    readonly property var cards: [
        {
            icon: "video-display-brightness-symbolic",
            // Distinct from the sheet header ("Welcome to Tin Hat") right above it.
            title: "Built for game nights",
            // selfhost: Memories is named only where the server records
            body: "Share a 4K60 screen straight from the GPU, hear everyone separately with your own "
                + "per-person volumes" + (tour.app && tour.app.serverMemories !== false ? ", and record the call for Memories to relive later" : "")
                + ". Native, encrypted, and yours."
        },
        {
            icon: "view-list-details-symbolic",
            title: "Find your way around",
            body: "A persistent Discord-style Spaces sidebar — or tuck it into a corner dock "
                + "to spare an OLED panel (toggle in ☰ → \"Tuck nav into corner\"). "
                + "Drag the space bubbles to reorder them however you like."
        },
        {
            icon: "preferences-desktop-color",
            title: "Make it yours",
            body: "Two hand-tuned themes — OLED and Goose — "
                + "all under ☰ → Theme. The chrome recolors to match, instantly."
        },
        {
            icon: "call-start-symbolic",
            title: "Hop into a call",
            body: "Join a room's call, then toggle your mic, camera, or a GPU-encoded "
                + "screen-share. Dial in the share resolution, frame rate, and encoder "
                + "any time from the gear ▾ in the top-right of the room header."
        },
        {
            icon: "mail-message-symbolic",
            title: "Chat like a pro",
            body: "Markdown, reactions, replies, @mentions, and GIFs — plus drag-and-drop "
                + "files right into the composer. Hit Ctrl+K any time for the quick room "
                + "switcher."
        },
        {
            icon: "audio-input-microphone-symbolic",
            title: "Two people, one corner",
            body: "Right-click anyone in a call (or the ⋯ on their tile) → Pull aside: just the two of "
                + "you, everyone else drops to 10 %. Right-click a room → Pop out chat for a floating "
                + "window that stays over your game."
        },
        {
            icon: "applications-games-symbolic",
            title: "Game on",
            body: "Set what you're playing and a \"looking to play\" status so friends know "
                + "you're up for a game. That's the tour — you're all set! Take it again any "
                + "time from Settings → About & help."
        }
    ]

    function start() {
        tour.step = 0
        swipe.currentIndex = 0
        tour.open()
    }
    function next() {
        if (tour.step >= tour.count - 1) { tour.done(); return }
        tour.step += 1
        swipe.currentIndex = tour.step
    }
    function back() {
        if (tour.step <= 0) return
        tour.step -= 1
        swipe.currentIndex = tour.step
    }
    // Finish (Done) or Skip both mark the tour as seen and close. We only close()
    // here; onClosed below is the single source of the finished() signal so it fires
    // exactly once per dismissal (Done / Skip / ESC / click-away all funnel through it).
    function done() { tour.close() }

    // Every dismissal path (Done, Skip, ESC, click-away) lands here. Treat as "seen"
    // so the tour doesn't auto-pop again.
    onClosed: if (tour.app) tour.finished()

    showCloseButton: false
    header: Kirigami.Heading {
        text: "Welcome to Tin Hat"
        color: Th.sheetTitleFg
    }

    ColumnLayout {
        spacing: Kirigami.Units.largeSpacing
        Layout.preferredWidth: Kirigami.Units.gridUnit * 24

        // Paged cards. SwipeView gives us swipe + programmatic paging. Swiping is
        // intentionally enabled (interactive: true) and is naturally clamped at index 0
        // and count-1, with onCurrentIndexChanged keeping tour.step in sync; the
        // Next/Back buttons drive it too. (The dots are a read-only position indicator.)
        Controls.SwipeView {
            id: swipe
            Layout.fillWidth: true
            Layout.preferredHeight: Kirigami.Units.gridUnit * 13
            clip: true
            interactive: true
            onCurrentIndexChanged: tour.step = currentIndex

            Repeater {
                model: tour.cards
                delegate: ColumnLayout {
                    required property var modelData
                    spacing: Kirigami.Units.largeSpacing
                    width: swipe.width

                    Item { Layout.fillHeight: true } // top spacer (vertical centering)

                    // Illustration: a tinted Kirigami icon, or a blobfox emote in Furry.
                    Item {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: Kirigami.Units.iconSizes.enormous
                        Layout.preferredHeight: Kirigami.Units.iconSizes.enormous

                        // Soft accent glow disc behind the art.
                        Rectangle {
                            anchors.centerIn: parent
                            width: parent.width * 1.2
                            height: width
                            radius: width / 2
                            opacity: 0.16
                            gradient: Gradient {
                                GradientStop { position: 0.0; color: Th.tourArtGlow }
                                GradientStop { position: 1.0; color: "transparent" }
                            }
                        }
                        Kirigami.Icon {
                            anchors.centerIn: parent
                            width: Kirigami.Units.iconSizes.huge
                            height: Kirigami.Units.iconSizes.huge
                            source: modelData.icon
                            color: Th.tourArtIcon
                        }
                    }

                    Kirigami.Heading {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        level: 2
                        text: modelData.title
                        color: Th.tourTitleFg
                        wrapMode: Text.WordWrap
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        Layout.leftMargin: Kirigami.Units.largeSpacing
                        Layout.rightMargin: Kirigami.Units.largeSpacing
                        horizontalAlignment: Text.AlignHCenter
                        text: modelData.body
                        color: Th.tourBodyFg
                        wrapMode: Text.WordWrap
                    }

                    Item { Layout.fillHeight: true } // bottom spacer
                }
            }
        }

        // Page dots.
        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            spacing: Kirigami.Units.smallSpacing
            Repeater {
                model: tour.count
                delegate: Rectangle {
                    required property int index
                    width: index === tour.step ? Kirigami.Units.smallSpacing * 2.4 : Kirigami.Units.smallSpacing * 1.4
                    height: Kirigami.Units.smallSpacing * 1.4
                    radius: height / 2
                    color: index === tour.step ? Th.tourPagerDotActive : Th.tourPagerDot
                    Behavior on width { NumberAnimation { duration: Motion.dur } }
                    Behavior on color { ColorAnimation { duration: Motion.dur } }
                }
            }
        }

        // Nav buttons: Skip · Back · Next/Done.
        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing

            ThButton {   // 10.1 (TH-DS-02): house buttons beside the house CTA
                app: tour.app
                text: "Skip"
                onClicked: tour.done()
            }
            Item { Layout.fillWidth: true }
            ThButton {
                app: tour.app
                text: "Back"
                enabled: tour.step > 0
                visible: tour.step > 0
                onClicked: tour.back()
            }
            GlowButton {
                app: tour.app
                text: tour.step >= tour.count - 1 ? "Done" : "Next"
                onClicked: tour.next()
            }
        }
    }
}
