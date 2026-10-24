// ThemeSection.qml — Settings → Appearance → Theme (themes step 1b, contract THEMES-UI.md §3; the web's ThemeSection):
// the notices, the summary with Undo, the favourites (+ the All themes tile), My themes (+ the Import tile) and "Use a
// different theme on this device". Every pick applies at once through the theme store (THEMES-SYNC.md) and can be
// undone.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

ColumnLayout {
    id: sec
    property var app: null
    property var core: null              // AppCore (chooseTheme)
    property var ctl: null               // ThemeController
    property var st: ({})                // parsed ctl.stateJson
    property var favourites: []          // [{id, name, author, variant}]
    property int libraryCount: 0
    signal openGallery()
    signal openImport()
    signal renameMine(string id, string name)
    signal removeMine(string id, string name)
    signal editTheme(string id, string name, bool mine)   // 11.0 (THEMES-UI §13.1)
    signal shareTheme(string id, string name)             // §14.5
    signal customize()                                    // the summary's Customize…

    spacing: 8

    function pick(id) {
        var err = sec.core ? sec.core.chooseTheme(id, true) : ""
        if (err && err.length > 0 && sec.app) sec.app.settingsToast(err, 6000)
    }

    ThemeNotices { Layout.fillWidth: true; st: sec.st; ctl: sec.ctl }

    // ---- §3.2 summary --------------------------------------------------------------------------------------------
    ThemeSummary {
        Layout.fillWidth: true
        app: sec.app
        ctl: sec.ctl
        st: sec.st
        undoVisible: sec.st.canUndo === true
        customizeVisible: true
        onCustomize: sec.customize()
        onUndo: {
            var err = sec.ctl.undoTheme()
            if (err.length > 0 && sec.app) sec.app.settingsToast(err, 6000)
        }
    }

    // ---- §3.3 favourites ---------------------------------------------------------------------------------------
    Controls.Label { Layout.topMargin: 4; text: "Favourites"; color: Th.settingsThemeMetaFg }
    ThemeGrid {
        Layout.fillWidth: true
        app: sec.app
        ctl: sec.ctl
        items: sec.favourites
        selectedId: sec.ctl ? sec.ctl.effectiveTheme : ""
        tileText: "All " + sec.libraryCount.toLocaleString(Qt.locale("en_US"), "f", 0) + " themes"
        tileIcon: "view-grid-symbolic"
        onUse: (id) => sec.pick(id)
        onEditTheme: (id, name) => sec.editTheme(id, name, false)
        onTileClicked: sec.openGallery()
    }

    // ---- §3.4 My themes ----------------------------------------------------------------------------------------
    Controls.Label {
        readonly property int n: (sec.st.mine || []).length
        Layout.topMargin: 4
        text: "My themes" + (n > 0 ? " · " + n : "")
        color: Th.settingsThemeMetaFg
    }
    ThemeGrid {
        Layout.fillWidth: true
        app: sec.app
        ctl: sec.ctl
        items: sec.st.mine || []
        stateRev: sec.ctl ? sec.ctl.stateRev : 0
        selectedId: sec.ctl ? sec.ctl.effectiveTheme : ""
        mine: true
        tileText: sec.st.themesReadOnly === true ? "" : "Import a theme"
        tileIcon: "document-open-symbolic"
        onUse: (id) => sec.pick(id)
        onRename: (id, name) => sec.renameMine(id, name)
        onRemove: (id, name) => sec.removeMine(id, name)
        onEditTheme: (id, name) => sec.editTheme(id, name, true)
        onShareTheme: (id, name) => sec.shareTheme(id, name)
        onTileClicked: sec.openImport()
    }
    HintLabel {
        app: sec.app
        Layout.fillWidth: true
        visible: sec.st.themesReadOnly === true || (sec.st.mine || []).length === 0
        text: sec.st.themesReadOnly === true ? "A newer Tin Hat manages My themes — update to add or remove themes."
                                            : "Themes you import are saved here and follow you to every device."
    }

    // ---- §3.5 the device override (not in gate mode: nothing syncs there) -------------------------------------
    ThSwitch {
        visible: sec.st.sync === true
        text: "Use a different theme on this device"
        checked: sec.ctl ? sec.ctl.overrideOn : false
        enabled: sec.st.signedIn === true
        onToggled: { sec.ctl.setDeviceOverride(checked); checked = Qt.binding(() => sec.ctl ? sec.ctl.overrideOn : false) }
    }
    HintLabel {
        app: sec.app
        Layout.fillWidth: true
        visible: sec.st.sync === true
        text: (sec.ctl && sec.ctl.overrideOn)
              ? ("This device: " + Th.themeName + ". Everywhere else: "
                 + ((typeof sec.st.accountThemeName === "string" && sec.st.accountThemeName.length > 0)
                    ? sec.st.accountThemeName : "each device's own theme") + ".")
              : "Your theme, sounds and icon style follow your account to every device. Turn this on to choose them separately here."
    }
}
