// ThHeading.qml — A31.4 (12.1.17): Kirigami.Heading at the person's text size (Settings → Appearance → Text size).
// Kirigami's Heading sizes itself from the platform's default font only (Platform.Theme.defaultFont × the level's
// factor), so it never followed Th.textScale; this is the same rule × Th.textScale — identical at 100 %.
import QtQuick
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Kirigami.Heading {
    id: heading
    font.pointSize: {
        var factor = heading.level === 1 ? 1.35 : heading.level === 2 ? 1.20 : heading.level === 3 ? 1.15 : heading.level === 4 ? 1.10 : 1
        return Kirigami.Theme.defaultFont.pointSize * factor * Th.textScale
    }
}
