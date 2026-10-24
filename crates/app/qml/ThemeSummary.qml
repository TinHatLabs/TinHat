// ThemeSummary.qml — the active theme's summary in Settings → Appearance and on the gallery page (themes step 1b,
// contract THEMES-UI.md §3.2; the web's ThemeSummary): name (bold, at most half the row) + "Dark theme · by ‹author› ·
// on this device", Undo after it (after an apply made while the Settings sheet is open), then "N colours adjusted for
// readability" and the offline "saved on this device" line when they apply. `compact` (the gallery toolbar): the
// row only, no adjusted note. 11.0 (THEMES-UI §13.1): a flat Customize… before Undo — the theme editor on the theme on
// screen (one of My themes → Edit…, any other → New theme from this).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

ColumnLayout {
    id: sum
    property var app: null
    property var ctl: null               // ThemeController
    property var st: ({})                // parsed ctl.stateJson
    property bool undoVisible: false
    property bool customizeVisible: false
    property bool compact: false         // the gallery header: one row only (no adjusted line)
    signal undo()
    signal customize()

    spacing: 4

    RowLayout {
        id: row
        Layout.fillWidth: true
        Layout.minimumHeight: 24
        spacing: 8
        Controls.Label {
            text: Th.themeName
            color: Th.settingsThemeNameFg
            font.bold: true
            elide: Text.ElideRight
            Layout.maximumWidth: Math.max(0, Math.min(implicitWidth, row.width / 2))
        }
        Controls.Label {
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            Layout.maximumWidth: Math.max(0, implicitWidth)
            elide: Text.ElideRight
            color: Th.settingsThemeMetaFg
            text: (Th.variant === "light" ? "Light theme" : "Dark theme")
                  + ((sum.ctl && sum.ctl.author.length > 0) ? " · by " + sum.ctl.author : "")
                  + ((sum.ctl && sum.ctl.overrideOn) ? " · on this device" : "")
        }
        Item { Layout.fillWidth: true }
        ThButton {
            app: sum.app
            visible: sum.customizeVisible
            flat: true
            text: "Customize…"
            icon.name: "document-edit-symbolic"
            Controls.ToolTip.text: "Change this theme's colours, fonts and look"
            Controls.ToolTip.visible: hovered
            Controls.ToolTip.delay: 600
            onClicked: sum.customize()
        }
        ThButton {
            app: sum.app
            visible: sum.undoVisible
            flat: true
            text: "Undo"
            icon.name: "edit-undo-symbolic"
            Controls.ToolTip.text: "Go back to the theme you had before"
            Controls.ToolTip.visible: hovered
            Controls.ToolTip.delay: 600
            onClicked: sum.undo()
        }
    }
    RowLayout {
        visible: !sum.compact && sum.ctl !== null && sum.ctl.adjusted > 0
        spacing: 6
        Kirigami.Icon {
            Layout.preferredWidth: 16
            Layout.preferredHeight: 16
            source: "help-about-symbolic"
            color: Th.settingsThemeNoteIcon
            isMask: true
        }
        Controls.Label {
            color: Th.settingsThemeNoteFg
            text: sum.ctl ? (sum.ctl.adjusted === 1 ? "1 colour adjusted for readability"
                                                    : sum.ctl.adjusted + " colours adjusted for readability") : ""
        }
    }
    Controls.Label {
        visible: sum.st && sum.st.pending === true
        color: Th.settingsThemeMetaFg
        font.italic: true
        font.pointSize: Kirigami.Theme.smallFont.pointSize
        text: "Saved on this device — it syncs when you're back online."
    }
}
