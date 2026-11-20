// RoomMissing — a room link to a room this account isn't in (PM 2026-11-14, found on Android). room.rs gives up after
// waitMs with `explanation` as the room's status; ConversationView shows `title` + `explanation` and a `button` back to
// the room list instead of "Couldn't load this conversation". The words are Tin Hat Web's
// (src/features/chat/roomMissing.ts); both sides run testvectors/room-missing/vectors.json (here:
// scripts/qol-parked-test.qml, run by scripts/qml-lint.sh, and cargo test room_missing).
import QtQml

QtObject {
    readonly property int waitMs: 10000
    readonly property string title: "Can't open this room"
    readonly property string explanation: "You're not in it, or it doesn't exist. Ask someone in it to invite you."
    readonly property string button: "Back to your rooms"
    // the room's status says it isn't in the local store (room.rs ROOM_MISSING_EXPLANATION)
    function isMissing(status) { return status === explanation }
}
