import QtQuick
import QtQuick.Window
import org.kde.kirigami as Kirigami
import "IconManifest.js" as Catalog

// Preserve the containing button's click, label, focus and accessibility behavior.
Item {
    id: root
    property string name: ""
    property color color: Kirigami.Theme.textColor
    property color accentColor: Kirigami.Theme.highlightColor
    property bool hovered: hover.hovered
    property bool focused: false
    property bool reducedMotion: false
    property real speed: 1
    readonly property bool animating: movement.running
    readonly property real progress: _progress
    readonly property int frame: Math.round(_progress * (Catalog.frameCount - 1))

    implicitWidth: 24
    implicitHeight: 24

    readonly property var _record: Catalog.icons[name] || null
    readonly property bool _engaged: enabled && visible && (hovered || focused)
    readonly property bool _useAtlas: !!_record && (_engaged || _progress > 0)
    readonly property real _side: Math.min(width, height)
    readonly property bool _small: _side <= 18
    readonly property int _size: _small ? 16 : _side <= 22 ? 22 : _side <= 24 ? 24 : 32
    readonly property bool _hasBase: !!_record && _record.tones.indexOf("base") >= 0
    readonly property bool _hasAccent: !!_record && _record.tones.indexOf("accent") >= 0
    readonly property real _accentMix: _record ? Math.max(0, Math.min(1, (_progress - _record.accentAt) / 0.22)) : 0
    readonly property bool _ready: drawing.item !== null && (!_hasBase || baseAtlas.status === Image.Ready)
        && (!_hasAccent || (neutralAccent.status === Image.Ready && litAccent.status === Image.Ready))
    readonly property string _baseSvg: drawing.item ? (_small ? drawing.item.smallBase : drawing.item.regularBase) : ""
    readonly property string _accentSvg: drawing.item ? (_small ? drawing.item.smallAccent : drawing.item.regularAccent) : ""
    property real _progress: 0

    // Tint the SVG stylesheet before Qt decodes it. This also works with the
    // software scene graph, where Kirigami's shader mask is unavailable.
    function tinted(svg, ink) {
        if (!svg) return ""
        const channel = n => Math.round(n * 255).toString(16).padStart(2, "0")
        const rgb = "#" + channel(ink.r) + channel(ink.g) + channel(ink.b)
        return "data:image/svg+xml," + encodeURIComponent(svg.replace("#eff0f1", rgb))
    }

    function updateMotion() {
        movement.stop()
        const target = _engaged && _record ? 1 : 0
        if (reducedMotion || !visible || !enabled) {
            _progress = target
            return
        }
        movement.from = _progress
        movement.to = target
        // Tin Hat 11.0 (GPU.md §1, R3): one duration token for icon motion too — every story takes the 400 ms step
        // (`verylong`, the cap) forward and 0.7× of it back, instead of its own 600–1100 ms duration
        movement.duration = Math.max(1, 400 * (target ? 1 : 0.7) * Math.abs(target - _progress) / Math.max(0.2, speed))
        if (_progress !== target) movement.start()
    }

    on_EngagedChanged: updateMotion()
    onReducedMotionChanged: updateMotion()
    onSpeedChanged: updateMotion()
    onNameChanged: { movement.stop(); _progress = 0; updateMotion() }
    Component.onCompleted: updateMotion()

    NumberAnimation {
        id: movement
        target: root
        property: "_progress"
        easing.type: Easing.Linear // Easing is already drawn into every motion track.
    }

    HoverHandler { id: hover }

    Loader {
        id: drawing
        source: root._record ? Qt.resolvedUrl("drawings/" + root._record.canonical + ".qml") : ""
    }

    Kirigami.Icon {
        anchors.centerIn: parent
        width: root._side
        height: root._side
        source: root._record ? "" : root.name
        color: root.color
        isMask: true
        visible: !root._record
        animated: false
    }

    Image {
        anchors.centerIn: parent
        width: root._side
        height: root._side
        sourceSize: Qt.size(Math.ceil(root._side * Screen.devicePixelRatio), Math.ceil(root._side * Screen.devicePixelRatio))
        source: drawing.item ? root.tinted(root._small ? drawing.item.restSmall : drawing.item.rest, root.color) : ""
        opacity: root.color.a
        visible: !root._useAtlas || !root._ready
        cache: true
    }

    Item {
        anchors.centerIn: parent
        width: root._side
        height: root._side
        clip: true
        visible: root._useAtlas && root._ready

        Item {
            x: -(root.frame % Catalog.columns) * root._side
            y: -Math.floor(root.frame / Catalog.columns) * root._side
            width: root._side * Catalog.columns
            height: root._side * Catalog.columns

            Image {
                id: baseAtlas
                anchors.fill: parent
                source: root._useAtlas ? root.tinted(root._baseSvg, root.color) : ""
                sourceSize: Qt.size(Math.ceil(root._side * Catalog.columns * Screen.devicePixelRatio), Math.ceil(root._side * Catalog.columns * Screen.devicePixelRatio))
                opacity: root.color.a
                cache: true
            }
            Image {
                id: neutralAccent
                anchors.fill: parent
                source: root._useAtlas ? root.tinted(root._accentSvg, root.color) : ""
                sourceSize: Qt.size(Math.ceil(root._side * Catalog.columns * Screen.devicePixelRatio), Math.ceil(root._side * Catalog.columns * Screen.devicePixelRatio))
                opacity: (1 - root._accentMix) * root.color.a
                cache: true
            }
            Image {
                id: litAccent
                anchors.fill: parent
                source: root._useAtlas ? root.tinted(root._accentSvg, root.accentColor) : ""
                sourceSize: Qt.size(Math.ceil(root._side * Catalog.columns * Screen.devicePixelRatio), Math.ceil(root._side * Catalog.columns * Screen.devicePixelRatio))
                opacity: root._accentMix * root.accentColor.a
                cache: true
            }
        }
    }
}
