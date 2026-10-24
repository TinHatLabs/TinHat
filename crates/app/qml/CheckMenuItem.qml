// CheckMenuItem — MenuItem with the NATIVE check indicator whose checked state stays
// DECLARATIVE. Interactive toggles permanently break a plain `checked:` binding (the
// mark then lies the next time the menu opens), so callers pass the condition as a
// FUNCTION and we re-establish the binding after every user toggle. QML dependency
// tracking sees through the function call, so the mark still updates live.
//
//     CheckMenuItem { isChecked: () => root.mInputName === modelData.node
//                     onTriggered: callctl.setInput(modelData.node) }
//
// Replaces the three "✓ "-text-prefix idioms (labels shifted on selection).
import QtQuick.Controls as Controls

Controls.MenuItem {
    id: item
    property var isChecked: function () { return false }
    checkable: true
    checked: isChecked()
    onToggled: checked = Qt.binding(isChecked)
}
