// ThWindow.qml — P22 (12.1.17): the root of every Tin Hat secondary window (Settings, Space settings, Calendar, Memories,
// Watch Party, the Game Reel, the picture-in-picture tile, the pop-out chats, the icon gallery). A plain QtQuick Window
// passes no font to the controls inside it, so a control that sizes nothing itself (a plain button, a tab or weekday label,
// a combo's items) stayed at the platform's size there whatever the person's Text size was; only the main window
// (Kirigami.ApplicationWindow) handed it on. This is a Controls.ApplicationWindow whose font carries the person's text size
// like Main.qml's: the window's own font, nothing app-wide (Qt.application.font and Kirigami.Theme's fonts, which the
// explicit sizes multiply by Th.textScale, stay the platform's, so nothing is scaled twice). At 100 % nothing is set: going
// back to 100 % RESETS the window's font (it inherits the platform's again, also a platform font changed meanwhile), never
// writes back a remembered number.
// (KDE's own Controls style, org.kde.desktop, binds its Label and TabButton text to Kirigami.Theme.defaultFont instead of the
// inherited font, so under that style those two follow only an explicit size; every other control follows the window.)
// scripts/text-scale-lint.py fails a window that isn't a ThWindow (unless listed, with its reason);
// scripts/window-font-scope-test.qml checks the scope itself.
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Controls.ApplicationWindow {
    id: thWindow
    // the window font's size for the person's text size; 0 = leave the window's font alone (100 %, or a platform font
    // without a point size)
    readonly property real thTextPointSize: Th.textSize !== 100 && Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize * Th.textScale : 0
    function thApplyTextSize() {
        if (thWindow.thTextPointSize > 0) thWindow.font.pointSize = thWindow.thTextPointSize
        else thWindow.font = undefined   // the RESET: the window inherits the platform's font again
    }
    onThTextPointSizeChanged: thWindow.thApplyTextSize()
    Component.onCompleted: thWindow.thApplyTextSize()
}
