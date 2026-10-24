// ThemeNotices.qml — the notices at the top of the Theme section and the gallery page (themes step 1b, contract
// THEMES-UI.md §3.1, §7, §10.6; the web's ThemeNotices): the sound-pack offer, read-only (a newer app saved the
// choice), an account theme this version doesn't have, colour changes left out. Each one an InlineMessage, full width.
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

ColumnLayout {
    id: notes
    property var st: ({})                // parsed ctl.stateJson
    property var ctl: null               // ThemeController
    spacing: 8
    visible: offerMsg.visible || roMsg.visible || unknownMsg.visible || ovMsg.visible

    Kirigami.InlineMessage {
        id: offerMsg
        Layout.fillWidth: true
        readonly property var o: notes.st.offer || null
        readonly property string pack: o && o.pack === "goose" ? "Goose" : "Tin Hat"
        visible: o !== null
        type: Kirigami.MessageType.Information
        text: o ? o.themeName + " comes with its own sounds. Use " + pack + " sounds too?" : ""
        actions: [
            Kirigami.Action { text: "Use " + offerMsg.pack + " sounds"; onTriggered: notes.ctl.acceptOffer() },
            Kirigami.Action { text: "No thanks"; onTriggered: notes.ctl.dismissOffer() }
        ]
    }
    Kirigami.InlineMessage {
        id: roMsg
        Layout.fillWidth: true
        visible: notes.st.readOnly === true
        type: Kirigami.MessageType.Warning
        text: "A newer version of Tin Hat saved your theme settings. What you change here stays on this device until you update."
    }
    Kirigami.InlineMessage {
        id: unknownMsg
        Layout.fillWidth: true
        visible: typeof notes.st.unknownTheme === "string" && notes.st.unknownTheme.length > 0
        type: Kirigami.MessageType.Warning
        text: "Your account uses a theme this version of Tin Hat doesn't have (“" + notes.st.unknownTheme + "”). This device keeps "
              + Th.themeName + " until you update."
    }
    Kirigami.InlineMessage {
        id: ovMsg
        Layout.fillWidth: true
        visible: notes.st.overridesRefused === true
        type: Kirigami.MessageType.Information
        text: "Some of your colour changes need a newer Tin Hat and are left out here."
    }
}
