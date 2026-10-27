// EmptyState — a tasteful empty / loading placeholder used across Tin Hat:
//   • "no rooms yet"        (room list, empty)
//   • "pick a room"         (no conversation open — for a future split view)
//   • conversation empty    (room open, no messages)
//   • loading               (spinner; furry uses BlobfoxSpinner)
//
// Drive it with `loading`, `title`, `explanation`, and either a furry blobfox emote
// name (`blobfoxName`) or a non-furry Kirigami `iconName`. Furry overrides title/
// explanation copy if `furryTitle`/`furryExplanation` are set. Pure-QML; its colours are the
// empty.* tokens of Th. The furry loading spinner is
// FurryFlair.BlobfoxSpinner (inline component of the sibling FurryFlair type).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: empty
    required property var app   // ApplicationWindow (theme tokens); set by parent
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme


    property bool loading: false
    property string iconName: "documentinfo-symbolic"
    property string title: ""
    property string explanation: ""

    readonly property string shownTitle: empty.title
    readonly property string shownExplanation: empty.explanation

    // 12.0: the height of what it draws, so a layout gives it a real slot. With no implicit size a
    // ColumnLayout made it 0 px high and the column centred on it spilled over its neighbours (the calendar's
    // day panel: the illustration over the date / main-game lines, the copy over "New event on this day";
    // Upcoming and Games & rules: the illustration cut off above the list). Hosts that anchor or fill it
    // size it themselves and are unchanged.
    implicitHeight: body.implicitHeight

    ColumnLayout {
        id: body
        anchors.centerIn: parent
        width: Math.min(parent.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 22)
        spacing: Kirigami.Units.largeSpacing

        // ---- Illustration -----------------------------------------------------
        Item {
            Layout.alignment: Qt.AlignHCenter
            Layout.preferredWidth: Kirigami.Units.iconSizes.enormous
            Layout.preferredHeight: Kirigami.Units.iconSizes.enormous

            // Soft glow disc behind the art.
            Rectangle {
                anchors.centerIn: parent
                width: parent.width * 1.15
                height: width
                radius: width / 2
                opacity: 0.18
                gradient: Gradient {
                    GradientStop { position: 0.0; color: Th.emptyGlow }
                    GradientStop { position: 1.0; color: "transparent" }
                }
            }



            // LOADING → the house busy mark (11.0, GPU.md §2.5: still — no spinner loop).
            ThBusyIndicator {
                anchors.centerIn: parent
                size: 48
                visible: empty.loading
                running: visible
            }

            // idle → a tinted Kirigami icon.
            Kirigami.Icon {
                anchors.centerIn: parent
                width: Kirigami.Units.iconSizes.huge
                height: Kirigami.Units.iconSizes.huge
                visible: !empty.loading
                source: empty.iconName
                color: Th.emptyIcon
                opacity: 0.85
            }
        }

        // ---- Copy -------------------------------------------------------------
        Kirigami.Heading {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            level: 2
            visible: empty.shownTitle.length > 0
            text: empty.shownTitle
            color: Th.emptyTitleFg
            wrapMode: Text.WordWrap
        }
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            visible: empty.shownExplanation.length > 0
            text: empty.shownExplanation
            color: Th.emptyBodyFg
            wrapMode: Text.WordWrap
        }
    }
}
