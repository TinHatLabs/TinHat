// ThIcon.qml — one icon, two renderers (0.8.26, the icon-pack layer).
//
// Draws a freedesktop icon NAME exactly like Kirigami.Icon did — unless the icon pack
// (IconPackController → app.mIconPack) carries the MOTION engine, in which case its
// `motion/ThIcon.qml` (loaded by URL from the pack dir, so the pack stays editable and
// swappable without a rebuild) draws the same name as a vector hover story: reveal a
// detail, act, settle, hold while hovered/active, unwind on exit. Names its catalogue
// lacks fall through to the static theme glyph inside its component, so this is a
// complete replacement whenever the pack is present.
//
// Call sites pass `hovered` and `active`; `active` (mic live, call joined, panel open, tab
// selected) HOLDS the story's end pose via its `focused` input, which is how an "on"
// control reads as on. `playing` forces the held pose (busy states). Base/accent colours
// come from the theme tokens the call site already uses.
import QtQuick
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: li
    property var app: null            // Main.qml root (mIconPack, mReducedMotion); optional
    property string name: ""          // freedesktop icon name ("" = nothing)
    property color color              // every caller passes the ink (the audit's dead #ffffff default is gone)
    // themes 1b (THEMES-UI §10.3): the icon style — "mono" keeps the accent tone in the glyph's own ink
    property color accentColor: (app && app.mIconStyle === "mono") ? li.color : Th.iconHighlight   // theme-lint: ok — ThIcon's own API (the motion engine's accent), not Main's DEPRECATED accentColor
    property bool isMask: true
    property bool hovered: false
    property bool active: false
    property bool playing: false      // explicit override (e.g. a busy state)
    // 11.0 (GPU.md §1, R3): the motion switch — while it is off (unfocused, Reduce motion) a story jumps to its end pose
    property bool reducedMotion: !Motion.animate
    property string trigger: ""       // kept for call-site compatibility (unused by the motion engine)

    readonly property var pack: (app && app.mIconPack && app.mIconPack.icons) ? app.mIconPack : null
    readonly property bool motionAvailable: !!(pack && pack.motion && pack.dir) && !_motionFailed
    readonly property var entry: (pack && name.length > 0 && pack.icons[name]) ? pack.icons[name] : null
    readonly property bool animated: motionAvailable && entry !== null
    property bool _motionFailed: false

    implicitWidth: Kirigami.Units.iconSizes.smallMedium
    implicitHeight: Kirigami.Units.iconSizes.smallMedium

    // Static path (no pack, or the engine failed to load): the plain theme icon.
    Kirigami.Icon {
        anchors.fill: parent
        visible: !li.motionAvailable
        source: li.name
        color: li.color
        isMask: li.isMask
    }

    // Motion path: the pack's component, by URL from the pack directory.
    Loader {
        id: motion
        anchors.fill: parent
        active: li.motionAvailable
        source: li.motionAvailable ? (li.pack.dir + "/motion/ThIcon.qml") : ""
        onStatusChanged: {
            if (status === Loader.Error) {
                console.warn("[iconpack] motion engine failed to load from " + source + " — static icons")
                li._motionFailed = true
            }
        }
        onLoaded: {
            item.name = Qt.binding(function () { return li.name })
            item.color = Qt.binding(function () { return li.color })
            item.accentColor = Qt.binding(function () { return li.accentColor })   // theme-lint: ok — the motion engine's API
            item.hovered = Qt.binding(function () { return li.hovered })
            item.focused = Qt.binding(function () { return li.active || li.playing })
            item.reducedMotion = Qt.binding(function () { return li.reducedMotion })
        }
    }
}
