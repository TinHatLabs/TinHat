// ThSlider — the house slider (fix/sliders-volume, 2026-09-28). Every slider of the app is one; theme-lint refuses a
// stock Controls.Slider / RangeSlider anywhere else.
//
// It IS the platform style's own Slider (Breeze on Plasma keeps its look; Basic keeps its shape) with the palette roles
// the Basic style paints it from set to the slider tokens the web's Slider paints (src/ui/Slider.module.css).
// Basic (Windows, and the Linux rig's Basic lanes) draws the filled part in palette.dark, which Main.qml sets to
// surface.window — black on OLED, the window colour in every theme — so no slider showed how far it was set (PARITY
// P44, like the switch's ON track); its groove is palette.midlight, which Main.qml never sets (a KColorScheme shade on
// Linux, the OS palette on Windows: #e4e4e4 there, not the web's groove). Here:
//   fill    palette.dark      = slider.fill (accent.base)
//   groove  palette.midlight  = slider.track (also the handle frame while disabled = slider.handle.border.disabled,
//                               the same expression and the same OLED / Goose pins)
//   handle  palette.window    = slider.handle.bg        (pressed: palette.light = slider.handle.bg.pressed)
//   frame   palette.mid       = slider.handle.border    (focus: palette.highlight = slider.handle.border.focus)
// `callHandle` = the call menus' and the pop-out's handle: call.slider.thumb with a call.slider.thumb.border ring (the
// web's menus.module.css .volSlider / Pip.module.css .slider — a window-coloured handle vanished on the black menu).
// On OLED / Goose the groove, the handle and its frames are the values they had (surface.window, border.subtle,
// accent.base, the rig's #b1b1b1 / #e995e9 pins); the fill (and the call handle) are what change. Breeze paints its
// groove, fill and handle from Kirigami.Theme and never reads these roles, so it does not change. Nothing else in a
// Basic Slider reads them (6.10+ also frames the groove in palette.dark / mid, only under a high-contrast preference).
// The wheel is the style's: org.kde.desktop's Slider (Plasma) steps on it itself (its own MouseArea: increase /
// decrease + moved(), never a press), Basic's leaves wheelEnabled false. The volume dials set wheelEnabled: true, so
// Windows steps them as Plasma and the web do, and save a wheel step at once (a moved() without a press has no release).
import QtQuick
import QtQuick.Controls as Controls
import dev.tinhat.app   // Th: slider.*, call.slider.thumb(.border)

Controls.Slider {
    property bool callHandle: false
    palette.dark: Th.sliderFill
    palette.midlight: Th.sliderTrack
    palette.window: callHandle ? Th.callSliderThumb : Th.sliderHandleBg
    palette.light: Th.sliderHandleBgPressed
    palette.mid: callHandle ? Th.callSliderThumbBorder : Th.sliderHandleBorder
    palette.highlight: Th.sliderHandleBorderFocus
}
