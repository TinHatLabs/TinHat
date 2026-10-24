// AppearancePane.qml — Settings → Appearance (themes step 1b, contract THEMES-UI.md §2; the web's AppearancePane.tsx),
// in the Settings window's card system (SETTINGS-WINDOW.md §5): Theme (§3, full width, not in a card), Sounds and icons
// (§11), Layout, Buttons (11.0), Navigation, Motion, Performance (10.8.1), Clock. The theme gallery page (§4) replaces the
// pane while open; the Settings rail stays.
// Also home to what both share: the import dialog (§6), Rename (a prompt) and Remove (at once, with an Undo toast in the
// Settings window's strip, §5.2). Undo and an unanswered sound offer last while Settings is open (theme_store.rs end_session).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: pane
    property var app: null               // Main.qml root
    property var core: null              // AppCore
    property var sheet: null             // the Settings window (Undo lasts while it is open)
    property string title: "Appearance"
    property string description: ""
    readonly property alias page: page   // SettingsWindow: the search jump scrolls it
    readonly property var ctl: core ? core.c_themeCtl : null
    readonly property var st: {
        var _r = pane.ctl ? pane.ctl.stateRev : 0
        try { return pane.ctl && pane.ctl.stateJson.length > 0 ? JSON.parse(pane.ctl.stateJson) : ({}) } catch (e) { return ({}) }
    }
    readonly property var gallery0: pane.ctl ? JSON.parse(pane.ctl.galleryJson("", "all")) : ({ favourites: [], all: [] })
    property bool galleryOpen: false
    Connections {
        target: pane.sheet
        function onOpened() { if (pane.ctl) pane.ctl.endSession() }
        function onClosed() { pane.galleryOpen = false; if (pane.ctl) pane.ctl.endSession() }
    }

    // GUI-verification hook (screenshot automation without coordinate clicking, like Main.qml's --tinhat-pane):
    // --tinhat-themes=STEP[,STEP…] runs once, the first time the pane shows: `gallery` / `gallery:<query>` / `light` /
    // `dark` open the gallery (searching / filtered), `import` / `import:<file path>` the import dialog (with a file),
    // `mine:<file path>` saves that theme file to My themes (not applied).
    readonly property string guiHook: {
        var args = Qt.application.arguments
        for (var i = 0; i < args.length; i++)
            if (args[i].indexOf("--tinhat-themes=") === 0) return args[i].substring("--tinhat-themes=".length)
        return ""
    }
    property bool _hookDone: false
    onVisibleChanged: if (visible && pane.guiHook.length > 0 && !pane._hookDone) { pane._hookDone = true; Qt.callLater(pane.runHook) }
    function runHook() {
        var steps = pane.guiHook.split(",")
        for (var i = 0; i < steps.length; i++) {
            var st = steps[i], k = st.indexOf(":"), cmd = k < 0 ? st : st.substring(0, k), arg = k < 0 ? "" : st.substring(k + 1)
            if (cmd === "gallery") { pane.galleryOpen = true; if (arg.length > 0) gallery.setQuery(arg) }
            else if (cmd === "light" || cmd === "dark") { pane.galleryOpen = true; gallery.filter = cmd }
            else if (cmd === "import") pane.openImport(arg.length > 0 ? "file://" + arg : "")
            // 11.0 buttons (review captures): scroll the pane to the Buttons section
            else if (cmd === "buttons") Qt.callLater(function () { page.scrollToItem(buttonsSection) })
            else if (cmd === "layout") Qt.callLater(function () { page.scrollToItem(layoutSection) })   // 11.0 edit mode captures
            else if (cmd === "mine" && arg.length > 0) {
                var f = JSON.parse(pane.ctl.readFile("file://" + arg))
                var r = f.ok ? JSON.parse(pane.ctl.importText(f.text, f.name)) : f
                if (r.ok) pane.ctl.saveImported(JSON.stringify(r.doc), r.name, false)
                else console.log("[themes] gui hook: " + r.text)
            }
        }
    }

    /// THEMES-UI §12: the ☰ Theme menu's "More themes…" — the gallery, opened fresh (no search, All, at the top) like
    /// the web's, whose page mounts anew every time
    function openGalleryFresh() { gallery.reset(); pane.galleryOpen = true }
    function openImport(url) { importDialog.openWith(url || "") }
    function setPack(key, value) {
        var err = pane.ctl.applyPack(key, value)
        if (err.length > 0 && pane.app) pane.app.settingsToast(err, 6000)
    }
    // 11.0 step 5 (THEMES-UI §13.1, §14.5): Edit… (one of My themes: Save replaces it) / New theme from this… (any other
    // card: a copy, "‹name› (edited)"), Customize… (the theme on screen), Share to a room… — the editor works on the live
    // app, so the Settings sheet closes when it opens (Main.qml: AppCore.themeEditorOpened)
    function editTheme(id, name, mine) {
        var d = pane.core.c_editorCtl.docJson(id)
        if (d.length === 0) { pane.app.settingsToast(name + " can't be edited by this version of Tin Hat.", "long"); return }
        pane.core.openThemeEditor(d, mine ? id : "", name)
    }
    function shareTheme(id, name) {
        var d = pane.core.c_editorCtl.docJson(id)
        if (d.length > 0) pane.app.shareThemeToRoom(d, name)
    }
    // Customize… (the summary's, the gallery's) and the ☰ Theme menu's "Edit current theme…" (Main.qml editCurrentTheme,
    // THEMES-UI §12 amendment 2026-09-28): the theme on screen. Its base name is the theme's OWN name, like the web's
    // editCurrentTheme (doc.name) — Th.themeName is the preview's while an editor shows ("OLED (edited)"), and the ☰ row
    // is reachable with the editor open ("A new theme from OLED (edited)" on the desktop, "… from OLED" on the web).
    function customize() {
        var id = pane.ctl.effectiveTheme
        var mine = (pane.st.mine || []).some(function (m) { return m.id === id })
        var name = Th.themeName
        try { name = JSON.parse(pane.core.c_editorCtl.docJson(id)).name || name } catch (e) {}
        pane.editTheme(id, name, mine)
    }
    function rename(id, name) { renameDialog.themeId = id; renameDialog.oldName = name; renameField.text = name; renameDialog.open() }
    function remove(id, name) {
        var doc = pane.ctl.removeMine(id)
        if (doc.length === 0) return
        pane.app.settingsToast(name + " removed", 8000, "Undo", function () {
            var err = pane.ctl.restoreMine(doc)
            if (err.length > 0) pane.app.settingsToast(err, "long")
        })
    }

    // THE pane (SETTINGS-WINDOW.md §5: today's sections as cards; Theme keeps its picker full width, not in a card)
    SettingsPage {
        id: page
        anchors.fill: parent
        visible: !pane.galleryOpen
        app: pane.app
        title: pane.title
        description: pane.description

        SettingsSection {
            id: themeSection
            title: "Theme"
            plain: true
            anchorIds: ["appearance.theme", "appearance.customize-theme"]
            ThemeSection {
                Layout.fillWidth: true
                app: pane.app
                core: pane.core
                ctl: pane.ctl
                st: pane.st
                favourites: pane.gallery0.favourites
                libraryCount: pane.gallery0.all.length
                onOpenGallery: pane.galleryOpen = true
                onOpenImport: pane.openImport("")
                onRenameMine: (id, name) => pane.rename(id, name)
                onRemoveMine: (id, name) => pane.remove(id, name)
                onEditTheme: (id, name, mine) => pane.editTheme(id, name, mine)
                onShareTheme: (id, name) => pane.shareTheme(id, name)
                onCustomize: pane.customize()
            }
        }

        // THEMES-UI §11: the sound pack and the icon style — their own settings, saved through the theme store like a
        // theme pick (the account, or this device with the override on)
        SettingsSection {
            title: "Sounds and icons"
            SettingsRow {
                app: pane.app
                anchorId: "appearance.sound-pack"
                label: "Sound pack"
                description: "Goose swaps the chimes you hear when someone joins or leaves a call. Every other sound stays the same."
                trailing: [
                    // the pack in use plays its join chime (a second click while it plays stops it — AppCore.previewSound)
                    ThToolButton { app: pane.app; iconName: "media-playback-start"; tip: "Hear the join chime"; onClicked: if (pane.core) pane.core.previewSound("join") },
                    ThComboBox {
                        Layout.preferredWidth: 240
                        Accessible.name: "Sound pack"
                        model: ["Tin Hat", "Goose"]
                        currentIndex: pane.ctl && pane.ctl.soundPack === "goose" ? 1 : 0
                        onActivated: (i) => { pane.setPack("sounds", i === 1 ? "goose" : "default"); currentIndex = Qt.binding(() => pane.ctl && pane.ctl.soundPack === "goose" ? 1 : 0) }
                    }
                ]
            }
            SettingsRow {
                app: pane.app
                anchorId: "appearance.icon-pack"
                label: "Icon pack"
                description: "Two-tone icons light up in the theme's accent colour when you point at them or turn them on."
                trailing: ThComboBox {
                    Layout.preferredWidth: 240
                    Accessible.name: "Icon style"
                    model: ["Two-tone", "One colour"]
                    currentIndex: pane.ctl && pane.ctl.iconStyle === "mono" ? 1 : 0
                    onActivated: (i) => { pane.setPack("icons", i === 1 ? "mono" : "duotone"); currentIndex = Qt.binding(() => pane.ctl && pane.ctl.iconStyle === "mono" ? 1 : 0) }
                }
            }
        }

        // 10.8: islands or the flat look (per device); 11.0 edit mode: the layouts (LayoutSection.qml)
        SettingsSection {
            id: layoutSection
            title: "Layout"
            SettingsRow {
                app: pane.app
                label: "Style"
                description: "Islands gives every part of the window its own rounded card; Flat puts them edge to edge, the way Tin Hat looked before."
                trailing: ThComboBox {
                    Layout.preferredWidth: 240
                    Accessible.name: "Layout style"
                    model: ["Islands", "Flat"]
                    currentIndex: pane.app.layoutStyle === "flat" ? 1 : 0
                    onActivated: (i) => { pane.app.setLayoutStyle(i === 1 ? "flat" : "islands"); currentIndex = Qt.binding(() => pane.app.layoutStyle === "flat" ? 1 : 0) }
                }
            }
            LayoutSection { app: pane.app }
        }

        // 11.0: the button rows and your own button islands (ButtonsSettings.qml; the web's ButtonsSettings.tsx)
        ButtonsSettings { id: buttonsSection; Layout.fillWidth: true; app: pane.app }

        // the same setting as the ☰ menu's toggle, surfaced so the choice is discoverable
        SettingsSection {
            title: "Navigation"
            SettingsRow {
                app: pane.app
                anchorId: "appearance.navigation"
                label: "Navigation"
                description: "Corner dock hides the room list until you point at the corner — nice for OLED screens and small windows."
                trailing: ThComboBox {
                    Layout.preferredWidth: 240
                    Accessible.name: "Navigation"
                    model: ["Sidebar (always visible)", "Corner dock (OLED-friendly)"]
                    currentIndex: pane.app.navMode === "corner" ? 1 : 0
                    onActivated: (i) => { pane.app.setNavMode(i === 1 ? "corner" : "persistent"); currentIndex = Qt.binding(() => pane.app.navMode === "corner" ? 1 : 0) }
                }
            }
        }

        // 10.1 (TH-VIS-21)
        SettingsSection {
            title: "Motion"
            SettingsRow {
                app: pane.app
                anchorId: "appearance.reduce-motion"
                label: "Reduce motion"
                description: "Slides and fades become instant changes, and the game reel jumps straight to its result."
                toggle: reduceSw
                trailing: ThSwitch {
                    id: reduceSw
                    Accessible.name: "Reduce motion"
                    checked: pane.app.mReducedMotion
                    onToggled: { pane.app.mReducedMotion = checked; pane.app.settings.applyUiPref("reduce-motion", checked ? "1" : "0") }
                }
            }
            // 11.0 (GPU.md §2.6, both apps): the pref `gif-autoplay`, default on (ThGif)
            SettingsRow {
                app: pane.app
                label: "Play GIFs automatically"
                description: "GIFs play up to three times, then rest on their first frame. Point at one to play it again; with Reduce motion on, a click plays it once."
                toggle: gifSw
                trailing: ThSwitch {
                    id: gifSw
                    Accessible.name: "Play GIFs automatically"
                    checked: Motion.gifAutoplay
                    onToggled: { Motion.gifAutoplay = checked; pane.app.settings.applyUiPref("gif-autoplay", checked ? "1" : "0") }
                }
            }
        }

        // 10.8.1 (GPU.md §1 amendment, both apps): the unfocused rules' master switch — the pref `call-unfocused-saver`, default OFF
        SettingsSection {
            title: "Performance"
            SettingsRow {
                app: pane.app
                label: "Save GPU while Tin Hat isn't focused"
                description: "For lower-end PCs: while Tin Hat isn't the active window it pauses animations, previews and GIFs and shows cameras small."
                toggle: saverSw
                trailing: ThSwitch {
                    id: saverSw
                    Accessible.name: "Save GPU while Tin Hat isn't focused"
                    checked: pane.app.mCallUnfocusedSaver
                    onToggled: { pane.app.mCallUnfocusedSaver = checked; pane.app.settings.applyUiPref("call-unfocused-saver", checked ? "1" : "0") }
                }
            }
        }

        SettingsSection {
            title: "Clock"
            SettingsRow {
                app: pane.app
                anchorId: "appearance.clock"
                label: "Clock"
                description: "Message times in chat. System follows your desktop's setting."
                trailing: ThComboBox {
                    Layout.preferredWidth: 240
                    readonly property var modes: ["system", "12", "24"]
                    Accessible.name: "Clock"
                    model: ["System", "12-hour", "24-hour"]
                    currentIndex: Math.max(0, modes.indexOf(pane.app.clockMode))
                    onActivated: (i) => { pane.app.setClockMode(modes[i]); currentIndex = Qt.binding(() => Math.max(0, modes.indexOf(pane.app.clockMode))) }
                }
            }
        }
    }

    ThemeGalleryPage {
        id: gallery
        anchors.fill: parent
        anchors.leftMargin: 28
        anchors.rightMargin: 16
        anchors.topMargin: 20
        anchors.bottomMargin: 8
        visible: pane.galleryOpen
        focus: pane.galleryOpen
        app: pane.app
        core: pane.core
        ctl: pane.ctl
        st: pane.st
        libraryCount: pane.gallery0.all.length
        onBack: pane.galleryOpen = false
        onOpenImport: (url) => pane.openImport(url)
        onRenameMine: (id, name) => pane.rename(id, name)
        onRemoveMine: (id, name) => pane.remove(id, name)
        onEditTheme: (id, name, mine) => pane.editTheme(id, name, mine)
        onShareTheme: (id, name) => pane.shareTheme(id, name)
        onCustomize: pane.customize()
    }

    ThemeImportDialog {
        id: importDialog
        app: pane.app
        ctl: pane.ctl
        onSaved: (id, name, used) => pane.app.settingsToast(name + " saved to My themes", "short")
    }

    // Rename… (THEMES-UI §5.2): "Rename theme", the name (1–80 characters), Save / Cancel. Renaming the theme in use
    // re-applies it (theme_store.rs rename_mine), so the new name shows and syncs.
    Kirigami.PromptDialog {
        id: renameDialog
        property string themeId: ""
        property string oldName: ""
        title: "Rename theme"
        preferredWidth: 360
        standardButtons: Kirigami.Dialog.Save | Kirigami.Dialog.Cancel
        onOpened: { renameField.forceActiveFocus(); renameField.selectAll() }
        onAccepted: pane.doRename()
        ThTextField {
            id: renameField
            Layout.fillWidth: true
            maximumLength: 80
            Accessible.name: "Theme name"
            onAccepted: { pane.doRename(); renameDialog.close() }
        }
    }
    function doRename() {
        var n = renameField.text.trim()
        if (n.length < 1 || n.length > 80 || n === renameDialog.oldName) return
        var err = pane.ctl.renameMine(renameDialog.themeId, n)
        if (err.length > 0) pane.app.settingsToast(err, "long")
    }
}
