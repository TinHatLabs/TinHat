// RowLayoutShim — tiny centered icon+label row used by GlowButton's contentItem.
// Kept separate so GlowButton.qml stays a clean Controls.Button override and we
// avoid an inline RowLayout (which needs its own import dance inside a contentItem).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

RowLayout {
    id: r
    // Optional (NOT required): GlowButton mounts this without setting `app`.
    property var app: null
    property string labelText: ""
    property font labelFont: Kirigami.Theme.defaultFont
    property color labelColor: Th.buttonCtaFg
    property string iconSource: ""

    // Center the cluster within the button.
    Item { Layout.fillWidth: true }
    Kirigami.Icon {
        visible: r.iconSource.length > 0
        source: r.iconSource
        color: r.labelColor
        implicitWidth: Kirigami.Units.iconSizes.small
        implicitHeight: Kirigami.Units.iconSizes.small
    }
    Controls.Label {
        text: r.labelText
        color: r.labelColor
        // Explicit pass-through: the shim may sit under a plain wrapper Item (GlowButton's
        // busy container), which breaks parent-font inheritance.
        font: r.labelFont
        elide: Text.ElideRight
    }
    Item { Layout.fillWidth: true }
}
