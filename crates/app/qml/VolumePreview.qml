// VolumePreview — Settings > Notifications > Sound volume (QOL sweep D07.1): the slider was silent, so nobody could hear
// how loud 60 % is without leaving the row for a Preview button. Letting go of it (or a keyboard step) now plays the
// notification sound in use at the new level, or the message sound when the notification sound is Off; a burst of
// steps plays once, debounceMs after the last, and a new press cancels a preview still waiting. Logic only; SettingsWindow.qml wires the slider. The rule is
// Tin Hat Web's (src/features/settings/volumePreview.ts); both sides run testvectors/volume-preview/vectors.json (here:
// scripts/qol-parked-test.qml, run by scripts/qml-lint.sh).
import QtQml

QtObject {
    readonly property int debounceMs: 250
    // the sound file, from the prefs snd.notify and snd.notify.variant
    function file(notify, variant) {
        if (notify === "0") return "receive"
        return (variant === "2" || variant === "3" || variant === "4") ? "notify-" + variant : "notify"
    }
}
