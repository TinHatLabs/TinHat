// ThSwitch — the house switch (fix/basic-switch-on, 2026-09-28). Every switch of the app is one; theme-lint refuses a
// stock Controls.Switch / SwitchDelegate anywhere else.
//
// It IS the platform style's own Switch (Breeze on Plasma keeps its look, Basic keeps its shape and its OFF track) with
// one role changed: the checked track. The Basic style (Windows, and the Linux rig) paints a checked Switch's track in
// palette.dark, which Main.qml sets to surface.window — black on OLED, the window colour in every theme — so an ON
// switch showed only its knob and read as OFF (PARITY P44). Here palette.dark = toggle.track.checked (accent.base):
// the accent track the web draws (CheckBox.module.css `.checked .track`, DECISIONS D2) and Breeze draws. Breeze
// paints its track from Kirigami's highlight colour and never reads palette.dark, so it does not change; nothing else
// in a Basic Switch reads palette.dark (Qt 6.10+ also frames the track with it, only under a high-contrast preference).
import QtQuick
import QtQuick.Controls as Controls
import dev.tinhat.app   // Th: toggle.track.checked

Controls.Switch {
    palette.dark: Th.toggleTrackChecked
    // 11.1.1 (2026-09-28): the Basic OFF track is palette.midlight, which Main.qml never set, so Windows
    // drew it in the OS palette (#e4e4e4), nearly invisible in light themes. toggle.track is the web's OFF track
    // (CheckBox.module.css .track); Breeze ignores it, like palette.dark.
    palette.midlight: Th.toggleTrack
}
